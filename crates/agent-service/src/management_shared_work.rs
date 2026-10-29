//! Shared work/reply journal. Personal delivery ownership is deliberately separate.
use super::*;
#[path="management_shared_approvals.rs"]
mod approvals;
#[path = "management_shared_context.rs"]
pub(super) mod context;
#[path = "management_shared_budget_refusals.rs"]
mod refusals;
use crate::management_shared_work_types::*;
use crate::{matrix_client::MatrixClient, transport::RoutedEvent};
use diesel::sql_types::Array;
#[derive(QueryableByName)]
struct WorkRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    runtime_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<BigInt>)]
    started_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<BigInt>)]
    completed_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    call_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    latest_receipt: Option<String>,
    #[diesel(sql_type=BigInt)]
    known_consumed_tokens: i64,
    #[diesel(sql_type=Nullable<BigInt>)]
    retained_tokens: Option<i64>,
    #[diesel(sql_type=Bool)]
    usage_complete: bool,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
}
async fn work_row(db: &mut AsyncPgConnection, id: &str) -> Result<WorkRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_shared_work WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn item(w: &WorkRow, r: &Runtime) -> Result<SharedWorkItem> {
    let v: SharedWorkItem = decode(&w.payload)?;
    v.validate()?;
    if w.runtime_id != r.id
        || v.id != w.id
        || v.assignment_id != r.assignment_id
        || v.runtime_command_digest != r.digest
        || allocation_digest(&v)? != w.digest
    {
        return Err(Error::Conflict("shared_work_changed"));
    }
    Ok(v)
}
async fn require_activity(
    db: &mut AsyncPgConnection,
    p: &Principal,
    r: &Runtime,
    a: &SharedActivityRef,
    server: &str,
    now: i64,
) -> Result<RuntimeScope> {
    a.validate()?;
    let w = original(db, r).await?;
    owner(&w, p)?;
    let c = command(r, server)?;
    if r.id != a.runtime_command_id
        || r.state != "qualified"
        || r.activity_epoch != a.epoch
        || r.activity_until_ms <= now
        || r.activity_hash.as_deref() != Some(hash(&a.token).as_str())
        || p.device_generation != Some(w.executor_generation)
        || !current(db, r, &w, &c, now).await?
    {
        return Err(Error::Conflict("shared_activity_expired_or_changed"));
    }
    Ok(c)
}
impl DomainStore {
    /// Called only with the canonical event parsed from a verified AS transaction.
    pub(crate) async fn route_shared_event(
        &self,
        policy: &ManagementPolicy,
        event: &RoutedEvent,
        received: i64,
        gateway: &Gateway,
    ) -> Result<()> {
        if policy.shared_durable_intake() {
            self.retain_shared_event(event, received, gateway).await
        } else {
            self.route_shared_event_page(policy, event, received, gateway, 32)
                .await
        }
    }
    // Exercise the production page-selection boundary with two actual managed
    // agents instead of creating 33 unrelated provider reservations in a fixture.
    #[cfg(test)]
    pub(crate) async fn route_shared_event_single_candidate(
        &self,
        policy: &ManagementPolicy,
        event: &RoutedEvent,
        received: i64,
        gateway: &Gateway,
    ) -> Result<()> {
        self.route_shared_event_page(policy, event, received, gateway, 1)
            .await
    }
    async fn route_shared_event_page(
        &self,
        policy: &ManagementPolicy,
        event: &RoutedEvent,
        received: i64,
        gateway: &Gateway,
        page_limit: i64,
    ) -> Result<()> {
        if !issuing(policy)
            || event.encrypted
            || event.is_edit
            || event.body.is_empty()
            || event.body.len() > MAX_SHARED_TEXT
        {
            return Ok(());
        }
        let ids = {
            let mut db = self.db.lock().await?;
            // Select the dedicated pool conversation or exact mention scope
            // before bounding the page. Group siblings cannot consume its slots.
            let mentions = event
                .mentioned_mxids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            sql_query("SELECT r.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=r.assignment_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.management_agent_requests q ON q.id=a.request_id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.state='qualified' AND NOT r.permanent_fence AND r.activity_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint AND m.state='matrix_verified' AND m.room_id=$1 AND (m.puppet_mxid=ANY($3) OR (q.pool_contribution_id IS NOT NULL AND m.space_id='' AND u.mxid=$2)) ORDER BY r.id LIMIT $4").bind::<Text,_>(&event.room_id).bind::<Text,_>(&event.sender_mxid).bind::<Array<Text>,_>(mentions).bind::<BigInt,_>(page_limit.clamp(1,32)).load::<Id>(&mut *db).await?
        };
        for id in ids {
            let scope = {
                let mut db = self.db.lock().await?;
                let r = runtime(&mut db, &id.id).await?;
                membership(&mut db, &r).await?
            };
            if !scope.addressed_by(&event.sender_mxid, &event.mentioned_mxids) {
                continue;
            }
            let observed = gateway
                .shared_delivery(
                    &scope.room_id,
                    &scope.space_id,
                    &scope.manager_mxid,
                    &event.sender_mxid,
                    &scope.puppet_mxid,
                )
                .await?;
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
                let r=runtime(db,&id.id).await?;let w=original(db,&r).await?;let c=command(&r,&self.server)?;let now=clock(db).await?;
                if !runtime_issuing(db,policy,&r).await? || r.state!="qualified" || r.activity_until_ms<=now || received<c.issued_at_ms || received>now || now-received>86_400_000 || !current(db,&r,&w,&c,now).await? {return Ok(());}
                let m=membership(db,&r).await?;facts(&m,&observed)?;
                if event.room_id!=m.room_id || !m.addressed_by(&event.sender_mxid,&event.mentioned_mxids) {return Err(Error::Unauthorized("shared_room_authority_changed"));}
                let old=sql_query("SELECT w.* FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes old ON old.id=w.runtime_id WHERE old.assignment_id=$1 AND w.event_id=$2").bind::<Text,_>(&r.assignment_id).bind::<Text,_>(&event.event_id).get_result::<WorkRow>(db).await.optional()?;
                let root=event.thread_root.as_deref().unwrap_or(&event.event_id);
                if let Some(old)=old {let old_runtime=runtime(db,&old.runtime_id).await?;let v=item(&old,&old_runtime)?;if v.room_id!=event.room_id || v.requester_mxid!=event.sender_mxid || v.body!=event.body || v.thread_root!=root {return Err(Error::Conflict("shared_event_changed"));}return Ok(());}
                // Turning intake off cannot turn an already retained event into
                // unrelated legacy work that bypasses its original Run revision.
                if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1 AND event_id=$2) AS matched").bind::<Text,_>(&r.assignment_id).bind::<Text,_>(&event.event_id).get_result::<Flag>(db).await?.matched {return Ok(());}
                let full=sql_query("SELECT count(*)>=100 AS matched FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1 AND state IN('queued','acknowledged','started','unknown')").bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched;
                if full {return Err(Error::Unavailable("shared_work_queue_full"));}
                let v=SharedWorkItem {id:format!("swork_{}",entity_id()?),assignment_id:r.assignment_id.clone(),runtime_command_digest:r.digest.clone(),event_id:event.event_id.clone(),room_id:event.room_id.clone(),requester_mxid:event.sender_mxid.clone(),thread_root:root.into(),body:event.body.clone(),body_digest:hash(&event.body),execution_id:format!("sx_{}",entity_id()?),created_at_ms:received};v.validate()?;
                sql_query("INSERT INTO hagency_agent_v1.management_shared_work(id,runtime_id,event_id,payload,digest,created_at_ms) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&v.id).bind::<Text,_>(&r.id).bind::<Text,_>(&v.event_id).bind::<Text,_>(wire(&v)?).bind::<Text,_>(allocation_digest(&v)?).bind::<BigInt,_>(received).execute(db).await?;context::register_mode(db,policy,&v.id).await?;Ok(())
            }).await?;
        }
        Ok(())
    }
    async fn shared_work_facts(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        a: &SharedActivityRef,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<DeliveryFacts> {
        if !issuing(policy) {
            return Err(Error::Unavailable("shared_execution_issuance_unavailable"));
        }
        let m = {
            let mut guard = self.db.lock().await?;
            (*guard)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, 0).await?;
                    let r = runtime(db, &a.runtime_command_id).await?;
                    protocol.check(&r)?;
                    require_budget_head(db, &r, &self.server).await?;
                    if !runtime_issuing(db, policy, &r).await? {
                        return Err(Error::Unavailable("shared_successor_issuance_unavailable"));
                    }
                    let now = clock(db).await?;
                    require_activity(db, p, &r, a, &self.server, now).await?;
                    membership(db, &r).await
                })
                .await?
        };
        gateway
            .shared_delivery(
                &m.room_id,
                &m.space_id,
                &m.manager_mxid,
                &m.manager_mxid,
                &m.puppet_mxid,
            )
            .await
    }
    pub async fn poll_shared_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        a: SharedActivityRef,
        gateway: &Gateway,
    ) -> Result<Option<SharedWorkItem>> {
        self.poll_shared_work_with_protocol(p, policy, a, gateway, WorkProtocol::Legacy)
            .await
    }
    pub async fn poll_shared_budget_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        a: SharedActivityRef,
        gateway: &Gateway,
    ) -> Result<Option<SharedWorkItem>> {
        self.poll_shared_work_with_protocol(p, policy, a, gateway, WorkProtocol::Budget)
            .await
    }
    async fn poll_shared_work_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        a: SharedActivityRef,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<Option<SharedWorkItem>> {
        a.validate()?;
        let observed = self
            .shared_work_facts(p, policy, &a, gateway, protocol)
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let r=runtime(db,&a.runtime_command_id).await?;protocol.check(&r)?;require_budget_head(db,&r,&self.server).await?;let now=clock(db).await?;if !runtime_issuing(db,policy,&r).await?{return Err(Error::Unavailable("shared_successor_issuance_unavailable"));}require_activity(db,p,&r,&a,&self.server,now).await?;facts(&membership(db,&r).await?,&observed)?;
            if matches!(protocol,WorkProtocol::Legacy) { demands::promote(db,p,policy,&r,&self.server,now).await?; } else {demands::budget::promote(db,p,policy,&r,&self.server,now).await?;}
            let row=sql_query("SELECT * FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1 AND state IN('queued','acknowledged') ORDER BY created_at_ms,id LIMIT 1").bind::<Text,_>(&r.id).get_result::<WorkRow>(db).await.optional()?;row.map(|w|item(&w,&r)).transpose()
        }).await
    }
    pub async fn acknowledge_shared_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
    ) -> Result<()> {
        self.acknowledge_shared_work_with_protocol(p, policy, input, gateway, WorkProtocol::Legacy)
            .await
    }
    pub async fn acknowledge_shared_budget_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
    ) -> Result<()> {
        self.acknowledge_shared_work_with_protocol(p, policy, input, gateway, WorkProtocol::Budget)
            .await
    }
    async fn acknowledge_shared_work_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<()> {
        key(&input.work_id)?;
        let observed = self
            .shared_work_facts(p, policy, &input.authority, gateway, protocol)
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let r=runtime(db,&input.authority.runtime_command_id).await?;protocol.check(&r)?;require_budget_head(db,&r,&self.server).await?;let now=clock(db).await?;if !runtime_issuing(db,policy,&r).await?{return Err(Error::Unavailable("shared_successor_issuance_unavailable"));}require_activity(db,p,&r,&input.authority,&self.server,now).await?;facts(&membership(db,&r).await?,&observed)?;let w=work_row(db,&input.work_id).await?;item(&w,&r)?;
            if w.digest!=input.work_digest {return Err(Error::Conflict("shared_work_changed"));}
            if w.state=="queued" {sql_query("UPDATE hagency_agent_v1.management_shared_work SET state='acknowledged',acknowledged_at_ms=$2 WHERE id=$1").bind::<Text,_>(&w.id).bind::<BigInt,_>(now).execute(db).await?;}Ok(())
        }).await
    }
    pub async fn start_shared_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
    ) -> Result<SharedWorkStart> {
        self.start_shared_work_with_protocol(p, policy, input, gateway, WorkProtocol::Legacy)
            .await
    }
    pub async fn start_shared_budget_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
    ) -> Result<SharedWorkStart> {
        self.start_shared_work_with_protocol(p, policy, input, gateway, WorkProtocol::Budget)
            .await
    }
    async fn start_shared_work_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkInput,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<SharedWorkStart> {
        key(&input.work_id)?;
        let observed = self
            .shared_work_facts(p, policy, &input.authority, gateway, protocol)
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let r=runtime(db,&input.authority.runtime_command_id).await?;protocol.check(&r)?;require_budget_head(db,&r,&self.server).await?;let now=clock(db).await?;if !runtime_issuing(db,policy,&r).await?{return Err(Error::Unavailable("shared_successor_issuance_unavailable"));}require_activity(db,p,&r,&input.authority,&self.server,now).await?;facts(&membership(db,&r).await?,&observed)?;let w=work_row(db,&input.work_id).await?;let v=item(&w,&r)?;context::require_plain(db,&w.id).await?;refusals::ensure_unfenced(db,&w.id).await?;
            if w.digest!=input.work_digest {return Err(Error::Conflict("shared_work_changed"));}
            if w.state=="queued" {return Err(Error::Conflict("shared_work_not_acknowledged"));}
            let newly=w.state=="acknowledged";
            if newly {let busy=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1 AND (state IN('started','unknown') OR (started_at_ms IS NOT NULL AND NOT usage_complete))) AS matched").bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched;if busy {return Err(Error::Conflict("shared_work_unsettled"));}
                sql_query("UPDATE hagency_agent_v1.management_shared_work SET state='started',started_at_ms=$2,start_epoch=$3 WHERE id=$1").bind::<Text,_>(&w.id).bind::<BigInt,_>(now).bind::<BigInt,_>(input.authority.epoch).execute(db).await?;
            }
            Ok(SharedWorkStart {work:v,work_digest:w.digest,activity_epoch:input.authority.epoch,newly_started:newly})
        }).await
    }
}

