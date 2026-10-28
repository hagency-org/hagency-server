//! Original AS ciphertext custody. Reading/decrypting never admits model work.
use super::*;
use diesel::sql_types::Jsonb;
use serde_json::{Value, json};
pub(crate) const CAPABILITY: &str = "agent-encrypted-inbox-v1";
const MAX_EVENT: usize = 131072;
#[derive(Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct Ciphertext {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=BigInt)]
    pub agent_generation: i64,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Text)]
    pub event_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub sender_mxid: String,
    #[diesel(sql_type=Text)]
    pub source_digest: String,
    #[diesel(sql_type=BigInt)]
    pub received_at_ms: i64,
    #[diesel(sql_type=Jsonb)]
    pub event: Value,
}
#[derive(diesel::QueryableByName)]
pub(crate) struct Header {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub sender_mxid: String,
}
// Independent of serde_json's preserve_order feature; never hash display order.
pub(super) fn canonical(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Object(o) => {
            let mut pairs = o.iter().collect::<Vec<_>>();
            pairs.sort_by_key(|(key, _)| *key);
            let mut out = Vec::new();
            for (k, v) in pairs {
                out.push(format!(
                    "{}:{}",
                    serde_json::to_string(k).map_err(|_| invalid())?,
                    canonical(v)?
                ));
            }
            format!("{{{}}}", out.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(canonical)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        _ => serde_json::to_string(v).map_err(|_| invalid())?,
    })
}
fn invalid() -> Error {
    Error::Invalid("invalid_ciphertext_event")
}
pub(super) fn seal_digest(c: &Ciphertext) -> Result<String> {
    use base64::Engine;
    let content = &c.event["content"];
    let raw = content["ciphertext"].as_str().ok_or_else(invalid)?;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(raw.trim_end_matches('='))
        .map_err(|_| invalid())?;
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes);
    let session = content["session_id"]
        .as_str()
        .ok_or_else(invalid)?
        .trim_end_matches('=');
    Ok(hash(&canonical(&json!([
        "agent-encrypted-seal-v1",
        c.agent_id,
        c.room_id,
        content["algorithm"],
        session,
        encoded
    ]))?))
}
pub(crate) fn parse(raw: &Value) -> Result<(Value, String)> {
    if raw["type"] != "m.room.encrypted"
        || raw.get("state_key").is_some()
        || raw["unsigned"].get("redacted_because").is_some()
    {
        return Err(invalid());
    }
    for (name, sigil) in [("event_id", '$'), ("room_id", '!'), ("sender", '@')] {
        matrix_id(raw[name].as_str().ok_or_else(invalid)?, sigil)?;
    }
    let c = &raw["content"];
    if c.get("m.relates_to").is_some_and(|v| !v.is_object())
        || c["algorithm"] != "m.megolm.v1.aes-sha2"
        || c.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "algorithm"
                        | "ciphertext"
                        | "session_id"
                        | "sender_key"
                        | "device_id"
                        | "m.relates_to"
                )
            })
        })
        || ["ciphertext", "session_id"].iter().any(|k| {
            c[k].as_str()
                .is_none_or(|s| s.is_empty() || s.chars().any(char::is_control))
        })
        || raw["origin_server_ts"].as_u64().is_none()
    {
        return Err(invalid());
    }
    let event = json!({"event_id":raw["event_id"],"room_id":raw["room_id"],"sender":raw["sender"],"origin_server_ts":raw["origin_server_ts"],"type":"m.room.encrypted","content":c});
    let encoded = canonical(&event)?;
    if encoded.len() > MAX_EVENT {
        return Err(invalid());
    }
    let digest = hash(&canonical(&json!(["agent-encrypted-inbox-v1", event]))?);
    Ok((event, digest))
}
impl TransportStore {
    pub(crate) async fn ingest_ciphertext(
        &self,
        transaction: &str,
        binding: &str,
        raw: &Value,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<()> {
        key(binding)?;
        let (event, digest) = parse(raw)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   Self::lock(db).await?;let now=Self::clock(db,now).await?;let scope=Self::scope(db,binding).await?;
   facts_mode(&scope,event["sender"].as_str().ok_or_else(invalid)?,f,now,true)?;
   if scope.service_paused||event["room_id"]!=scope.room_id{return Err(Error::Unauthorized("ciphertext_scope_revoked"));}
   #[derive(diesel::QueryableByName)]struct Source{#[diesel(sql_type=BigInt)] received_at_ms:i64}
   // Original full raw event must still be in a durable verified AS transaction.
   // This same lock serializes against routing compaction and AS acceptance.
   let source=sql_query("SELECT t.received_at_ms FROM hagency_agent_v1.inbound_transactions t JOIN hagency_agent_v1.routing_jobs j ON j.transaction_id=t.id WHERE t.id=$1 AND j.state='pending' AND t.body->'events' @> $2::jsonb").bind::<Text,_>(transaction).bind::<Jsonb,_>(json!([raw])).get_result::<Source>(db).await.optional()?.ok_or(Error::Unauthorized("original_ciphertext_required"))?;
   if source.received_at_ms<0||source.received_at_ms>now{return Err(invalid());}
   let puppet=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE puppet_mxid=$1) AS matched").bind::<Text,_>(event["sender"].as_str().ok_or_else(invalid)?).get_result::<Flag>(db).await?;
   if puppet.matched||event["sender"].as_str().is_some_and(|s|s.starts_with("@_hagency_")){return Ok(());}
   #[derive(diesel::QueryableByName)]struct Digest{#[diesel(sql_type=Text)] source_digest:String}
   if let Some(old)=sql_query("SELECT source_digest FROM hagency_agent_v1.ciphertext_inbox WHERE binding_id=$1 AND event_id=$2").bind::<Text,_>(binding).bind::<Text,_>(event["event_id"].as_str().ok_or_else(invalid)?).get_result::<Digest>(db).await.optional()?{
    return if old.source_digest==digest{Ok(())}else{Err(Error::Conflict("ciphertext_changed"))};
   }
   if source.received_at_ms<=now.saturating_sub(self.limits.event_ttl_ms){return Err(Error::Conflict("event_expired"));}
   sql_query("UPDATE hagency_agent_v1.ciphertext_inbox SET state='expired',event=NULL WHERE state='pending' AND received_at_ms<=$1").bind::<BigInt,_>(now.saturating_sub(self.limits.event_ttl_ms)).execute(db).await?;
   let count=sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.ciphertext_inbox WHERE owner_user_id=$1 AND state='pending' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.encrypted_dispatches d WHERE d.inbox_id=hagency_agent_v1.ciphertext_inbox.id))+(SELECT count(*) FROM hagency_agent_v1.owner_events WHERE owner_user_id=$1 AND state IN ('pending','offered','acknowledged','running','unknown','encrypted_pending','encrypted_offered','encrypted_acknowledged','encrypted_running','encrypted_unknown')) AS total").bind::<Text,_>(&scope.owner_user_id).get_result::<Count>(db).await?;
   if count.total>=self.limits.max_owner_events{return Err(Error::Unavailable("owner_queue_full"));}
   let generation=sql_query("SELECT generation AS value FROM hagency_agent_v1.agents WHERE id=$1").bind::<Text,_>(&scope.agent_id).get_result::<Generation>(db).await?.value;
   sql_query("INSERT INTO hagency_agent_v1.ciphertext_inbox(id,transaction_id,binding_id,agent_id,owner_user_id,agent_generation,binding_generation,event_id,room_id,sender_mxid,source_digest,event,received_at_ms,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'pending')")
    .bind::<Text,_>(format!("enc_{}",entity_id()?)).bind::<Text,_>(transaction).bind::<Text,_>(binding).bind::<Text,_>(&scope.agent_id).bind::<Text,_>(&scope.owner_user_id).bind::<BigInt,_>(generation).bind::<BigInt,_>(scope.binding_generation).bind::<Text,_>(event["event_id"].as_str().ok_or_else(invalid)?).bind::<Text,_>(&scope.room_id).bind::<Text,_>(event["sender"].as_str().ok_or_else(invalid)?).bind::<Text,_>(digest).bind::<Jsonb,_>(&event).bind::<BigInt,_>(source.received_at_ms).execute(db).await?;
   Ok(())
  }).await
    }
    pub(crate) async fn ciphertext_candidates(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        binding: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<Header>> {
        key(binding)?;
        if limit == 0 || limit > 4 {
            return Err(Error::Invalid("invalid_batch_limit"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   let(now,_)=Self::authenticate(db,p,now).await?;Self::lease(db,p,lease,now).await?;Self::agent_active(db,p,&lease.agent_id).await?;
   let scope=Self::scope(db,binding).await?;if scope.owner_user_id!=p.user_id||scope.agent_id!=lease.agent_id||!scope.active||scope.service_paused{return Err(Error::Unauthorized("ciphertext_scope_revoked"));}
   Ok(sql_query("SELECT c.id,c.binding_id,c.sender_mxid FROM hagency_agent_v1.ciphertext_inbox c JOIN hagency_agent_v1.bindings b ON b.id=c.binding_id JOIN hagency_agent_v1.agents a ON a.id=c.agent_id WHERE c.owner_user_id=$1 AND c.agent_id=$2 AND c.binding_id=$3 AND c.state='pending' AND c.binding_generation=b.generation AND c.agent_generation=a.generation AND c.received_at_ms>$4 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.encrypted_dispatches d JOIN hagency_agent_v1.owner_events e ON e.id=d.dispatch_id WHERE d.inbox_id=c.id AND e.state NOT IN ('encrypted_pending','encrypted_offered')) ORDER BY c.received_at_ms,c.id LIMIT $5").bind::<Text,_>(&p.user_id).bind::<Text,_>(&lease.agent_id).bind::<Text,_>(binding).bind::<BigInt,_>(now.saturating_sub(self.limits.event_ttl_ms)).bind::<BigInt,_>(limit as i64).load(db).await?)
  }).await
    }
    pub(crate) async fn ciphertext_header(
        &self,
        p: &Principal,
        id: &str,
        now: i64,
    ) -> Result<Header> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{Self::authenticate(db,p,now).await?;sql_query("SELECT id,binding_id,sender_mxid FROM hagency_agent_v1.ciphertext_inbox WHERE id=$1 AND owner_user_id=$2").bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result(db).await.map_err(unauthorized)}).await
    }
    pub(crate) async fn read_ciphertext(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Ciphertext> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::current_ciphertext(db, p, lease, id, f, self.limits.event_ttl_ms, now)
                    .await
                    .map(|(event, _)| event)
            })
            .await
    }
    pub(super) async fn current_ciphertext(
        db: &mut AsyncPgConnection,
        p: &Principal,
        lease: &LeaseRef,
        id: &str,
        f: &DeliveryFacts,
        ttl: i64,
        now: i64,
    ) -> Result<(Ciphertext, i64)> {
        let (now, _) = Self::authenticate(db, p, now).await?;
        Self::lease(db, p, lease, now).await?;
        Self::agent_active(db, p, &lease.agent_id).await?;
        let c=sql_query("SELECT id,binding_id,agent_id,agent_generation,binding_generation,event_id,room_id,sender_mxid,source_digest,received_at_ms,event FROM hagency_agent_v1.ciphertext_inbox WHERE id=$1 AND owner_user_id=$2 AND state='pending' AND received_at_ms>$3").bind::<Text,_>(id).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(now.saturating_sub(ttl)).get_result::<Ciphertext>(db).await.map_err(unauthorized)?;
        let scope = Self::scope(db, &c.binding_id).await?;
        let generation =
            sql_query("SELECT generation AS value FROM hagency_agent_v1.agents WHERE id=$1")
                .bind::<Text, _>(&scope.agent_id)
                .get_result::<Generation>(db)
                .await?
                .value;
        if scope.owner_user_id != p.user_id
            || scope.agent_id != lease.agent_id
            || c.agent_id != lease.agent_id
            || c.binding_generation != scope.binding_generation
            || c.agent_generation != generation
            || scope.service_paused
        {
            return Err(Error::Unauthorized("ciphertext_scope_revoked"));
        }
        facts_mode(&scope, &c.sender_mxid, f, now, true)?;
        let (_, digest) = parse(&c.event)?;
        if digest != c.source_digest {
            return Err(Error::Conflict("ciphertext_changed"));
        }
        Ok((c, now))
    }
}
#[derive(diesel::QueryableByName)]
struct Generation {
    #[diesel(sql_type=BigInt)]
    value: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ciphertext_commitment_is_canonical_and_never_carries_plaintext_or_verification_flags() {
        let fixture: Value =
            serde_json::from_str(include_str!("ciphertext-wire-fixture.json")).unwrap();
        let mut raw = fixture["event"].clone();
        assert_eq!(parse(&raw).unwrap().1, fixture["sourceDigest"]);
        raw["unsigned"] = json!({"age":100,"verified":true});
        assert_eq!(parse(&raw).unwrap().1, fixture["sourceDigest"]);
        for (field, value) in [("type", json!("m.room.message")), ("state_key", json!(""))] {
            let mut wrong = raw.clone();
            wrong[field] = value;
            assert!(parse(&wrong).is_err());
        }
        let mut wrong = raw.clone();
        wrong["content"]["body"] = json!("untrusted plaintext");
        assert!(parse(&wrong).is_err());
        let mut threaded = raw.clone();
        threaded["content"]["m.relates_to"] =
            json!({"rel_type":"m.thread","event_id":"$root","is_falling_back":false});
        assert!(parse(&threaded).is_ok());
        assert_ne!(parse(&threaded).unwrap().1, fixture["sourceDigest"]);
        threaded["content"]["m.relates_to"] = json!("forged");
        assert!(parse(&threaded).is_err());
        raw["unsigned"]["redacted_because"] = json!({});
        assert!(parse(&raw).is_err());
    }
}
