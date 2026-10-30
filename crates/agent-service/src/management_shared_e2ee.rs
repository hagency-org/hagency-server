//! The assigned executor is a normal Matrix device. This journal holds only
//! event references and delivery receipts, never plaintext or encryption keys.
use super::*;
use crate::{matrix_client::MatrixClient, transport::RoutedEvent};
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CryptoInput {
    pub assignment_id: String,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub addressed: Option<bool>,
    #[serde(default)]
    pub thread_root: Option<String>,
}
#[derive(QueryableByName)]
struct Candidate {
    #[diesel(sql_type=Text)]
    event_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=BigInt)]
    received_at_ms: i64,
    #[diesel(sql_type=Text)]
    state: String,
}
#[derive(QueryableByName)]
struct Device {
    #[diesel(sql_type=Text)]
    scope_id: String,
    #[diesel(sql_type=Text)]
    puppet_mxid: String,
    #[diesel(sql_type=Text)]
    device_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Bool)]
    retiring: bool,
}
async fn authorized(
    db: &mut AsyncPgConnection,
    p: &Principal,
    id: &str,
    server: &str,
) -> Result<Membership> {
    DomainStore::authorize(db, p, 0).await?;
    let w = intake::assignment(db, id).await?;
    owner(&w, p)?;
    let now = clock(db).await?;
    if p.device_generation != Some(w.executor_generation)
        || !intake::running(db, id).await?
        || !intake::retainable(db, id, &w, server, now).await?
    {
        return Err(Error::Unauthorized("shared_crypto_authority_changed"));
    }
    intake::member(db, id).await
}
pub(super) fn device_id(scope: &str, device: &str, generation: i64) -> String {
    format!(
        "HAGE2EE{}",
        hash(&format!("{scope}:{device}:{generation}"))[..16].to_ascii_uppercase()
    )
}
/// Deliberately tiny and closed: even a buggy executor cannot upload a reply
/// body to an encrypted work record through this API.
pub(super) fn receipt(body: &str, room: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Receipt {
        algorithm: String,
        room_id: String,
        event_id: Option<String>,
        delivery: String,
    }
    let r: Receipt =
        serde_json::from_str(body).map_err(|_| Error::Invalid("encrypted_receipt_required"))?;
    if r.algorithm != "m.megolm.v1.aes-sha2"
        || r.room_id != room
        || r.event_id
            .as_ref()
            .is_some_and(|e| !e.starts_with('$') || e.len() > 255)
        || !matches!(r.delivery.as_str(), "sent" | "unknown")
        || (r.delivery == "sent") != r.event_id.is_some()
    {
        return Err(Error::Invalid("invalid_encrypted_receipt"));
    }
    Ok(r.event_id)
}
impl DomainStore {
    pub(crate) async fn retain_shared_encrypted(
        &self,
        event: &RoutedEvent,
        received: i64,
    ) -> Result<()> {
        if !event.encrypted || !event.body.is_empty() {
            return Err(Error::Invalid("ciphertext_reference_required"));
        }
        let payload = serde_json::to_string(event).map_err(|_| Error::Invalid("invalid_event"))?;
        let mut db = self.db.lock().await?;
        let now = clock(&mut db).await?;
        if received <= now - 86_400_000 {
            return Ok(());
        }
        if received > now || event.sender_mxid.starts_with("@_hagency_") {
            return Ok(());
        }
        sql_query("DELETE FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE received_at_ms<=$1").bind::<BigInt,_>(now-86_400_000).execute(&mut *db).await?;
        let full=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_e2ee_candidates c JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=c.assignment_id WHERE m.room_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_e2ee_candidates old WHERE old.assignment_id=c.assignment_id AND old.event_id=$2) GROUP BY c.assignment_id HAVING count(*)>=10000) AS matched").bind::<Text,_>(&event.room_id).bind::<Text,_>(&event.event_id).get_result::<Flag>(&mut *db).await?.matched;
        if full {
            return Err(Error::Unavailable("encrypted_candidate_capacity"));
        }
        // Binding identity, not a model process, selects the candidate. The
        // executor must confirm addressing before durable demand intake.
        sql_query("INSERT INTO hagency_agent_v1.management_shared_e2ee_candidates(assignment_id,event_id,payload,received_at_ms) SELECT m.assignment_id,$2,$3,$4 FROM hagency_agent_v1.management_agent_matrix m JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=m.assignment_id WHERE m.room_id=$1 AND m.state='matrix_verified' AND c.mode='running' AND m.puppet_mxid<>$5 ON CONFLICT DO NOTHING")
            .bind::<Text,_>(&event.room_id).bind::<Text,_>(&event.event_id).bind::<Text,_>(&payload).bind::<BigInt,_>(received).bind::<Text,_>(&event.sender_mxid).execute(&mut *db).await?;
        Ok(())
    }
    pub(crate) async fn shared_crypto_poll(
        &self,
        p: &Principal,
        after: Option<&str>,
    ) -> Result<Value> {
        let mut db = self.db.lock().await?;
        Self::authorize(&mut db, p, 0).await?;
        let (device, generation) = device(p)?;
        let ids=sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND a.executor_generation=$3 AND a.state='pending_native' AND m.state='matrix_verified' AND a.id>$4 ORDER BY a.id LIMIT 17")
            .bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<BigInt,_>(generation).bind::<Text,_>(after.unwrap_or("")).load::<Id>(&mut *db).await?;
        let next = (ids.len() == 17).then(|| ids[15].id.clone());
        let mut scopes = Vec::new();
        for id in ids.iter().take(16) {
            let m = match authorized(&mut db, p, &id.id, &self.server).await {
                Ok(m) => m,
                Err(Error::Unauthorized(_) | Error::NotFound(_) | Error::Conflict(_)) => continue,
                Err(e) => return Err(e),
            };
            let rows=sql_query("SELECT * FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE assignment_id=$1 AND state<>'ignored' AND received_at_ms>$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake i WHERE i.assignment_id=$1 AND i.event_id=management_shared_e2ee_candidates.event_id) ORDER BY last_polled_ms,received_at_ms,event_id LIMIT 16")
                .bind::<Text,_>(&id.id).bind::<BigInt,_>(clock(&mut db).await?-86_400_000).load::<Candidate>(&mut *db).await?;
            for row in &rows {
                sql_query("UPDATE hagency_agent_v1.management_shared_e2ee_candidates SET last_polled_ms=$3 WHERE assignment_id=$1 AND event_id=$2").bind::<Text,_>(&id.id).bind::<Text,_>(&row.event_id).bind::<BigInt,_>(clock(&mut db).await?).execute(&mut *db).await?;
            }
            let events = rows
                .iter()
                .map(|r| decode::<RoutedEvent>(&r.payload))
                .collect::<Result<Vec<_>>>()?;
            scopes.push(json!({"assignmentId":id.id,"puppetMxid":m.puppet_mxid,"roomId":m.room_id,"managerMxid":m.manager_mxid,"direct":m.pool_agent&&m.space_id.is_empty(),"events":events}));
        }
        Ok(json!({"scopes":scopes,"nextAfter":next}))
    }
    pub(crate) async fn shared_crypto_thread(
        &self,
        p: &Principal,
        input: CryptoInput,
    ) -> Result<Value> {
        let mut db = self.db.lock().await?;
        authorized(&mut db, p, &input.assignment_id, &self.server).await?;
        let thread = input.thread_root.ok_or(Error::Invalid("thread_required"))?;
        let known=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1 AND w.payload::jsonb->>'threadRoot'=$2) AS matched").bind::<Text,_>(&input.assignment_id).bind::<Text,_>(thread).get_result::<Flag>(&mut *db).await?.matched;
        Ok(json!({"known":known}))
    }
    pub(crate) async fn shared_crypto_device(
        &self,
        p: &Principal,
        input: CryptoInput,
        matrix: &MatrixClient,
    ) -> Result<Value> {
        key(&input.assignment_id)?;
        let mut db = self.db.lock().await?;
        let m = authorized(&mut db, p, &input.assignment_id, &self.server).await?;
        let (device, generation) = device(p)?;
        let initial = device_id(&input.assignment_id, device, generation);
        let id = crate::executor_matrix_device::select(
            &mut db,
            &input.assignment_id,
            device,
            generation,
            &initial,
        )
        .await?;
        // Persist intent before login. A crash after login still leaves a
        // revocation obligation. Serialize issue/revoke using this same lock.
        sql_query("INSERT INTO hagency_agent_v1.executor_matrix_devices VALUES($1,$2,$3,$4,$5,'shared',false) ON CONFLICT(scope_id,device_id) DO UPDATE SET revoked=false")
            .bind::<Text,_>(&input.assignment_id).bind::<Text,_>(&m.puppet_mxid).bind::<Text,_>(&id).bind::<Text,_>(device).bind::<BigInt,_>(generation).execute(&mut *db).await?;
        let token = matrix.login_device(&m.puppet_mxid, &id).await?;
        Ok(json!({"userId":m.puppet_mxid,"deviceId":id,"accessToken":token}))
    }
    pub(crate) async fn shared_crypto_confirm(
        &self,
        p: &Principal,
        input: CryptoInput,
        gateway: &Gateway,
    ) -> Result<Value> {
        key(&input.assignment_id)?;
        let event_id = input
            .event_id
            .as_deref()
            .ok_or(Error::Invalid("event_required"))?;
        let addressed = input
            .addressed
            .ok_or(Error::Invalid("addressing_required"))?;
        let (mut event, received, m) = {
            let mut db = self.db.lock().await?;
            let m = authorized(&mut db, p, &input.assignment_id, &self.server).await?;
            let c=sql_query("SELECT * FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE assignment_id=$1 AND event_id=$2")
                .bind::<Text,_>(&input.assignment_id).bind::<Text,_>(event_id).get_result::<Candidate>(&mut *db).await.map_err(hidden)?;
            if c.received_at_ms <= clock(&mut db).await? - 86_400_000 {
                return Err(Error::Conflict("encrypted_candidate_expired"));
            }
            if c.state == "ignored" {
                return Ok(json!({"confirmed":false}));
            }
            (decode::<RoutedEvent>(&c.payload)?, c.received_at_ms, m)
        };
        let f = gateway
            .shared_delivery(
                &m.room_id,
                &m.space_id,
                &m.manager_mxid,
                &event.sender_mxid,
                &m.puppet_mxid,
            )
            .await?;
        facts(&m, &f)?;
        if !f.encrypted || event.room_id != m.room_id {
            return Err(Error::Unauthorized("encrypted_room_required"));
        }
        if let Some(root) = &input.thread_root {
            if !root.starts_with('$') || root.len() > 255 {
                return Err(Error::Invalid("invalid_thread"));
            }
        }
        event.body = event.event_id.clone();
        event.thread_root = input.thread_root.clone();
        event.mentioned_mxids.insert(m.puppet_mxid.clone());
        let confirmed_payload =
            serde_json::to_string(&event).map_err(|_| Error::Invalid("invalid_event"))?;
        {
            let mut db = self.db.lock().await?;
            authorized(&mut db, p, &input.assignment_id, &self.server).await?;
            let prior=sql_query("SELECT * FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE assignment_id=$1 AND event_id=$2").bind::<Text,_>(&input.assignment_id).bind::<Text,_>(event_id).get_result::<Candidate>(&mut *db).await?;
            if prior.state == "ignored" {
                return Ok(json!({"confirmed":false}));
            }
            if prior.state == "confirmed" && (!addressed || prior.payload != confirmed_payload) {
                return Err(Error::Conflict("encrypted_confirmation_changed"));
            }
            let pinned=sql_query("UPDATE hagency_agent_v1.management_shared_e2ee_candidates SET state=$3,payload=$4 WHERE assignment_id=$1 AND event_id=$2 AND (state='pending' OR (state=$3 AND payload=$4))")
                .bind::<Text,_>(&input.assignment_id).bind::<Text,_>(event_id).bind::<Text,_>(if addressed {"confirmed"}else{"ignored"}).bind::<Text,_>(&confirmed_payload).execute(&mut *db).await?;
            if pinned != 1 {
                return Err(Error::Conflict("encrypted_confirmation_changed"));
            }
        }
        if addressed {
            self.retain_shared_event_for(&event, received, gateway, Some(&input.assignment_id))
                .await?;
        }
        Ok(json!({"confirmed":addressed}))
    }
    pub(crate) async fn revoke_shared_crypto_devices(&self, matrix: &MatrixClient) -> Result<()> {
        let mut db = self.db.lock().await?;
        let devices=sql_query("SELECT * FROM hagency_agent_v1.executor_matrix_devices WHERE kind='shared' AND NOT revoked ORDER BY scope_id,device_id")
            .load::<Device>(&mut *db).await?;
        let mut failure = None;
        for d in devices {
            let result: Result<()> = async {
            let assignment = match intake::assignment(&mut db, &d.scope_id).await {
                Ok(w) => Some(w), Err(Error::NotFound(_)) => None, Err(e) => return Err(e),
            };
            let now = clock(&mut db).await?;
            let live = if let Some(w) = assignment { !d.retiring && w.executor_device_id == d.executor_device_id
                && w.executor_generation == d.executor_generation
                && intake::member(&mut db, &d.scope_id).await.is_ok()
                && intake::running(&mut db, &d.scope_id).await?
                && intake::retainable(&mut db, &d.scope_id, &w, &self.server, now).await?
            } else { false };
            if !live {
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true WHERE scope_id=$1 AND device_id=$2").bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).execute(&mut *db).await?;
                matrix.logout_device(&d.puppet_mxid, &d.device_id).await?;
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2")
                    .bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).execute(&mut *db).await?;
            }
            Ok(())
            }.await;
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_receipts_reject_plaintext_and_extra_fields() {
        assert!(receipt("secret answer", "!r:s").is_err());
        assert!(receipt(r#"{"algorithm":"m.megolm.v1.aes-sha2","delivery":"sent","eventId":"$e","roomId":"!r:s","body":"secret"}"#,"!r:s").is_err());
        assert!(receipt(r#"{"algorithm":"m.megolm.v1.aes-sha2","delivery":"sent","eventId":"$e","roomId":"!other:s"}"#,"!r:s").is_err());
        assert_eq!(receipt(r#"{"algorithm":"m.megolm.v1.aes-sha2","delivery":"sent","eventId":"$e","roomId":"!r:s"}"#,"!r:s").unwrap(),Some("$e".into()));
    }
}
