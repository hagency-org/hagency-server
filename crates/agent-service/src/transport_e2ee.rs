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
#[derive(diesel::QueryableByName)]
struct IssuanceDevice {
    #[diesel(sql_type=Text)]
    puppet_mxid: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    kind: String,
    #[diesel(sql_type=Bool)]
    revoked: bool,
    #[diesel(sql_type=Bool)]
    retiring: bool,
    #[diesel(sql_type=Nullable<Text>)]
    issuance_nonce: Option<String>,
    #[diesel(sql_type=BigInt)]
    issuance_until_ms: i64,
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
        let nonce = secret_token();
        let (executor, generation) = device(p)?;
        let id = {
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
                let now=Self::authorize_matrix_device_issue(db,p,lease).await?;
                // Normally one pass. An expired, uncertain issuance retires its
                // old Olm identity before selecting a fresh device on the second.
                for _ in 0..2 {
                    let id=crate::executor_matrix_device::select(db,&lease.agent_id,executor,generation,initial).await?;
                    sql_query("INSERT INTO hagency_agent_v1.executor_matrix_devices(scope_id,puppet_mxid,device_id,executor_device_id,executor_generation,kind) VALUES($1,$2,$3,$4,$5,'personal') ON CONFLICT(scope_id,device_id) DO NOTHING")
                        .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(puppet).bind::<Text,_>(&id).bind::<Text,_>(executor).bind::<BigInt,_>(generation).execute(db).await?;
                    let saved=sql_query("SELECT puppet_mxid,executor_device_id,executor_generation,kind,revoked,retiring,issuance_nonce,issuance_until_ms FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND device_id=$2 FOR UPDATE")
                        .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).get_result::<IssuanceDevice>(db).await?;
                    if saved.puppet_mxid!=puppet || saved.executor_device_id!=executor || saved.executor_generation!=generation || saved.kind!="personal" || saved.retiring || saved.revoked {
                        return Err(Error::Conflict("matrix_device_identity_changed"));
                    }
                    if saved.issuance_nonce.is_some() {
                        if saved.issuance_until_ms>now {return Err(Error::Conflict("matrix_device_issuance_pending"));}
                        // Preserve the old nonce: a late response can still restore
                        // its retirement obligation after a concurrent logout.
                        sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true,revoked=false WHERE scope_id=$1 AND device_id=$2")
                            .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).execute(db).await?;
                        continue;
                    }
                    sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET issuance_nonce=$3,issuance_until_ms=$4 WHERE scope_id=$1 AND device_id=$2")
                        .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Text,_>(&nonce).bind::<BigInt,_>(now+30_000).execute(db).await?;
                    return Ok(id);
                }
                Err(Error::Conflict("matrix_device_issuance_pending"))
            }).await?
        };
        // No DB session or global authorization lock crosses Matrix IO.
        let issued = matrix.login_device(puppet, &id).await;
        let login_completed = issued.is_ok();
        let checked:Result<String>=async {
            let token=issued?;
            let mut guard=self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
                let now=Self::authorize_matrix_device_issue(db,p,lease).await?;
                sql_query("SELECT true AS matched FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND device_id=$2 AND puppet_mxid=$3 AND executor_device_id=$4 AND executor_generation=$5 AND kind='personal' AND NOT revoked AND NOT retiring AND issuance_nonce=$6 AND issuance_until_ms>greatest($7,(extract(epoch from clock_timestamp())*1000)::bigint) FOR UPDATE")
                    .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Text,_>(puppet).bind::<Text,_>(executor).bind::<BigInt,_>(generation).bind::<Text,_>(&nonce).bind::<BigInt,_>(now).get_result::<Flag>(db).await.map_err(unauthorized)?;
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET issuance_nonce=NULL,issuance_until_ms=0 WHERE scope_id=$1 AND device_id=$2 AND issuance_nonce=$3")
                    .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Text,_>(&nonce).execute(db).await?;
                Ok(token)
            }).await
        }.await;
        match checked {
            Ok(token) => Ok((id, token)),
            Err(error) => {
                // Commit retirement before attempting logout, including when an
                // earlier cleanup already marked this ID revoked before login won.
                let retired:Result<usize>=async {
                    let mut db=self.db.lock().await?;
                    Ok(sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true,revoked=false,issuance_nonce=CASE WHEN $4 THEN NULL ELSE issuance_nonce END,issuance_until_ms=CASE WHEN $4 THEN 0 ELSE issuance_until_ms END,last_checked_ms=0 WHERE scope_id=$1 AND device_id=$2 AND issuance_nonce=$3")
                        .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Text,_>(&nonce).bind::<Bool,_>(login_completed).execute(&mut *db).await?)
                }.await;
                match retired {
                    Ok(0) => {}
                    Ok(_) => {
                        if matrix.logout_device(puppet, &id).await.is_ok() {
                            if let Ok(mut db) = self.db.lock().await {
                                // Unknown login outcomes retain their nonce until
                                // a later cleanup after the claim deadline.
                                let _=sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND issuance_nonce IS NULL")
                                    .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).execute(&mut *db).await;
                            }
                        }
                    }
                    Err(_) => {
                        // Phase one already persisted the claim. Expiry makes
                        // the cleanup worker retire it even after task cancellation.
                        tracing::warn!("Matrix issuance failed; durable claim awaits cleanup");
                    }
                }
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
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
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
        let mut db = self.db.lock().await?;
        sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND issuance_nonce IS NOT DISTINCT FROM $3 AND (issuance_nonce IS NULL OR issuance_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint)")
            .bind::<Text,_>(&lease.agent_id).bind::<Text,_>(&id).bind::<Nullable<Text>,_>(&nonce).execute(&mut *db).await?;
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
            #[diesel(sql_type=Nullable<Text>)]
            issuance_nonce: Option<String>,
        }
        let rows = {
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            // Serialize only the bounded DB decision, never the remote logout.
            Self::lock(db).await?;
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
                let mut db=self.db.lock().await?;
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND NOT revoked AND issuance_nonce IS NOT DISTINCT FROM $3 AND (issuance_nonce IS NULL OR issuance_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint)").bind::<Text,_>(&d.scope_id).bind::<Text,_>(&d.device_id).bind::<Nullable<Text>,_>(&d.issuance_nonce).execute(&mut *db).await?;
                Ok(())
            }.await;
            if let Err(e) = result {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
