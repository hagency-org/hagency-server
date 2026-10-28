//! Current execution-host identity only. This does not grant encrypted task or send authority.
use super::*;
use crate::matrix_client::MatrixClient;
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use diesel::QueryableByName;
use serde_json::{Value, json};

pub(crate) const CAPABILITY: &str = "agent-crypto-identity-v1";
pub(crate) const PREFIX: &str = "/hagency/agent-crypto/";
pub(crate) const MAX_BYTES: usize = 131_072;
#[path = "agent_crypto_receive.rs"]
pub(crate) mod receive;
static REQUESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Provision,
    Versions,
    Whoami,
    Upload,
    Query,
    Signing,
    Signatures,
    OwnerQuery,
    OwnerSignatures,
    ReceiveSync,
    OwnerClaim,
}
impl Operation {
    pub(crate) fn route(method: &str, path: &str) -> Result<Self> {
        match (method, path) {
            ("GET", "/_matrix/client/versions") => Ok(Self::Versions),
            ("GET", "/_matrix/client/v3/account/whoami") => Ok(Self::Whoami),
            ("POST", "/_matrix/client/v3/keys/upload") => Ok(Self::Upload),
            ("POST", "/_matrix/client/v3/keys/query") => Ok(Self::Query),
            ("POST", "/_matrix/client/v3/keys/device_signing/upload") => Ok(Self::Signing),
            ("POST", "/_matrix/client/v3/keys/signatures/upload") => Ok(Self::Signatures),
            _ => Err(Error::Unauthorized("agent_crypto_operation_not_allowed")),
        }
    }
    pub(crate) fn request(self, device: &str) -> (reqwest::Method, Vec<&str>) {
        use reqwest::Method;
        let tail = match self {
            Self::Provision => {
                return (
                    Method::PUT,
                    vec!["_matrix", "client", "v3", "devices", device],
                );
            }
            Self::Versions => return (Method::GET, vec!["_matrix", "client", "versions"]),
            Self::Whoami => {
                return (
                    Method::GET,
                    vec!["_matrix", "client", "v3", "account", "whoami"],
                );
            }
            Self::Upload => "upload",
            Self::OwnerClaim => "claim",
            Self::Query | Self::OwnerQuery => "query",
            Self::Signing => "device_signing/upload",
            Self::Signatures | Self::OwnerSignatures => "signatures/upload",
            Self::ReceiveSync => return (Method::GET, vec!["_matrix", "client", "v3", "sync"]),
        };
        let mut segments = vec!["_matrix", "client", "v3", "keys"];
        segments.extend(tail.split('/'));
        (Method::POST, segments)
    }
}