async fn reply_state(db: &mut AsyncPgConnection, id: &str) -> Result<Option<String>> {
    Ok(sql_query(
        "SELECT state AS payload FROM hagency_agent_v1.management_shared_replies WHERE work_id=$1",
    )
    .bind::<Text, _>(id)
    .get_result::<Json>(db)
    .await
    .optional()?
    .map(|v| v.payload))
}
async fn latest_work_receipt(
    db: &mut AsyncPgConnection,
    id: &str,
    observation: Option<&str>,
) -> Result<Option<SharedWorkReceipt>> {
    let r=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_work_receipts WHERE work_id=$1 AND ($2::text IS NULL OR id=$2) ORDER BY ledger_revision DESC LIMIT 1").bind::<Text,_>(id).bind::<Nullable<Text>,_>(observation).get_result::<Json>(db).await.optional()?;
    r.map(|v| decode(&v.payload)).transpose()
}
impl DomainStore {
    /// Exact executor history/upload only, including after generation rotation or
    /// cancellation. It exposes neither a new prompt nor model issuance authority.
    pub async fn recover_shared_work_result(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedWorkRecoverInput,
    ) -> Result<SharedWorkResultTicket> {
        enabled(policy)?;
        key(id)?;
        if let Some(v) = &input.observation_id {
            key(v)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let w=work_row(db,id).await?;let r=runtime(db,&w.runtime_id).await?;owner(&original(db,&r).await?,p)?;item(&w,&r)?;
            if r.digest!=input.runtime_command_digest || w.digest!=input.work_digest || w.started_at_ms.is_none() && !refusals::exists(db,id).await? {return Err(Error::Conflict("shared_work_changed"));}
            let now=clock(db).await?;let token=secret_token();let expiry=until(db,p,now).await?;
            sql_query("UPDATE hagency_agent_v1.management_shared_work SET lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(expiry).execute(db).await?;
            Ok(SharedWorkResultTicket {work_id:w.id.clone(),work_digest:w.digest,token,lease_until_ms:expiry,recorded:latest_work_receipt(db,id,input.observation_id.as_deref()).await?,state:w.state,reply_state:reply_state(db,id).await?})
        }).await
    }
    pub async fn record_shared_work_result(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedWorkResultUpload,
    ) -> Result<SharedWorkAcknowledgement> {
        enabled(policy)?;
        key(id)?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let w=work_row(db,id).await?;let r=runtime(db,&w.runtime_id).await?;let original=original(db,&r).await?;owner(&original,p)?;let c=command(&r,&self.server)?;let v=item(&w,&r)?;let now=clock(db).await?;
            if w.digest!=input.work_digest || w.started_at_ms.is_none() && !refusals::exists(db,id).await? || w.lease_hash.as_deref()!=Some(hash(&input.token).as_str()) || w.lease_until_ms<=now {return Err(Error::Conflict("shared_work_result_ticket_changed"));}
            if w.started_at_ms.is_none() {refusals::validate_zero(&input.receipt)?;}
            let process=r.process_digest.as_deref().ok_or(Error::Conflict("shared_process_required"))?;input.receipt.validate_for(&v,process)?;let digest=allocation_digest(&input.receipt)?;
            let old=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_work_receipts WHERE id=$1").bind::<Text,_>(&input.receipt.observation_id).get_result::<Json>(db).await.optional()?;
            if let Some(old)=old {let old:SharedWorkReceipt=decode(&old.payload)?;if allocation_digest(&old)?!=digest {return Err(Error::Conflict("shared_work_receipt_changed"));}}
            else {
                validate_work_receipt(db,&w,&r,&original,&input.receipt).await?;
                sql_query("INSERT INTO hagency_agent_v1.management_shared_work_receipts VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&input.receipt.observation_id).bind::<Text,_>(id).bind::<Text,_>(wire_with_limit(&input.receipt,48*1024)?).bind::<Text,_>(&digest).bind::<BigInt,_>(input.receipt.ledger_revision).execute(db).await?;
                let latest=latest_work_receipt(db,id,None).await?.unwrap();
                if latest.observation_id==input.receipt.observation_id {
                    sql_query("UPDATE hagency_agent_v1.management_shared_work SET state=$2,call_id=coalesce(call_id,$3),latest_receipt=$4,known_consumed_tokens=$5,retained_tokens=$6,usage_complete=$7,completed_at_ms=CASE WHEN $2 IN('completed','failed') THEN coalesce(completed_at_ms,$8) ELSE completed_at_ms END WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(&latest.state).bind::<Text,_>(&latest.call_id).bind::<Text,_>(&latest.observation_id).bind::<BigInt,_>(latest.known_consumed_tokens).bind::<Nullable<BigInt>,_>(latest.retained_tokens).bind::<Bool,_>(latest.usage_complete).bind::<BigInt,_>(now).execute(db).await?;
                    if latest.state=="completed" {
                        let body=latest.body.as_ref().unwrap();let content=crate::workers::reply_content(false,&v.room_id,&v.thread_root,body);let content=wire_with_limit(&content,64*1024)?;
                        let sendable=runtime_issuing(db,policy,&r).await?&&r.state=="qualified"&&r.activity_until_ms>now&&p.device_generation==Some(original.executor_generation)&&current(db,&r,&original,&c,now).await?;
                        sql_query("INSERT INTO hagency_agent_v1.management_shared_replies(work_id,content,content_digest,matrix_txn_id,created_at_ms,state) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(work_id) DO NOTHING").bind::<Text,_>(id).bind::<Text,_>(&content).bind::<Text,_>(hash(&content)).bind::<Text,_>(format!("hagency_shared_{id}")).bind::<BigInt,_>(now).bind::<Text,_>(if sendable {"pending"} else {"blocked"}).execute(db).await?;
                    }
                }
            }
            Ok(SharedWorkAcknowledgement {observation_id:input.receipt.observation_id,receipt_digest:digest,state:work_row(db,id).await?.state,reply_state:reply_state(db,id).await?})
        }).await
    }
    /// Read-only recovery index; issuance and the admission inbox may be disabled.
    pub async fn shared_work_index(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedWorkIndexQuery,
    ) -> Result<SharedWorkIndexPage> {
        enabled(policy)?;
        for value in [&input.project_id, &input.after].into_iter().flatten() {
            key(value)?;
        }
        let limit = input.limit.unwrap_or(16);
        if !(1..=16).contains(&limit) {
            return Err(Error::Invalid("invalid_shared_work_query"));
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;
            let mut rows=sql_query("SELECT json_build_object('requestId',q.id,'projectId',q.plan_id,'projectName',p.name,'agentName',q.definition::jsonb->>'name')::text AS payload FROM hagency_agent_v1.management_agent_requests q JOIN hagency_agent_v1.management_projects p ON p.id=q.plan_id JOIN hagency_agent_v1.management_project_authorities a ON a.plan_id=q.plan_id WHERE (q.manager_user_id=$1 OR a.administrator_user_id=$1) AND ($2::text IS NULL OR q.plan_id=$2) AND q.id>$3 AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments s JOIN hagency_agent_v1.management_shared_runtimes r ON r.assignment_id=s.id JOIN hagency_agent_v1.management_shared_work w ON w.runtime_id=r.id WHERE s.request_id=q.id) ORDER BY q.id LIMIT $4")
                .bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(&input.project_id).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(i64::from(limit)+1).load::<Json>(db).await?;
            let more=rows.len()>usize::from(limit);rows.truncate(usize::from(limit));
            let items=rows.into_iter().map(|r|decode::<SharedWorkIndexItem>(&r.payload)).collect::<Result<Vec<_>>>()?;
            let next_after=more.then(||items.last().unwrap().request_id.clone());
            Ok(SharedWorkIndexPage {items,next_after})
        }).await
    }
    pub async fn shared_work_history(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request: &str,
        input: SharedWorkQuery,
    ) -> Result<SharedWorkPage> {
        enabled(policy)?;
        key(request)?;
        if let Some(after) = &input.after {
            key(after)?;
        }
        let limit = input.limit.unwrap_or(16);
        if !(1..=16).contains(&limit) {
            return Err(Error::Invalid("invalid_shared_work_query"));
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {Self::authorize(db,p,0).await?;let request=super::super::super::row(db,request).await?;super::super::super::visible(db,&request,p).await?;
            let mut rows=sql_query("SELECT w.* FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.request_id=$1 AND w.id>$2 ORDER BY w.id LIMIT $3").bind::<Text,_>(&request.id).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(i64::from(limit)+1).load::<WorkRow>(db).await?;
            let more=rows.len()>usize::from(limit);rows.truncate(usize::from(limit));let next_after=more.then(||rows.last().unwrap().id.clone());let mut items=Vec::new();
            for w in rows {let r=runtime(db,&w.runtime_id).await?;let c=command(&r,&self.server)?;let v=item(&w,&r)?;let reply=sql_query("SELECT state AS payload FROM hagency_agent_v1.management_shared_replies WHERE work_id=$1").bind::<Text,_>(&w.id).get_result::<Json>(db).await.optional()?;
                items.push(SharedWorkHistoryItem {id:w.id,execution_id:v.execution_id,event_id:v.event_id,requester_mxid:v.requester_mxid,state:w.state,reply_state:reply.map(|v|v.payload),created_at_ms:v.created_at_ms,started_at_ms:w.started_at_ms,completed_at_ms:w.completed_at_ms,usage:w.latest_receipt.map(|_|SharedObservedUsage {known_consumed_tokens:w.known_consumed_tokens,retained_tokens:w.retained_tokens,complete:w.usage_complete,period_key:c.reserve_command.period_key.clone()})});
            }Ok(SharedWorkPage {items,next_after})
        }).await
    }
}
async fn validate_work_receipt(
    db: &mut AsyncPgConnection,
    w: &WorkRow,
    r: &Runtime,
    original: &Work,
    v: &SharedWorkReceipt,
) -> Result<()> {
    if w.call_id.as_ref().is_some_and(|id| id != &v.call_id) {
        return Err(Error::Conflict("shared_call_changed"));
    }
    let process = attempts::latest_evidence(db, &r.id, None)
        .await?
        .ok_or(Error::Conflict("shared_process_required"))?;
    // Process cleanup may be reported later than already-issued work, so only
    // the original qualified observation is a lower bound for call evidence.
    let qualified=sql_query("SELECT min(ledger_revision)::bigint AS now FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND payload::jsonb->>'state'='qualified'").bind::<Text,_>(&r.id).get_result::<Clock>(db).await?;
    if v.ledger_revision <= qualified.now || v.ledger_revision == process.ledger_revision {
        return Err(Error::Conflict("shared_work_ledger_revision_changed"));
    }
    super::super::validate_external_ledger_receipt(
        db,
        &original.executor_owner_user_id,
        &original.executor_device_id,
        v.ledger_revision,
        v.ledger_revision,
    )
    .await?;
    let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3 UNION ALL SELECT 1 FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND ((receipt::jsonb->>'ledgerRevision')::bigint=$3 OR (receipt::jsonb->>'fenceRevision')::bigint=$3) UNION ALL SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3) AS matched").bind::<Text,_>(&original.executor_owner_user_id).bind::<Text,_>(&original.executor_device_id).bind::<BigInt,_>(v.ledger_revision).get_result::<Flag>(db).await?.matched;
    if collision {
        return Err(Error::Conflict("shared_work_ledger_revision_changed"));
    }
    let old=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_work_receipts WHERE work_id=$1 ORDER BY ledger_revision LIMIT 256").bind::<Text,_>(&w.id).load::<Json>(db).await?;
    if old.len() >= 256 {
        return Err(Error::Conflict("shared_work_observation_limit"));
    }
    for old in old {
        let old: SharedWorkReceipt = decode(&old.payload)?;
        let (earlier, later) = if old.ledger_revision < v.ledger_revision {
            (&old, v)
        } else {
            (v, &old)
        };
        if old.ledger_revision == v.ledger_revision
            || old.call_id != v.call_id
            || (old.body.is_some() && v.body.is_some() && old.body != v.body)
            || earlier.known_consumed_tokens > later.known_consumed_tokens
            || (earlier.usage_complete && !later.usage_complete)
            || (earlier.state == "completed" && later.state != "completed")
            || (earlier.state == "failed" && later.state != "failed")
        {
            return Err(Error::Conflict("shared_work_evidence_changed"));
        }
    }
    Ok(())
}

