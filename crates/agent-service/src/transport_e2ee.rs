//! Group addressing is confirmed by the leased Matrix device before ordinary
//! personal dispatch/budget admission. No decrypted text crosses this API.
use super::*;
use serde_json::{Value, json};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CryptoCandidateInput {
    pub lease: LeaseRef,
    pub binding_id: String,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub mentions: BTreeSet<String>,
    #[serde(default)]
    pub thread_root: Option<String>,
}
#[derive(diesel::QueryableByName)]
struct Candidate {
    #[diesel(sql_type=Text)]
    event_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=BigInt)]
    received_at_ms: i64,
}
impl TransportStore {
    pub(crate) async fn encrypted_candidates(
        &self,
        p: &Principal,
        input: CryptoCandidateInput,
    ) -> Result<Value> {
        let mut db = self.db.lock().await?;
        let now = crate::api::now_ms();
        Self::authenticate(&mut db, p, now).await?;
        Self::lease(&mut db, p, &input.lease, now).await?;
        let scope = Self::scope(&mut db, &input.binding_id).await?;
        if scope.agent_id != input.lease.agent_id || scope.owner_user_id != p.user_id {
            return Err(Error::Unauthorized("binding_denied"));
        }
        let rows=sql_query("SELECT event_id,payload,received_at_ms FROM hagency_agent_v1.personal_e2ee_candidates WHERE binding_id=$1 AND NOT done AND received_at_ms>$2 ORDER BY last_polled_ms,received_at_ms,event_id LIMIT 16")
            .bind::<Text,_>(&input.binding_id).bind::<BigInt,_>(now-self.limits.event_ttl_ms).load::<Candidate>(&mut *db).await?;
        for row in &rows {
            sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET last_polled_ms=$3 WHERE binding_id=$1 AND event_id=$2").bind::<Text,_>(&input.binding_id).bind::<Text,_>(&row.event_id).bind::<BigInt,_>(now).execute(&mut *db).await?;
        }
        let events = rows
            .iter()
            .map(|r| {
                serde_json::from_str::<Value>(&r.payload)
                    .map_err(|_| Error::Unavailable("invalid_saved_event"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"events":events,"puppetMxid":scope.puppet_mxid}))
    }
    pub(crate) async fn confirm_encrypted_candidate(
        &self,
        p: &Principal,
        input: CryptoCandidateInput,
        g: &crate::gateway::Gateway,
    ) -> Result<Value> {
        let event_id = input
            .event_id
            .as_deref()
            .ok_or(Error::Invalid("event_required"))?;
        matrix_id(event_id, '$')?;
        if input.mentions.len() > 128 {
            return Err(Error::Invalid("too_many_mentions"));
        }
        for m in &input.mentions {
            matrix_id(m, '@')?;
        }
        if let Some(t) = &input.thread_root {
            matrix_id(t, '$')?;
        }
        let (event, received, scope) = {
            let mut db = self.db.lock().await?;
            let now = crate::api::now_ms();
            Self::authenticate(&mut db, p, now).await?;
            Self::lease(&mut db, p, &input.lease, now).await?;
            let scope = Self::scope(&mut db, &input.binding_id).await?;
            if scope.agent_id != input.lease.agent_id || scope.owner_user_id != p.user_id {
                return Err(Error::Unauthorized("binding_denied"));
            }
            let c=sql_query("SELECT event_id,payload,received_at_ms FROM hagency_agent_v1.personal_e2ee_candidates WHERE binding_id=$1 AND event_id=$2")
                .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).get_result::<Candidate>(&mut *db).await.map_err(unauthorized)?;
            let mut event = serde_json::from_str::<RoutedEvent>(&c.payload)
                .map_err(|_| Error::Unavailable("invalid_saved_event"))?;
            event.mentioned_mxids = input.mentions;
            event.thread_root = input.thread_root;
            let confirmation =
                serde_json::to_string(&event).map_err(|_| Error::Invalid("invalid_event"))?;
            // Pin even ignored decisions. A retry cannot change addressing or
            // move this event to a different thread after an uncertain response.
            let changed = sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET confirmation=COALESCE(confirmation,$3) WHERE binding_id=$1 AND event_id=$2 AND (confirmation IS NULL OR confirmation=$3)")
                .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).bind::<Text,_>(&confirmation).execute(&mut *db).await?;
            if changed != 1 {
                return Err(Error::Conflict("encrypted_confirmation_changed"));
            }
            (event, c.received_at_ms, scope)
        };
        let f = g
            .delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &event.sender_mxid,
                &scope.puppet_mxid,
            )
            .await?;
        let result = self
            .ingest_routed_confirmed(
                &input.binding_id,
                event,
                &f,
                received,
                crate::api::now_ms(),
                true,
            )
            .await?;
        let mut db = self.db.lock().await?;
        sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET done=true WHERE binding_id=$1 AND event_id=$2")
            .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).execute(&mut *db).await?;
        Ok(json!({"confirmed":!matches!(result,RouteResult::Ignored)}))
    }
    pub(crate) async fn issue_matrix_device(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        puppet: &str,
        id: &str,
        matrix: &crate::matrix_client::MatrixClient,
    ) -> Result<(String, String)> {
        let mut db = self.db.lock().await?;
        Self::lease(&mut db, p, lease, crate::api::now_ms()).await?;
        let (device, generation) = device(p)?;
        let id =
            crate::executor_matrix_device::select(&mut db, &lease.agent_id, device, generation, id)
                .await?;
        sql_query("INSERT INTO hagency_agent_v1.executor_matrix_devices VALUES($1,$2,$3,$4,$5,'personal',false) ON CONFLICT(scope_id,device_id) DO UPDATE SET revoked=false")
            .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(puppet).bind::<Text,_>(&id).bind::<Text,_>(device).bind::<BigInt,_>(generation).execute(&mut *db).await?;
        let token = matrix.login_device(puppet, &id).await?;
        Ok((id, token))
    }
    pub(crate) async fn revoke_current_matrix_device(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        puppet: &str,
        initial: &str,
        matrix: &crate::matrix_client::MatrixClient,
    ) -> Result<String> {
        let mut db = self.db.lock().await?;
        Self::lease(&mut db, p, lease, crate::api::now_ms()).await?;
        let (device, generation) = device(p)?;
        let id = crate::executor_matrix_device::select(
            &mut db,
            &lease.agent_id,
            device,
            generation,
            initial,
        )
        .await?;
        sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true WHERE scope_id=$1 AND device_id=$2").bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).execute(&mut *db).await?;
        matrix.logout_device(puppet, &id).await?;
        sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2").bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).execute(&mut *db).await?;
        Ok(id)
    }
    pub(crate) async fn revoke_stale_matrix_devices(
        &self,
        m: &crate::matrix_client::MatrixClient,
    ) -> Result<()> {
        #[derive(diesel::QueryableByName)]
        struct Device {
            #[diesel(sql_type=Text)]
            scope_id: String,
            #[diesel(sql_type=Text)]
            puppet_mxid: String,
            #[diesel(sql_type=Text)]
            device_id: String,
        }
        let mut db = self.db.lock().await?;
        let rows=sql_query("SELECT d.scope_id,d.puppet_mxid,d.device_id FROM hagency_agent_v1.executor_matrix_devices d WHERE d.kind='personal' AND NOT d.revoked AND (d.retiring OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.devices e ON e.id=a.execution_device_id JOIN hagency_agent_v1.sessions s ON s.id=e.session_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE a.id=d.scope_id AND a.execution_kind='personal' AND a.state='active' AND e.id=d.executor_device_id AND e.generation=d.executor_generation AND NOT e.revoked AND NOT s.revoked AND u.active AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id AND b.state='active')))")
            .load::<Device>(&mut *db).await?;
        let mut failure = None;
        for d in rows {
            let result: Result<()> = async {
            sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true WHERE scope_id=$1 AND device_id=$2").bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).execute(&mut *db).await?;
            m.logout_device(&d.puppet_mxid, &d.device_id).await?;
            sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2")
                .bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).execute(&mut *db).await?;
            Ok(())
            }.await;
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
