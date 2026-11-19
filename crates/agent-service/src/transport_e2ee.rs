//! Group addressing is confirmed by the leased Matrix device before ordinary
//! personal dispatch/budget admission. No decrypted text crosses this API.
use serde_json::{Value, json};

use super::*;
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
        self.db.write_transaction(async |db| {
            let now = crate::api::now_ms();
            Self::authenticate(db, p, now).await?;
            Self::lease(db, p, &input.lease, now).await?;
            let scope = Self::scope(db, &input.binding_id).await?;
            if scope.agent_id != input.lease.agent_id || scope.owner_user_id != p.user_id {
                return Err(Error::Unauthorized("binding_denied"));
            }
            let rows=sql_query("SELECT event_id,payload,received_at_ms FROM hagency_agent_v1.personal_e2ee_candidates WHERE binding_id=$1 AND NOT done AND received_at_ms>$2 ORDER BY last_polled_ms,received_at_ms,event_id LIMIT 16")
                .bind::<Text,_>(&input.binding_id).bind::<BigInt,_>(now-self.limits.event_ttl_ms).load::<Candidate>(db).await?;
            for row in &rows {
                sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET last_polled_ms=$3 WHERE binding_id=$1 AND event_id=$2").bind::<Text,_>(&input.binding_id).bind::<Text,_>(&row.event_id).bind::<BigInt,_>(now).execute(db).await?;
            }
            let events = rows
                .iter()
                .map(|r| {
                    serde_json::from_str::<Value>(&r.payload)
                        .map_err(|_| Error::Unavailable("invalid_saved_event"))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"events":events,"puppetMxid":scope.puppet_mxid}))
        }).await
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
            self.db.write_transaction(async |db| {
                let now = crate::api::now_ms();
                Self::authenticate(db, p, now).await?;
                Self::lease(db, p, &input.lease, now).await?;
                let scope = Self::scope(db, &input.binding_id).await?;
                if scope.agent_id != input.lease.agent_id || scope.owner_user_id != p.user_id {
                    return Err(Error::Unauthorized("binding_denied"));
                }
                let c=sql_query("SELECT event_id,payload,received_at_ms FROM hagency_agent_v1.personal_e2ee_candidates WHERE binding_id=$1 AND event_id=$2")
                    .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).get_result::<Candidate>(db).await.map_err(unauthorized)?;
                let mut event = serde_json::from_str::<RoutedEvent>(&c.payload)
                    .map_err(|_| Error::Unavailable("invalid_saved_event"))?;
                event.mentioned_mxids = input.mentions;
                event.thread_root = input.thread_root;
                let confirmation =
                    serde_json::to_string(&event).map_err(|_| Error::Invalid("invalid_event"))?;
                // Pin even ignored decisions. A retry cannot change addressing or
                // move this event to a different thread after an uncertain response.
                let changed = sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET confirmation=COALESCE(confirmation,$3) WHERE binding_id=$1 AND event_id=$2 AND (confirmation IS NULL OR confirmation=$3)")
                    .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).bind::<Text,_>(&confirmation).execute(db).await?;
                if changed != 1 {
                    return Err(Error::Conflict("encrypted_confirmation_changed"));
                }
                Ok((event, c.received_at_ms, scope))
            }).await?
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
        self.db.transaction(async |db| {
            sql_query("UPDATE hagency_agent_v1.personal_e2ee_candidates SET done=true WHERE binding_id=$1 AND event_id=$2")
                .bind::<Text,_>(&input.binding_id).bind::<Text,_>(event_id).execute(db).await?;
            Ok(json!({"confirmed":!matches!(result,RouteResult::Ignored)}))
        }).await
    }
    async fn authorize_matrix_device_issue(
        db: &mut AsyncPgConnection,
        p: &Principal,
        lease: &LeaseRef,
    ) -> Result<i64> {
        let (now, _) = Self::authenticate(db, p, crate::api::now_ms()).await?;
        Self::agent_active(db, p, &lease.agent_id).await?;
        Self::lease(db, p, lease, crate::api::now_ms()).await?;
        sql_query("SELECT true AS matched FROM hagency_agent_v1.agents a WHERE a.id=$1 AND a.execution_kind='personal' AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id AND b.state='active') FOR SHARE OF a")
            .bind::<Text,_>(&lease.agent_id).get_result::<Flag>(db).await.map_err(unauthorized)?;
        Ok(now)
    }
    pub(crate) async fn issue_matrix_device(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        puppet: &str,
        initial: &str,
        matrix: &crate::matrix_client::MatrixClient,
    ) -> Result<(String, String)> {
        let (executor, generation) = device(p)?;
        let claim = crate::executor_matrix_device::Issuance::new(
            &lease.agent_id,
            puppet,
            executor,
            generation,
            "personal",
        );
        let id = self
            .db
            .write_transaction(async |db| {
                let now = Self::authorize_matrix_device_issue(db, p, lease).await?;
                claim.reserve(db, initial, now).await
            })
            .await?;
        let issued = matrix.login_device(puppet, &id).await;
        let received = issued.is_ok();
        let result = async {
            let token = issued?;
            self.db
                .write_transaction(async |db| {
                    let now = Self::authorize_matrix_device_issue(db, p, lease).await?;
                    claim.confirm(db, &id, now).await
                })
                .await?;
            Ok(token)
        }
        .await;
        match result {
            Ok(token) => Ok((id, token)),
            Err(error) => {
                claim.retire(&self.db, matrix, &id, received).await;
                Err(error)
            }
        }
    }
    pub(crate) async fn revoke_current_matrix_device(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        puppet: &str,
        initial: &str,
        matrix: &crate::matrix_client::MatrixClient,
    ) -> Result<String> {
        let (id, nonce) = {
            self.db.write_transaction(async |db:&mut AsyncPgConnection| {
                Self::authenticate(db,p,crate::api::now_ms()).await?;
                Self::lease(db,p,lease,crate::api::now_ms()).await?;
                let (device,generation)=device(p)?;
                let id=crate::executor_matrix_device::select(db,&lease.agent_id,device,generation,initial).await?;
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true WHERE scope_id=$1 AND device_id=$2")
                    .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).execute(db).await?;
                #[derive(diesel::QueryableByName)]
                struct Nonce {#[diesel(sql_type=Nullable<Text>)] issuance_nonce: Option<String>}
                let nonce=sql_query("SELECT issuance_nonce FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND device_id=$2")
                    .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).get_result::<Nonce>(db).await.optional()?.and_then(|n|n.issuance_nonce);
                Ok((id,nonce))
            }).await?
        };
        // Retirement is durable before logout; no DB guard crosses network IO.
        matrix.logout_device(puppet, &id).await?;
        self.db.transaction(async |db| {
            sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND issuance_nonce IS NOT DISTINCT FROM $3 AND (issuance_nonce IS NULL OR issuance_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint)")
                .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Nullable<Text>,_>(&nonce).execute(db).await?;
            Ok(id)
        }).await
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
            #[diesel(sql_type=Nullable<Text>)]
            issuance_nonce: Option<String>,
        }
        let rows = {
            self.db.write_transaction(async |db:&mut AsyncPgConnection| {
            // Serialize only the bounded DB decision, never the remote logout.

            let rows=sql_query("SELECT d.scope_id,d.puppet_mxid,d.device_id,d.issuance_nonce FROM hagency_agent_v1.executor_matrix_devices d WHERE d.kind='personal' AND NOT d.revoked AND (d.retiring OR (d.issuance_nonce IS NOT NULL AND d.issuance_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint) OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.devices e ON e.id=a.execution_device_id JOIN hagency_agent_v1.sessions s ON s.id=e.session_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE a.id=d.scope_id AND a.execution_kind='personal' AND a.state='active' AND e.id=d.executor_device_id AND e.generation=d.executor_generation AND NOT e.revoked AND NOT s.revoked AND u.active AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id AND b.state='active'))) ORDER BY d.last_checked_ms,d.scope_id,d.device_id LIMIT 8 FOR UPDATE OF d").load::<Device>(db).await?;
            for d in &rows {
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true,last_checked_ms=greatest(last_checked_ms+1,$3) WHERE scope_id=$1 AND device_id=$2").bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).bind::<BigInt,_>(crate::api::now_ms()).execute(db).await?;
            }
            Ok(rows)
            }).await?
        };
        let mut failure = None;
        for d in rows {
            let result:Result<()>=async {
                m.logout_device(&d.puppet_mxid,&d.device_id).await?;
                self.db.transaction(async |db| {
                    sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND NOT revoked AND issuance_nonce IS NOT DISTINCT FROM $3 AND (issuance_nonce IS NULL OR issuance_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint)").bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).bind::<Nullable<Text>,_>(&d.issuance_nonce).execute(db).await?;
                    Ok(())
                }).await
}.await;
            if let Err(e) = result {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
