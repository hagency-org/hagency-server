//! Versioned shared process/activity authority; personal execution stays separate.
#[path = "management_shared_tickets.rs"]
mod tickets;
use super::*;
#[path = "management_shared_attempts.rs"]
mod attempts;
#[path = "management_shared_budget_attempts.rs"]
mod budget_attempts;
#[path = "management_shared_demands.rs"]
mod demands;
#[path = "management_shared_intake.rs"]
pub(super) mod intake;
#[path = "management_shared_work.rs"]
mod work;
use crate::{
    gateway::Gateway, management_agent_runtime_types::ManagedRuntimeRecoverInput,
    management_shared_execution_types::*, transport::DeliveryFacts,
};
#[derive(QueryableByName)]
struct Id {
    #[diesel(sql_type=Text)]
    id: String,
}
#[derive(QueryableByName)]
struct RuntimeCandidate {
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Nullable<Text>)]
    runtime_id: Option<String>,
}
#[derive(QueryableByName)]
struct Runtime {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Bool)]
    permanent_fence: bool,
    #[diesel(sql_type=Nullable<Text>)]
    process_digest: Option<String>,
    #[diesel(sql_type=BigInt)]
    activity_epoch: i64,
    #[diesel(sql_type=Nullable<Text>)]
    activity_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    activity_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    last_renewal_id: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    last_renewal_epoch: Option<i64>,
}
#[derive(QueryableByName)]
struct Membership {
    #[diesel(sql_type=Text)]
    room_id: String,
    #[diesel(sql_type=Text)]
    space_id: String,
    #[diesel(sql_type=Text)]
    puppet_mxid: String,
    #[diesel(sql_type=Text)]
    manager_mxid: String,
    #[diesel(sql_type=Bool)]
    pool_agent: bool,
}
impl Membership {
    // Any joined Room member may address the shared Agent with an explicit
    // mention. Only the manager of an assignment created directly from a pool
    // is addressed without one, in that assignment's dedicated conversation.
    // Callers also verify the room and fresh Matrix membership of the sender.
    fn addressed_by(&self, sender: &str, mentions: &std::collections::BTreeSet<String>) -> bool {
        (self.pool_agent && self.space_id.is_empty() && sender == self.manager_mxid)
            || mentions.contains(&self.puppet_mxid)
    }
}
fn enabled(p: &ManagementPolicy) -> Result<()> {
    if p.shared_execution() {
        Ok(())
    } else {
        Err(Error::Unavailable("shared_execution_unavailable"))
    }
}
fn issuing(p: &ManagementPolicy) -> bool {
    p.shared_execution()
        && p.agent_matrix_provisioning()
        && p.agent_executor()
        && p.agent_admission()
        && p.project_reservations()
}
async fn runtime_issuing(
    db: &mut AsyncPgConnection,
    policy: &ManagementPolicy,
    r: &Runtime,
) -> Result<bool> {
    Ok(
        (runtime_version(r)? != 3 || policy.shared_budget_execution())
            && demands::allowed(db, policy, r).await?
            && issuing(policy)
            && (policy.shared_successors()
                || serde_json::from_str::<serde_json::Value>(&r.payload)
                    .ok()
                    .and_then(|v| v.get("version").and_then(serde_json::Value::as_u64))
                    == Some(1)),
    )
}
fn runtime_version(r: &Runtime) -> Result<u64> {
    let raw: serde_json::Value = decode(&r.payload)?;
    raw.get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(Error::Conflict("shared_runtime_command_changed"))
}
#[derive(Clone, Copy)]
enum WorkProtocol {
    Legacy,
    Budget,
}
impl WorkProtocol {
    fn check(self, r: &Runtime) -> Result<()> {
        let version = runtime_version(r)?;
        if !matches!((self, version), (Self::Legacy, 1 | 2) | (Self::Budget, 3)) {
            return Err(Error::Conflict("shared_execution_version_required"));
        }
        Ok(())
    }
}
async fn require_budget_head(db: &mut AsyncPgConnection, r: &Runtime, server: &str) -> Result<()> {
    let c = command(r, server)?;
    if let Some(pin) = c.execution_budget {
        let w = original(db, r).await?;
        let (head, _, known) =
            super::super::budget::execution_head(db, &w.request_id, server).await?;
        if pin != head {
            return Err(Error::Conflict("shared_budget_head_changed"));
        }
        if known >= pin.effective_ceiling_tokens {
            return Err(Error::Conflict("shared_budget_exhausted"));
        }
    }
    Ok(())
}
async fn runtime(db: &mut AsyncPgConnection, id: &str) -> Result<Runtime> {
    sql_query("SELECT * FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn original(db: &mut AsyncPgConnection, r: &Runtime) -> Result<Work> {
    sql_query(format!("{WORK} JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=i.request_id WHERE a.id=$1 AND w.operation='agent.reserve'")).bind::<Text,_>(&r.assignment_id).get_result(db).await.map_err(hidden)
}
fn original_command(r: &Runtime, server: &str) -> Result<SharedRuntimeCommand> {
    let c: SharedRuntimeCommand = decode(&r.payload)?;
    c.validate(server)?;
    if c.command_id != r.id
        || c.assignment_id != r.assignment_id
        || allocation_digest(&c)? != r.digest
    {
        return Err(Error::Conflict("shared_runtime_command_changed"));
    }
    Ok(c)
}
// Common authority projection only; never serialized as a v1 command.
struct RuntimeScope {
    reserve_command: ManagedAgentReserveCommand,
    execution_budget: Option<crate::management_shared_budget_types::SharedExecutionBudget>,
    issued_at_ms: i64,
    expires_at_ms: i64,
}
fn command(r: &Runtime, server: &str) -> Result<RuntimeScope> {
    let raw: serde_json::Value = decode(&r.payload)?;
    if raw.get("version").and_then(serde_json::Value::as_u64) == Some(3) {
        let c = budget_attempts::attempt_command(r, server)?;
        Ok(RuntimeScope {
            reserve_command: c.original_runtime_command.reserve_command,
            execution_budget: Some(c.execution_budget),
            issued_at_ms: c.issued_at_ms,
            expires_at_ms: c.expires_at_ms,
        })
    } else if raw.get("version").and_then(serde_json::Value::as_u64) == Some(2) {
        let c = attempts::attempt_command(r, server)?;
        Ok(RuntimeScope {
            reserve_command: c.original_runtime_command.reserve_command,
            execution_budget: None,
            issued_at_ms: c.issued_at_ms,
            expires_at_ms: c.expires_at_ms,
        })
    } else {
        let c = original_command(r, server)?;
        Ok(RuntimeScope {
            reserve_command: c.reserve_command,
            execution_budget: None,
            issued_at_ms: c.issued_at_ms,
            expires_at_ms: c.expires_at_ms,
        })
    }
}
async fn current(
    db: &mut AsyncPgConnection,
    r: &Runtime,
    w: &Work,
    c: &RuntimeScope,
    now: i64,
) -> Result<bool> {
    if !demands::current(db, r, now).await?
        || r.permanent_fence
        || r.state == "stopped"
        || intake::paused(db, &r.assignment_id).await?
        || !live(db, w, &c.reserve_command, now).await?
    {
        return Ok(false);
    }
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id LEFT JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(r.pool_contribution_id,p.contribution_id) WHERE a.id=$1 AND a.state='pending_native' AND c.runtime='codex') AS matched").bind::<Text,_>(&r.assignment_id).get_result::<Flag>(db).await?.matched)
}
async fn membership(db: &mut AsyncPgConnection, r: &Runtime) -> Result<Membership> {
    sql_query("SELECT m.room_id,m.space_id,m.puppet_mxid,u.mxid AS manager_mxid,q.pool_contribution_id IS NOT NULL AS pool_agent FROM hagency_agent_v1.management_agent_matrix m JOIN hagency_agent_v1.management_shared_assignments a ON a.id=m.assignment_id JOIN hagency_agent_v1.management_agent_requests q ON q.id=a.request_id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE m.assignment_id=$1 AND m.state='matrix_verified'").bind::<Text,_>(&r.assignment_id).get_result(db).await.map_err(hidden)
}
fn facts(m: &Membership, f: &DeliveryFacts) -> Result<()> {
    // Gateway stamps its observations with this process clock. Comparing that
    // instant to PostgreSQL's separate host clock can reject fresh facts as
    // future evidence. Database time still independently fences all leases.
    facts_at(m, f, crate::api::now_ms())
}
fn facts_at(m: &Membership, f: &DeliveryFacts, now: i64) -> Result<()> {
    if f.room_id != m.room_id
        || f.space_id != m.space_id
        || f.owner_mxid != m.manager_mxid
        || f.puppet_mxid != m.puppet_mxid
        || f.observed_at_ms > now
        || now - f.observed_at_ms >= MAX_FACT_AGE_MS
        || !f.owner_in_room
        || if m.space_id.is_empty() { !f.owner_direct_valid } else { !f.owner_in_space || !f.room_in_space }
        || !f.requester_in_room
        || !f.puppet_in_room
        || !f.puppet_can_send_message
        || f.encrypted
    {
        return Err(Error::Unauthorized("shared_room_authority_changed"));
    }
    Ok(())
}
async fn latest(
    db: &mut AsyncPgConnection,
    id: &str,
    observation: Option<&str>,
) -> Result<Option<SharedProcessReceipt>> {
    let r=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND ($2::text IS NULL OR id=$2) ORDER BY ledger_revision DESC LIMIT 1").bind::<Text,_>(id).bind::<Nullable<Text>,_>(observation).get_result::<Json>(db).await.optional()?;
    r.map(|v| decode(&v.payload)).transpose()
}
async fn until(db: &mut AsyncPgConnection, p: &Principal, now: i64) -> Result<i64> {
    Ok(sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now)
}
async fn fence_runtime(db: &mut AsyncPgConnection, id: &str) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=true,activity_until_ms=0 WHERE id=$1").bind::<Text,_>(id).execute(db).await?;
    Ok(())
}
async fn delivery(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    r: &Runtime,
    server: &str,
    historical: bool,
    observation: Option<&str>,
) -> Result<SharedRuntimeDelivery> {
    let w = original(db, r).await?;
    owner(&w, p)?;
    let c = original_command(r, server)?;
    let scope = command(r, server)?;
    let now = clock(db).await?;
    let live = current(db, r, &w, &scope, now).await?;
    let expired = r.state == "qualified" && r.activity_until_ms <= now
        || matches!(r.state.as_str(), "pending" | "issued") && c.expires_at_ms <= now;
    if !live || expired {
        fence_runtime(db, &r.id).await?;
    }
    let recorded = latest(db, &r.id, observation).await?;
    let allowed = !historical
        && (!policy.shared_durable_intake() || intake::running(db, &r.assignment_id).await?)
        && issuing(policy)
        && live
        && !expired
        && p.device_generation == Some(w.executor_generation)
        && matches!(r.state.as_str(), "pending" | "issued")
        && recorded.is_none();
    let cleanup = !historical
        && (!live || expired || r.permanent_fence || r.state == "unknown")
        && r.state != "stopped";
    let token = secret_token();
    let until = if allowed {
        until(db, p, now).await?.min(c.expires_at_ms)
    } else {
        until(db, p, now).await?
    };
    tickets::issue(
        db,
        p,
        &r.id,
        &token,
        if allowed {
            "execute"
        } else if cleanup {
            "cleanup"
        } else {
            "observe"
        },
        until,
        now,
    )
    .await?;
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET lease_hash=$2,lease_until_ms=$3,lease_mode=$5,state=CASE WHEN $4 AND state='pending' THEN 'issued' ELSE state END WHERE id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).bind::<Bool,_>(allowed).bind::<Text,_>(if allowed {"execute"}else if cleanup {"cleanup"}else{"observe"}).execute(db).await?;
    Ok(SharedRuntimeDelivery {
        command: c,
        command_digest: r.digest.clone(),
        token,
        lease_until_ms: until,
        execution_allowed: allowed,
        cleanup_allowed: cleanup,
        recorded,
    })
}
fn activity(r: &Runtime, token: String) -> Result<SharedActivityLease> {
    Ok(SharedActivityLease {
        authority: SharedActivityRef {
            runtime_command_id: r.id.clone(),
            epoch: r.activity_epoch,
            token,
        },
        command_digest: r.digest.clone(),
        process_digest: r
            .process_digest
            .clone()
            .ok_or(Error::Conflict("shared_process_required"))?,
        expires_at_ms: r.activity_until_ms,
    })
}
async fn mint_activity(
    db: &mut AsyncPgConnection,
    p: &Principal,
    r: &Runtime,
    c: &RuntimeScope,
    now: i64,
) -> Result<Option<SharedActivityLease>> {
    let w = original(db, r).await?;
    if r.state != "qualified"
        || !current(db, r, &w, c, now).await?
        || p.device_generation != Some(w.executor_generation)
    {
        return Ok(None);
    }
    // Exact receipt replay may recover a current lease, never extend its end.
    let expiry = if r.activity_epoch == 0 {
        until(db, p, now)
            .await?
            .min(c.reserve_command.expires_at_ms)
    } else {
        r.activity_until_ms
    };
    if expiry <= now {
        return Ok(None);
    }
    let token = secret_token();
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET activity_epoch=greatest(activity_epoch,1),activity_hash=$2,activity_until_ms=$3 WHERE id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(expiry).execute(db).await?;
    Ok(Some(activity(&runtime(db, &r.id).await?, token)?))
}
impl DomainStore {
    pub async fn poll_shared_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<SharedRuntimeDelivery>> {
        enabled(policy)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;let now=clock(db).await?;let (device,generation)=device(p)?;sweep(db,p,now).await?;
            // A healthy qualified process has its own activity/work channel;
            // returning it here shadows new agents and needlessly rotates its
            // observation ticket. Discovery covers actionable work only.
            // Resolve an opaque returned command cursor inside this exact owner
            // installation; unrelated or stale cursors cannot select a scope.
            let cursor=sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE r.id=$1 AND a.executor_owner_user_id=$2 AND a.executor_device_id=$3")
                .bind::<Text,_>(input.after.as_deref().unwrap_or(""))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(device)
                .get_result::<Id>(db).await.optional()?.map(|v|v.id).unwrap_or_default();
            // Existing cleanup and fresh issuance share one circular ordering.
            // The first poll favors cleanup; advancing `after` then guarantees
            // that an unresolved process cannot permanently starve its siblings.
            let ids=sql_query("SELECT a.id AS assignment_id,r.id AS runtime_id
                FROM hagency_agent_v1.management_shared_assignments a
                LEFT JOIN hagency_agent_v1.management_shared_runtimes r ON r.assignment_id=a.id AND r.payload::jsonb->>'version'='1'
                LEFT JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
                WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND (
                    (r.id IS NOT NULL AND r.state<>'stopped' AND
                     (r.state<>'qualified' OR r.permanent_fence OR r.activity_until_ms<=$5))
                    OR (r.id IS NULL AND $7 AND a.executor_generation=$4 AND a.state='pending_native'
                        AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls sc WHERE sc.assignment_id=a.id AND sc.mode='paused')
                        AND m.state='matrix_verified' AND m.verified_at_ms>$5-$6 AND m.verified_at_ms<=$5
                        AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands x WHERE x.assignment_id=a.id)
                        AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests ar
                            LEFT JOIN hagency_agent_v1.management_projects p ON p.id=ar.plan_id
                            JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(ar.pool_contribution_id,p.contribution_id)
                            WHERE ar.id=a.request_id AND c.runtime='codex')))
                ORDER BY CASE WHEN $3='' THEN coalesce(r.permanent_fence OR r.state='unknown'
                    OR r.state='qualified' AND r.activity_until_ms<=$5
                    OR r.state IN ('pending','issued') AND r.expires_at_ms<=$5,false) ELSE false END DESC,
                    (a.id<=$3),a.id LIMIT 32")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(&cursor)
                .bind::<BigInt,_>(generation).bind::<BigInt,_>(now).bind::<BigInt,_>(MAX_FACT_AGE_MS)
                .bind::<Bool,_>(issuing(policy)).load::<RuntimeCandidate>(db).await?;
            for id in ids {
                if let Some(id)=id.runtime_id {
                    let r=runtime(db,&id).await?;
                    return Ok(Some(delivery(db,p,policy,&r,&self.server,false,None).await?));
                }
                if policy.shared_durable_intake() && !intake::running(db,&id.assignment_id).await? { continue; }
                let w=sql_query(format!("{WORK} JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=i.request_id WHERE a.id=$1 AND w.operation='agent.reserve'")).bind::<Text,_>(&id.assignment_id).get_result::<Work>(db).await?;
                let reserve:ManagedAgentReserveCommand=decode(&w.payload)?;
                if !live(db,&w,&reserve,now).await? {continue;}
                let codex=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests ar LEFT JOIN hagency_agent_v1.management_projects p ON p.id=ar.plan_id JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(ar.pool_contribution_id,p.contribution_id) WHERE ar.id=$1 AND c.runtime='codex') AS matched").bind::<Text,_>(&reserve.request_id).get_result::<Flag>(db).await?.matched;
                if !codex {continue;}
                let receipt:ManagedAgentReservationReceipt=decode(w.receipt.as_deref().ok_or(Error::Conflict("agent_reservation_required"))?)?;
                let c=SharedRuntimeCommand {version:1,capability:SHARED_EXECUTION_CAPABILITY.into(),operation:"agent.execution.provision".into(),command_id:format!("sex_{}",entity_id()?),assignment_id:id.assignment_id,assignment_generation:1,agent_reservation_id:receipt.agent_reservation_id.ok_or(Error::Conflict("agent_reservation_required"))?,reserve_command_digest:w.digest,reserve_receipt_digest:w.receipt_digest.ok_or(Error::Conflict("agent_reservation_required"))?,policy:SHARED_TEXT_POLICY.into(),issued_at_ms:now,expires_at_ms:(now+30000).min(reserve.expires_at_ms),reserve_command:reserve};c.validate(&self.server)?;
                let digest=allocation_digest(&c)?;
                sql_query("INSERT INTO hagency_agent_v1.management_shared_runtimes(id,assignment_id,payload,digest,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id).bind::<Text,_>(wire(&c)?).bind::<Text,_>(&digest).bind::<BigInt,_>(now).bind::<BigInt,_>(c.expires_at_ms).execute(db).await?;
                let r=runtime(db,&c.command_id).await?;return Ok(Some(delivery(db,p,policy,&r,&self.server,false,None).await?));
            } Ok(None)
        }).await
    }
    pub async fn recover_shared_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ManagedRuntimeRecoverInput,
    ) -> Result<SharedRuntimeDelivery> {
        enabled(policy)?;
        key(id)?;
        if !crate::management_allocation_types::valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_shared_execution"));
        }
        if let Some(v) = &input.observation_id {
            key(v)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let r = runtime(db, id).await?;
                owner(&original(db, &r).await?, p)?;
                if r.digest != input.command_digest {
                    return Err(Error::Conflict("shared_runtime_command_changed"));
                }
                delivery(
                    db,
                    p,
                    policy,
                    &r,
                    &self.server,
                    true,
                    input.observation_id.as_deref(),
                )
                .await
            })
            .await
    }
    pub async fn record_shared_process(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedProcessInput,
    ) -> Result<SharedRuntimeAcknowledgement> {
        enabled(policy)?;
        key(id)?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;let r=runtime(db,id).await?;let w=original(db,&r).await?;owner(&w,p)?;let original=original_command(&r,&self.server)?;input.receipt.validate_for(&original)?;let c=command(&r,&self.server)?;
            let now=clock(db).await?;
            if input.command_digest!=r.digest {return Err(Error::Conflict("shared_result_ticket_changed"));}
            let ticket_mode=tickets::mode(db,p,id,&input.token,now).await?;
            let digest=allocation_digest(&input.receipt)?;
            let old=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_process_receipts WHERE id=$1").bind::<Text,_>(&input.receipt.observation_id).get_result::<Json>(db).await.optional()?;
            if let Some(old)=old {
                let old:SharedProcessReceipt=decode(&old.payload)?;
                if allocation_digest(&old)?!=digest {return Err(Error::Conflict("shared_process_receipt_changed"));}
            } else {
                validate_receipt(db,&r,&w,&input.receipt).await?;
                sql_query("INSERT INTO hagency_agent_v1.management_shared_process_receipts VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&input.receipt.observation_id).bind::<Text,_>(id).bind::<Text,_>(wire(&input.receipt)?).bind::<Text,_>(&digest).bind::<BigInt,_>(input.receipt.ledger_revision).execute(db).await?;
            }
            let fresh=latest(db,id,None).await?.unwrap();
                if fresh.observation_id==input.receipt.observation_id {
                    let live=current(db,&r,&w,&c,now).await?;
                    let pending=input.receipt.state=="qualified"&&ticket_mode!="execute"&&r.activity_epoch==0&&!r.permanent_fence&&now<c.expires_at_ms&&live&&tickets::has_execute(db,p,id,now).await?;
                    let fenced=r.permanent_fence||!pending&&(ticket_mode!="execute"&&r.activity_epoch==0||input.receipt.state!="qualified"||now>=c.expires_at_ms&&r.activity_epoch==0||!live);
                    let state=if pending {&r.state}else{&input.receipt.state};
                    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET state=$2,process_digest=coalesce(process_digest,$3),permanent_fence=permanent_fence OR $4,activity_until_ms=CASE WHEN $4 THEN 0 ELSE activity_until_ms END WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(state).bind::<Nullable<Text>,_>(&input.receipt.process_digest).bind::<Bool,_>(fenced).execute(db).await?;
                }
            let r=runtime(db,id).await?;
            let activity=if runtime_issuing(db,policy,&r).await?&&input.receipt.state=="qualified"&&ticket_mode=="execute" {let current=clock(db).await?;mint_activity(db,p,&r,&c,current).await?}else{None};
            Ok(SharedRuntimeAcknowledgement {observation_id:input.receipt.observation_id,receipt_digest:digest,activity,state:if r.permanent_fence&&r.state!="stopped" {"cleanup_pending".into()}else{r.state}})
        }).await
    }
    pub async fn renew_shared_activity(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedActivityRenewInput,
        gateway: &Gateway,
    ) -> Result<SharedActivityLease> {
        self.renew_shared_activity_with_protocol(p, policy, input, gateway, WorkProtocol::Legacy)
            .await
    }
    pub async fn renew_shared_budget_activity(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedActivityRenewInput,
        gateway: &Gateway,
    ) -> Result<SharedActivityLease> {
        self.renew_shared_activity_with_protocol(p, policy, input, gateway, WorkProtocol::Budget)
            .await
    }
    async fn renew_shared_activity_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedActivityRenewInput,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<SharedActivityLease> {
        enabled(policy)?;
        input.authority.validate()?;
        key(&input.renewal_id)?;
        if !issuing(policy) {
            return Err(Error::Unavailable("shared_execution_issuance_unavailable"));
        }
        let m = {
            let mut guard = self.db.lock().await?;
            (*guard)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, 0).await?;
                    let r = runtime(db, &input.authority.runtime_command_id).await?;
                    protocol.check(&r)?;
                    if !runtime_issuing(db, policy, &r).await? {
                        return Err(Error::Unavailable("shared_successor_issuance_unavailable"));
                    }
                    let w = original(db, &r).await?;
                    owner(&w, p)?;
                    let c = command(&r, &self.server)?;
                    let now = clock(db).await?;
                    check_activity(db, p, &r, &w, &c, &input, now).await?;
                    membership(db, &r).await
                })
                .await?
        };
        let f = gateway
            .shared_delivery(
                &m.room_id,
                &m.space_id,
                &m.manager_mxid,
                &m.manager_mxid,
                &m.puppet_mxid,
            )
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;let r=runtime(db,&input.authority.runtime_command_id).await?;protocol.check(&r)?;if !runtime_issuing(db,policy,&r).await?{return Err(Error::Unavailable("shared_successor_issuance_unavailable"));}let w=original(db,&r).await?;let c=command(&r,&self.server)?;let now=clock(db).await?;
            check_activity(db,p,&r,&w,&c,&input,now).await?;facts(&membership(db,&r).await?,&f)?;
            if r.last_renewal_id.as_deref()==Some(&input.renewal_id)&&r.last_renewal_epoch==Some(input.authority.epoch) {return activity(&r,input.authority.token);}
            let reused=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_activity_renewals WHERE id=$1) AS matched").bind::<Text,_>(&input.renewal_id).get_result::<Flag>(db).await?.matched;
            if reused {return Err(Error::Conflict("shared_activity_renewal_changed"));}
            let expiry=until(db,p,now).await?.min(c.reserve_command.expires_at_ms);
            if r.activity_epoch>=MAX_EXACT {return Err(Error::Conflict("shared_activity_epoch_exhausted"));}
            sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET activity_epoch=activity_epoch+1,activity_until_ms=$2,last_renewal_id=$3,last_renewal_epoch=$4 WHERE id=$1").bind::<Text,_>(&r.id).bind::<BigInt,_>(expiry).bind::<Text,_>(&input.renewal_id).bind::<BigInt,_>(input.authority.epoch).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_shared_activity_renewals(id,runtime_id,from_epoch,to_epoch,expires_at_ms,authority_hash) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&input.renewal_id).bind::<Text,_>(&r.id).bind::<BigInt,_>(input.authority.epoch).bind::<BigInt,_>(input.authority.epoch+1).bind::<BigInt,_>(expiry).bind::<Text,_>(hash(&input.authority.token)).execute(db).await?;
            activity(&runtime(db,&r.id).await?,input.authority.token)
        }).await
    }
    pub async fn shared_execution_status(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request: &str,
    ) -> Result<Option<SharedExecutionStatus>> {
        enabled(policy)?;
        key(request)?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;let request=super::super::row(db,request).await?;
            super::super::visible(db,&request,p).await?;
            let id=sql_query("SELECT r.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.request_id=$1 ORDER BY coalesce((r.payload::jsonb->>'processAttempt')::bigint,1) DESC LIMIT 1").bind::<Text,_>(&request.id).get_result::<Id>(db).await.optional()?;
            let Some(id)=id else{return Ok(None)};let r=runtime(db,&id.id).await?;let now=clock(db).await?;
            let w=original(db,&r).await?;let c=command(&r,&self.server)?;let live=current(db,&r,&w,&c,now).await?;
            let state=if r.state=="stopped" {"stopped"}else if !live||r.permanent_fence||r.state=="qualified"&&r.activity_until_ms<=now {"cleanup_pending"}else if r.state=="unknown" {"outcome_unknown"}else{&r.state};
            let spendable=runtime_version(&r)?!=3 || budget_attempts::head_matches(db,&r,&self.server).await?;
            Ok(Some(SharedExecutionStatus {assignment_id:r.assignment_id.clone(),state:state.into(),policy:SHARED_TEXT_POLICY.into(),activity_expires_at_ms:(r.activity_until_ms>0).then_some(r.activity_until_ms),work_available:runtime_issuing(db,policy,&r).await?&&live&&spendable&&r.state=="qualified"&&r.activity_until_ms>now,capacity_released:false,recent_work:vec![]}))
        }).await
    }
}
async fn check_activity(
    db: &mut AsyncPgConnection,
    p: &Principal,
    r: &Runtime,
    w: &Work,
    c: &RuntimeScope,
    input: &SharedActivityRenewInput,
    now: i64,
) -> Result<()> {
    let matches = r.activity_epoch == input.authority.epoch
        || r.last_renewal_id.as_deref() == Some(&input.renewal_id)
            && r.last_renewal_epoch == Some(input.authority.epoch)
            && r.activity_epoch == input.authority.epoch + 1;
    if r.state != "qualified"
        || r.activity_until_ms <= now
        || !matches
        || r.activity_hash.as_deref() != Some(hash(&input.authority.token).as_str())
        || p.device_generation != Some(w.executor_generation)
        || !current(db, r, w, c, now).await?
    {
        return Err(Error::Conflict("shared_activity_expired_or_changed"));
    }
    Ok(())
}
async fn validate_receipt(
    db: &mut AsyncPgConnection,
    r: &Runtime,
    w: &Work,
    v: &SharedProcessReceipt,
) -> Result<()> {
    let base: ManagedAgentReservationReceipt = decode(
        w.receipt
            .as_deref()
            .ok_or(Error::Conflict("agent_reservation_required"))?,
    )?;
    if v.ledger_revision <= base.ledger_revision
        || r.process_digest
            .as_ref()
            .is_some_and(|d| v.process_digest.as_ref().is_some_and(|v| v != d))
    {
        return Err(Error::Conflict("shared_process_evidence_changed"));
    }
    super::validate_external_ledger_receipt(
        db,
        &w.executor_owner_user_id,
        &w.executor_device_id,
        v.ledger_revision,
        v.ledger_revision,
    )
    .await?;
    let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3 UNION ALL SELECT 1 FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND ((receipt::jsonb->>'ledgerRevision')::bigint=$3 OR (receipt::jsonb->>'fenceRevision')::bigint=$3) UNION ALL SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3) AS matched").bind::<Text,_>(&w.executor_owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(v.ledger_revision).get_result::<Flag>(db).await?.matched;
    if collision {
        return Err(Error::Conflict("shared_process_evidence_changed"));
    }
    let old=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 ORDER BY ledger_revision DESC LIMIT 256").bind::<Text,_>(&r.id).load::<Json>(db).await?;
    if old.len() >= 256 {
        return Err(Error::Conflict("shared_process_observation_limit"));
    }
    for old in old {
        let old: SharedProcessReceipt = decode(&old.payload)?;
        if old.ledger_revision == v.ledger_revision
            || old.state == "stopped"
                && v.state == "qualified"
                && v.ledger_revision > old.ledger_revision
            || v.state == "stopped"
                && old.state == "qualified"
                && old.ledger_revision > v.ledger_revision
        {
            return Err(Error::Conflict("shared_process_cannot_restart"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    #[test]
    fn matrix_observation_freshness_uses_its_host_clock_without_accepting_stale_or_future_facts() {
        let m = Membership {
            room_id: "!r:example.test".into(),
            space_id: "!s:example.test".into(),
            puppet_mxid: "@_hagency_a:example.test".into(),
            manager_mxid: "@manager:example.test".into(),
            pool_agent: false,
        };
        let mut f = DeliveryFacts {
            owner_mxid: m.manager_mxid.clone(),
            requester_mxid: m.manager_mxid.clone(),
            puppet_mxid: m.puppet_mxid.clone(),
            room_id: m.room_id.clone(),
            space_id: m.space_id.clone(),
            observed_at_ms: 100_000,
            owner_in_space: true,
            owner_in_room: true,
            room_in_space: true,
            requester_in_room: true,
            puppet_in_room: true,
            puppet_can_send_message: true,
            encrypted: false,
            owner_direct_valid: false,
        };
        // Separate database clock can be behind the process by one millisecond;
        // it is not the time source of this observation and must not judge age.
        assert!(facts_at(&m, &f, 99_999).is_err());
        assert!(facts_at(&m, &f, 100_001).is_ok());
        assert!(facts_at(&m, &f, 100_000 + MAX_FACT_AGE_MS).is_err());
        f.puppet_can_send_message = false;
        assert!(facts_at(&m, &f, 100_001).is_err());
    }
    #[test]
    fn joined_room_members_address_the_shared_agent_by_mention() {
        let mut m = Membership {
            room_id: "!r:example.test".into(),
            space_id: String::new(),
            puppet_mxid: "@_hagency_a:example.test".into(),
            manager_mxid: "@manager:example.test".into(),
            pool_agent: true,
        };
        let none = std::collections::BTreeSet::new();
        let mention = std::collections::BTreeSet::from([m.puppet_mxid.clone()]);
        // A pool conversation needs no mention from its manager, only from others.
        assert!(m.addressed_by("@manager:example.test", &none));
        assert!(!m.addressed_by("@member:example.test", &none));
        assert!(m.addressed_by("@member:example.test", &mention));
        m.space_id = "!s:example.test".into();
        assert!(!m.addressed_by("@manager:example.test", &none));
        assert!(m.addressed_by("@member:example.test", &mention));
        m.space_id.clear();
        let mut f = DeliveryFacts {
            owner_mxid: m.manager_mxid.clone(),
            requester_mxid: "@member:example.test".into(),
            puppet_mxid: m.puppet_mxid.clone(),
            room_id: m.room_id.clone(),
            space_id: String::new(),
            observed_at_ms: 100_000,
            owner_in_space: false,
            owner_in_room: true,
            room_in_space: false,
            requester_in_room: true,
            puppet_in_room: true,
            puppet_can_send_message: true,
            encrypted: false,
            owner_direct_valid: true,
        };
        assert!(facts_at(&m, &f, 100_001).is_ok());
        f.requester_in_room = false;
        assert!(facts_at(&m, &f, 100_001).is_err());
        f.requester_in_room = true;
        f.owner_in_room = false;
        assert!(facts_at(&m, &f, 100_001).is_err());
    }
}