#[derive(QueryableByName)]
struct Deadline {
    #[diesel(sql_type=BigInt)]
    valid_until_ms: i64,
}
struct Scope {
    valid_until_ms: i64,
    agent: Agent,
    execution_device: String,
    device_generation: i64,
    matrix_device: String,
}
#[derive(QueryableByName)]
struct Pin {
    #[diesel(sql_type=Nullable<Text>)]
    digest: Option<String>,
}
fn invalid() -> Error {
    Error::Invalid("invalid_agent_crypto_request")
}
fn object<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .filter(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
        .ok_or_else(invalid)
}
fn public_key(value: &Value) -> Result<&str> {
    let s = value.as_str().ok_or_else(invalid)?;
    if STANDARD_NO_PAD.decode(s).map_err(|_| invalid())?.len() != 32 {
        return Err(invalid());
    }
    Ok(s)
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(hash(&serde_json::to_string(value).map_err(|_| invalid())?))
}
// Pin public identity before possible HTTP IO. A lost local crypto store cannot
// silently overwrite an existing Matrix device/cross-signing identity.
fn fingerprints(
    op: Operation,
    body: &Value,
    user: &str,
    device: &str,
) -> Result<Vec<(&'static str, String)>> {
    let mut pins = Vec::new();
    match op {
        Operation::Upload => {
            let o = object(body, &["device_keys", "one_time_keys", "fallback_keys"])?;
            if let Some(keys) = o.get("device_keys") {
                object(
                    keys,
                    &[
                        "user_id",
                        "device_id",
                        "algorithms",
                        "keys",
                        "signatures",
                        "unsigned",
                    ],
                )?;
                if keys["user_id"] != user || keys["device_id"] != device {
                    return Err(invalid());
                }
                let algorithms = keys["algorithms"].as_array().ok_or_else(invalid)?;
                let mut algorithms = algorithms
                    .iter()
                    .map(|v| v.as_str().ok_or_else(invalid))
                    .collect::<Result<Vec<_>>>()?;
                algorithms.sort_unstable();
                if algorithms != ["m.megolm.v1.aes-sha2", "m.olm.v1.curve25519-aes-sha2"] {
                    return Err(invalid());
                }
                let keys = keys["keys"]
                    .as_object()
                    .filter(|k| k.len() == 2)
                    .ok_or_else(invalid)?;
                let ed = public_key(keys.get(&format!("ed25519:{device}")).ok_or_else(invalid)?)?;
                let curve = public_key(
                    keys.get(&format!("curve25519:{device}"))
                        .ok_or_else(invalid)?,
                )?;
                pins.push((
                    "device_keys_digest",
                    digest(&(user, device, algorithms, ed, curve))?,
                ));
            }
            for name in ["one_time_keys", "fallback_keys"] {
                if let Some(value) = o.get(name) {
                    value.as_object().ok_or_else(invalid)?;
                }
            }
        }
        Operation::Signing => {
            let o = object(
                body,
                &["master_key", "self_signing_key", "user_signing_key"],
            )?;
            if o.is_empty() {
                return Err(invalid());
            }
            for (field, usage, column) in [
                ("master_key", "master", "master_key_digest"),
                (
                    "self_signing_key",
                    "self_signing",
                    "self_signing_key_digest",
                ),
                (
                    "user_signing_key",
                    "user_signing",
                    "user_signing_key_digest",
                ),
            ] {
                if let Some(value) = o.get(field) {
                    object(value, &["user_id", "usage", "keys", "signatures"])?;
                    if value["user_id"] != user || value["usage"] != json!([usage]) {
                        return Err(invalid());
                    }
                    let keys = value["keys"]
                        .as_object()
                        .filter(|k| k.len() == 1)
                        .ok_or_else(invalid)?;
                    let (id, key) = keys.iter().next().ok_or_else(invalid)?;
                    let key = public_key(key)?;
                    if id != &format!("ed25519:{key}") {
                        return Err(invalid());
                    }
                    pins.push((column, digest(&(user, usage, key))?));
                }
            }
        }
        Operation::Query => {
            let o = object(body, &["device_keys", "timeout"])?;
            if o.get("timeout")
                .is_some_and(|v| v.as_u64().is_none_or(|n| n > 5000))
            {
                return Err(invalid());
            }
            let users = body["device_keys"]
                .as_object()
                .filter(|u| u.len() == 1 && u.contains_key(user))
                .ok_or_else(invalid)?;
            let devices = users[user].as_array().ok_or_else(invalid)?;
            if devices.len() > 1 || devices.iter().any(|d| d.as_str() != Some(device)) {
                return Err(invalid());
            }
        }
        Operation::Signatures => {
            let users = body
                .as_object()
                .filter(|u| u.len() == 1 && u.contains_key(user))
                .ok_or_else(invalid)?;
            let signed = users[user]
                .as_object()
                .filter(|o| !o.is_empty() && o.len() <= 4)
                .ok_or_else(invalid)?;
            for (target, value) in signed {
                object(
                    value,
                    &[
                        "user_id",
                        "device_id",
                        "usage",
                        "algorithms",
                        "keys",
                        "signatures",
                        "unsigned",
                    ],
                )?;
                if value["user_id"] != user {
                    return Err(invalid());
                }
                if value.get("device_id").is_some() {
                    if value["device_id"] != device || target != device {
                        return Err(invalid());
                    }
                } else {
                    let keys = value["keys"]
                        .as_object()
                        .filter(|k| k.len() == 1)
                        .ok_or_else(invalid)?;
                    let (id, key) = keys.iter().next().ok_or_else(invalid)?;
                    let key = public_key(key)?;
                    if id != &format!("ed25519:{key}") || target != key {
                        return Err(invalid());
                    }
                    let usage = value["usage"]
                        .as_array()
                        .filter(|u| u.len() == 1)
                        .ok_or_else(invalid)?;
                    if !matches!(
                        usage[0].as_str(),
                        Some("master" | "self_signing" | "user_signing")
                    ) {
                        return Err(invalid());
                    }
                }
            }
        }
        Operation::OwnerQuery
        | Operation::OwnerSignatures
        | Operation::ReceiveSync
        | Operation::OwnerClaim => {
            return Err(invalid());
        }
        Operation::Provision | Operation::Versions | Operation::Whoami => {
            if !body.is_null() {
                return Err(invalid());
            }
        }
    }
    Ok(pins)
}
impl DomainStore {
    async fn crypto_scope(
        db: &mut AsyncPgConnection,
        p: &Principal,
        agent: &str,
        generation: i64,
    ) -> Result<Scope> {
        Self::authorize_identity(db, p, crate::api::now_ms()).await?;
        let device = p
            .device_id
            .as_deref()
            .ok_or(Error::Unauthorized("device_authorization_required"))?;
        let device_generation = p
            .device_generation
            .ok_or(Error::Unauthorized("device_authorization_required"))?;
        sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2 FOR SHARE")
            .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await.map_err(hidden)?;
        let a = Self::agent_db(db, p, agent).await?;
        if a.generation != generation {
            return Err(Error::Conflict("stale_agent_generation"));
        }
        if a.execution_device_id.as_deref() != Some(device) {
            return Err(Error::Unauthorized("execution_device_required"));
        }
        if !matches!(a.state.as_str(), "creating" | "active" | "suspended") {
            return Err(Error::Conflict("agent_retired"));
        }
        // Resample after the Agent row lock; an expired session cannot gain IO.
        Self::authorize_identity(db, p, crate::api::now_ms()).await?;
        let matrix_device = format!("HAGENCY_{}", digest(&("agent-crypto-v1", &a.id, device))?);
        let deadline =
            sql_query("SELECT valid_until_ms FROM hagency_agent_v1.sessions WHERE id=$1")
                .bind::<Text, _>(&p.session_id)
                .get_result::<Deadline>(db)
                .await?;
        Ok(Scope {
            valid_until_ms: deadline.valid_until_ms,
            agent: a,
            execution_device: device.into(),
            device_generation,
            matrix_device,
        })
    }
    pub(crate) async fn crypto_request(
        &self,
        matrix: &MatrixClient,
        p: &Principal,
        agent: &str,
        generation: i64,
        op: Operation,
        body: Value,
    ) -> Result<(u16, Value, String)> {
        key(agent)?;
        if generation < 1 || serde_json::to_vec(&body).map_err(|_| invalid())?.len() > MAX_BYTES {
            return Err(invalid());
        }
        // Commit immutable public pins BEFORE network IO, in a distinct transaction.
        // Cancellation or response loss cannot roll these witnesses back.
        let _permit = REQUESTS
            .try_acquire()
            .map_err(|_| Error::Unavailable("agent_crypto_busy"))?;
        let mut db = self.crypto_db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let s=Self::crypto_scope(db,p,agent,generation).await?;
            let pins=receive::fingerprints(op,&body,&s.agent.puppet_mxid,&s.matrix_device,&p.mxid)?;
            if let Some(expected)=receive::signing_digest(op,&body,&s.agent.puppet_mxid,&p.mxid)? {
                let pins=sql_query("SELECT user_signing_key_digest AS digest FROM hagency_agent_v1.agent_crypto_identities WHERE agent_id=$1 ORDER BY matrix_device_id FOR SHARE").bind::<Text,_>(agent).load::<Pin>(db).await?;
                if !pins.iter().any(|p|p.digest.as_ref()==Some(&expected)) || pins.iter().any(|p|p.digest.as_ref().is_some_and(|d|d!=&expected)){return Err(Error::Conflict("agent_crypto_identity_changed"));}
            }
            sql_query("INSERT INTO hagency_agent_v1.agent_crypto_identities(agent_id,first_agent_generation,execution_device_id,first_device_generation,matrix_device_id) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                .bind::<Text,_>(agent).bind::<BigInt,_>(generation).bind::<Text,_>(&s.execution_device).bind::<BigInt,_>(s.device_generation).bind::<Text,_>(&s.matrix_device).execute(db).await?;
            for (column,digest) in pins {
                // Column comes only from the closed fingerprints implementation.
                let query=if column=="device_keys_digest" {
                    format!("SELECT {column} AS digest FROM hagency_agent_v1.agent_crypto_identities WHERE matrix_device_id=$1 FOR UPDATE")
                } else {
                    // Cross-signing belongs to the Agent account, not to an
                    // execution-host credential epoch or even another host.
                    format!("SELECT {column} AS digest FROM hagency_agent_v1.agent_crypto_identities WHERE agent_id=$1 ORDER BY matrix_device_id FOR UPDATE")
                };
                let pins=sql_query(query).bind::<Text,_>(if column=="device_keys_digest" {s.matrix_device.as_str()}else{agent}).load::<Pin>(db).await?;
                if pins.iter().any(|pin|pin.digest.as_ref().is_some_and(|old|old!=&digest)) { return Err(Error::Conflict("agent_crypto_identity_changed")); }
                sql_query(format!("UPDATE hagency_agent_v1.agent_crypto_identities SET {column}=coalesce({column},$2) WHERE matrix_device_id=$1"))
                    .bind::<Text,_>(&s.matrix_device).bind::<Text,_>(digest).execute(db).await?;
            }
            Ok(())
        }).await?;
        // Keep actual session/device/Agent rows locked through this bounded IO.
        // Assignment, logout and revocation serialize against their real rows;
        // no global domain advisory lock is held over HTTP.
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                let s = Self::crypto_scope(db, p, agent, generation).await?;
                tokio::time::timeout(std::time::Duration::from_secs(12), async {
                    if matches!(op, Operation::Provision) {
                        let (status, versions) = matrix
                            .crypto_identity_request(
                                &s.agent.puppet_mxid,
                                &s.matrix_device,
                                Operation::Versions,
                                Value::Null,
                                s.valid_until_ms,
                            )
                            .await?;
                        if !(200..300).contains(&status) {
                            return Ok((status, versions, s.matrix_device));
                        }
                        if versions["versions"].as_array().is_none_or(|v| {
                            v.is_empty()
                                || v.len() > 64
                                || v.iter().any(|s| s.as_str().is_none_or(|s| s.len() > 32))
                        }) {
                            return Err(Error::Unavailable("invalid_agent_crypto_response"));
                        }
                        Self::authorize_identity(db, p, crate::api::now_ms()).await?;
                        let (status, value) = matrix
                            .crypto_identity_request(
                                &s.agent.puppet_mxid,
                                &s.matrix_device,
                                op,
                                body,
                                s.valid_until_ms,
                            )
                            .await?;
                        if !(200..300).contains(&status) {
                            return Ok((status, value, s.matrix_device));
                        }
                        Self::authorize_identity(db, p, crate::api::now_ms()).await?;
                        let (status, who) = matrix
                            .crypto_identity_request(
                                &s.agent.puppet_mxid,
                                &s.matrix_device,
                                Operation::Whoami,
                                Value::Null,
                                s.valid_until_ms,
                            )
                            .await?;
                        if !(200..300).contains(&status) {
                            return Ok((status, who, s.matrix_device));
                        }
                        return Ok((200, versions, s.matrix_device));
                    }
                    let (status, value) = matrix
                        .crypto_identity_request(
                            &s.agent.puppet_mxid,
                            &s.matrix_device,
                            op,
                            body,
                            s.valid_until_ms,
                        )
                        .await?;
                    Ok((status, value, s.matrix_device))
                })
                .await
                .map_err(|_| Error::Unavailable("agent_crypto_outcome_unknown"))?
            })
            .await
    }
}

#[cfg(test)]
#[path = "agent_crypto_tests.rs"]
mod tests;