#[derive(QueryableByName)]
struct Reply {
    #[diesel(sql_type=Text)]
    work_id: String,
    #[diesel(sql_type=Text)]
    content: String,
    #[diesel(sql_type=Text)]
    content_digest: String,
    #[diesel(sql_type=Text)]
    matrix_txn_id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    matrix_event_id: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
}
struct SendClaim {
    work_id: String,
    content: Value,
    content_digest: String,
    transaction: String,
    scope: Membership,
}
async fn reply_live(
    db: &mut AsyncPgConnection,
    policy: &ManagementPolicy,
    r: &Runtime,
    server: &str,
    now: i64,
) -> Result<bool> {
    let w = original(db, r).await?;
    let c = command(r, server)?;
    Ok(runtime_issuing(db, policy, r).await?
        && r.state == "qualified"
        && r.activity_until_ms > now
        && current(db, r, &w, &c, now).await?)
}
impl DomainStore {
    pub(crate) async fn send_shared_replies(
        &self,
        policy: &ManagementPolicy,
        gateway: &Gateway,
        matrix: &MatrixClient,
        cursor: &mut String,
    ) -> Result<()> {
        if !issuing(policy) {
            return Ok(());
        }
        let ids = {
            let mut db = self.db.lock().await?;
            sql_query("SELECT work_id AS id FROM hagency_agent_v1.management_shared_replies WHERE state IN('pending','issued','unknown') AND lease_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint ORDER BY (work_id<=$1),work_id LIMIT 16").bind::<Text,_>(&*cursor).load::<Id>(&mut *db).await?
        };
        for id in ids {
            *cursor = id.id.clone();
            let scope = {
                let mut db = self.db.lock().await?;
                let w = work_row(&mut db, &id.id).await?;
                let r = runtime(&mut db, &w.runtime_id).await?;
                let now = clock(&mut db).await?;
                if !reply_live(&mut db, policy, &r, &self.server, now).await? {
                    None
                } else {
                    Some(membership(&mut db, &r).await?)
                }
            };
            let observed = match scope {
                Some(m) => match tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    gateway.shared_delivery(
                        &m.room_id,
                        &m.space_id,
                        &m.manager_mxid,
                        &m.manager_mxid,
                        &m.puppet_mxid,
                    ),
                )
                .await
                {
                    Ok(Ok(f)) => Some(f),
                    _ => continue,
                },
                None => None,
            };
            let claim = {
                let mut guard = self.db.lock().await?;
                (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;let now=clock(db).await?;let w=work_row(db,&id.id).await?;let r=runtime(db,&w.runtime_id).await?;item(&w,&r)?;
                let reply=sql_query("SELECT * FROM hagency_agent_v1.management_shared_replies WHERE work_id=$1").bind::<Text,_>(&id.id).get_result::<Reply>(db).await?;
                if !matches!(reply.state.as_str(),"pending"|"issued"|"unknown") || reply.lease_until_ms>now {return Ok(None);}
                let m=membership(db,&r).await?;
                if !reply_live(db,policy,&r,&self.server,now).await? || observed.as_ref().is_none_or(|f|facts(&m,f).is_err()) {
                    sql_query("UPDATE hagency_agent_v1.management_shared_replies SET state='blocked' WHERE work_id=$1").bind::<Text,_>(&reply.work_id).execute(db).await?;return Ok(None);
                }
                if hash(&reply.content)!=reply.content_digest {return Err(Error::Conflict("shared_reply_changed"));}
                let token=secret_token();sql_query("UPDATE hagency_agent_v1.management_shared_replies SET state='issued',lease_hash=$2,lease_until_ms=$3 WHERE work_id=$1").bind::<Text,_>(&reply.work_id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(now+30_000).execute(db).await?;
                Ok(Some(SendClaim {work_id:reply.work_id,content:decode(&reply.content)?,content_digest:reply.content_digest,transaction:reply.matrix_txn_id,scope:m}))
            }).await?
            };
            let Some(claim) = claim else { continue };
            let outcome = matrix
                .send(
                    &claim.scope.puppet_mxid,
                    &claim.scope.room_id,
                    &claim.transaction,
                    claim.content,
                )
                .await;
            // A send may complete after revocation. Preserve the observed event
            // under its original transaction instead of claiming it was unsent.
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
                let saved=sql_query("SELECT * FROM hagency_agent_v1.management_shared_replies WHERE work_id=$1").bind::<Text,_>(&claim.work_id).get_result::<Reply>(db).await?;
                if saved.content_digest!=claim.content_digest || saved.matrix_txn_id!=claim.transaction {return Err(Error::Conflict("shared_reply_changed"));}
                match outcome {
                    Ok(event)=>{
                        if saved.matrix_event_id.as_ref().is_some_and(|old|old!=&event) {return Err(Error::Conflict("shared_reply_event_changed"));}
                        let now=clock(db).await?;
                        sql_query("UPDATE hagency_agent_v1.management_shared_replies SET state='sent',matrix_event_id=$2,sent_at_ms=coalesce(sent_at_ms,$3) WHERE work_id=$1").bind::<Text,_>(&claim.work_id).bind::<Text,_>(event).bind::<BigInt,_>(now).execute(db).await?;
                    }
                    Err(e) if saved.state!="sent"=>{
                        let w=work_row(db,&claim.work_id).await?;let r=runtime(db,&w.runtime_id).await?;let now=clock(db).await?;let active=reply_live(db,policy,&r,&self.server,now).await?;
                        sql_query("UPDATE hagency_agent_v1.management_shared_replies SET state=$2 WHERE work_id=$1").bind::<Text,_>(&claim.work_id).bind::<Text,_>(if active && !matches!(e,Error::Conflict("matrix_permission_missing")) {"unknown"}else{"blocked"}).execute(db).await?;
                    }
                    Err(_)=>(),
                }Ok(())
            }).await?;
        }
        Ok(())
    }
}
