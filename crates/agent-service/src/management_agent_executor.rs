//! Exact enrolled-executor admission delivery. No work lease or runtime exists.
use super::*;
#[path = "management_agent_matrix.rs"]
mod matrix;
#[path = "management_agent_runtime.rs"]
mod runtime;
#[path = "management_shared_execution.rs"]
mod shared;
use crate::{
    management_agent_executor_types::*,
    management_allocation_types::{ExecutorPollInput, ExecutorRecoverInput, MAX_EXACT},
    secret_token,
};

const WORK: &str = "SELECT w.*,i.request_id,i.executor_owner_user_id,i.executor_device_id,i.executor_generation,i.payload,i.digest,i.state AS intent_state FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id";
#[derive(QueryableByName)]
struct Work {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    intent_id: String,
    #[diesel(sql_type=Text)]
    operation: String,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    receipt: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    receipt_digest: Option<String>,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    executor_owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    intent_state: String,
}
#[derive(QueryableByName)]
struct Json {
    #[diesel(sql_type=Text)]
    payload: String,
}
fn enabled_executor(policy: &ManagementPolicy) -> Result<()> {
    if !policy.agent_executor() {
        return Err(Error::Unavailable("agent_executor_unavailable"));
    }
    Ok(())
}
async fn work(db: &mut AsyncPgConnection, id: &str) -> Result<Work> {
    sql_query(format!("{WORK} WHERE w.id=$1"))
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn owner(w: &Work, p: &Principal) -> Result<()> {
    let (id, generation) = device(p)?;
    if w.executor_owner_user_id != p.user_id
        || w.executor_device_id != id
        || generation < w.executor_generation
    {
        return Err(Error::NotFound("agent_executor_command_not_found"));
    }
    Ok(())
}
fn command(w: &Work, server: &str) -> Result<(AgentExecutorCommand, String)> {
    let reserve: ManagedAgentReserveCommand = decode(&w.payload)?;
    reserve.validate(server)?;
    if allocation_digest(&reserve)? != w.digest
        || reserve.command_id != w.intent_id
        || reserve.request_id != w.request_id
    {
        return Err(Error::Conflict("agent_executor_command_conflict"));
    }
    let command = if w.operation == "agent.reserve" {
        AgentExecutorCommand::Reserve(reserve)
    } else {
        let c = ManagedAgentFenceCommand {
            version: 1,
            capability: AGENT_FENCE_CAPABILITY.into(),
            operation: "agent.fence".into(),
            command_id: w.id.clone(),
            reserve_command: reserve,
            reserve_command_digest: w.digest.clone(),
            issued_at_ms: w.created_at_ms,
        };
        c.validate(server)?;
        AgentExecutorCommand::Fence(c)
    };
    wire(&command)?;
    let digest = allocation_digest(&command)?;
    Ok((command, digest))
}
async fn live(
    db: &mut AsyncPgConnection,
    w: &Work,
    c: &ManagedAgentReserveCommand,
    now: i64,
) -> Result<bool> {
    if w.intent_state == "fenced" || now >= c.expires_at_ms {
        return Ok(false);
    }
    let r = row(db, &w.request_id).await?;
    if r.state != "approved_waiting_executor"
        || r.revision != c.request_revision
        || r.authority_revision != c.authority_revision
    {
        return Ok(false);
    }
    // A renewable owner login expiring is an availability failure, not an
    // irreversible withdrawal of the approved reservation. No work may run
    // until the same device obtains fresh matching upstream proof.
    if session_temporarily_expired(db, w, now).await? {
        return Err(Error::Unavailable("agent_executor_session_expired"));
    }
    if r.pool_contribution_id.is_some() {
        return super::pool::command_live(db, &r, c, now).await;
    }
    let cx = match context(db, &r.plan_id, now).await {
        Ok(c) => c,
        Err(Error::Conflict(_) | Error::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    let b = cx.setup.current_binding.as_ref().unwrap();
    Ok(cx.matches(&r)
        && b.revision == c.setup_binding_revision
        && b.authority_revision == c.authority_revision
        && cx.roles.administrator_user_id == r.administrator_user_id
        && cx.roles.administrator_mxid == c.decision_actor_mxid
        && cx.roles.manager_mxid == c.manager_mxid
        && cx.owner == w.executor_owner_user_id
        && cx.capacity.grant_digest == c.grant_receipt_digest
        && cx.capacity.grant.enrollment_id == c.enrollment_id
        && cx.capacity.grant.executor_device_id == w.executor_device_id
        && cx.capacity.grant.executor_generation == w.executor_generation
        && cx.capacity.grant.resource_revision == c.resource_revision)
}
async fn session_temporarily_expired(db: &mut AsyncPgConnection, w: &Work, now: i64) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id WHERE d.id=$1 AND d.generation=$2 AND NOT d.revoked AND NOT s.revoked AND u.active AND s.valid_until_ms<=$3) AS matched")
        .bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(w.executor_generation).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
async fn fence(db: &mut AsyncPgConnection, intent: &str) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE id=$1 AND state<>'fenced'").bind::<Text,_>(intent).execute(db).await?;
    Ok(())
}
const POOL_SWEEP: &str = "UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE id IN (SELECT i.id FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.management_agent_requests r ON r.id=i.request_id AND r.pool_grant_id IS NOT NULL JOIN hagency_agent_v1.management_resource_grants c ON c.id=i.payload::jsonb->'poolGrant'->>'grantId' JOIN hagency_agent_v1.devices d ON d.id=i.executor_device_id JOIN hagency_agent_v1.sessions ds ON ds.id=d.session_id LEFT JOIN LATERAL (SELECT g.id FROM hagency_agent_v1.management_resource_grants g WHERE g.contribution_id=r.pool_contribution_id AND g.state='active' AND g.period_start_ms<=$3 AND g.expires_at_ms>$3 ORDER BY g.id LIMIT 1) e ON true WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND i.state<>'fenced' AND (r.state<>'approved_waiting_executor' OR (i.payload::jsonb->>'expiresAtMs')::bigint<=$3 OR e.id IS NULL OR d.generation<>i.executor_generation OR d.revoked OR ds.revoked OR (e.id<>c.id AND NOT (EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands w WHERE w.intent_id=i.id AND w.operation='agent.reserve' AND w.receipt::jsonb->>'state'='reserved') AND EXISTS(WITH RECURSIVE up(id,predecessor,depth) AS (SELECT u.id,u.predecessor_grant_id,0 FROM hagency_agent_v1.management_resource_grants u WHERE u.id=e.id UNION ALL SELECT u.id,u.predecessor_grant_id,up.depth+1 FROM hagency_agent_v1.management_resource_grants u JOIN up ON u.id=up.predecessor WHERE up.depth<4096) SELECT 1 FROM up WHERE up.id=c.id)))) ORDER BY i.id LIMIT 64)";
async fn sweep(db: &mut AsyncPgConnection, p: &Principal, now: i64) -> Result<()> {
    // Pool intents follow their pool's chain. `c` is the command's own grant and
    // `e` the pool's effective grant: a reserved Agent stays while `c` is `e` or
    // one of its predecessors; an intent that never reserved needs `c` = `e`.
    sql_query(POOL_SWEEP).bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(p.device_id.as_deref()).bind::<BigInt,_>(now).execute(db).await?;

    // Bounded expired/stale candidates only: a page of live assignments cannot
    // starve cleanup for a later expired one. No network observation is invented.
    sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE id IN (SELECT i.id FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.management_agent_requests r ON r.id=i.request_id JOIN hagency_agent_v1.management_allocations a ON a.id=r.allocation_id JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id JOIN hagency_agent_v1.management_project_setups s ON s.id=r.setup_id JOIN hagency_agent_v1.management_project_authorities pa ON pa.plan_id=r.plan_id JOIN hagency_agent_v1.devices d ON d.id=i.executor_device_id JOIN hagency_agent_v1.sessions ds ON ds.id=d.session_id WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND i.state<>'fenced' AND (r.state<>'approved_waiting_executor' OR (i.payload::jsonb->>'expiresAtMs')::bigint<=$3 OR a.state<>'reserved' OR a.expires_at_ms<=$3 OR a.release_command_id IS NOT NULL OR g.state<>'active' OR g.expires_at_ms<=$3 OR s.state<>'ready' OR s.current_binding_revision<>(i.payload::jsonb->>'setupBindingRevision')::bigint OR pa.revision<>(i.payload::jsonb->>'authorityRevision')::bigint OR d.generation<>i.executor_generation OR d.revoked OR ds.revoked) ORDER BY i.id LIMIT 64)").bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(p.device_id.as_deref()).bind::<BigInt,_>(now).execute(db).await?;
    Ok(())
}
async fn projection(
    db: &mut AsyncPgConnection,
    request: &str,
) -> Result<Option<PendingManagedAssignment>> {
    let r=sql_query("SELECT json_build_object('id',a.id,'requestId',a.request_id,'agentId',a.agent_id,'managerMxid',u.mxid,'executorDeviceId',a.executor_device_id,'executorGeneration',a.executor_generation,'agentReservationId',a.agent_reservation_id,'reserveReceiptDigest',a.reserve_receipt_digest,'state',a.state,'generation',a.generation,'executionAvailable',false)::text AS payload FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE a.request_id=$1").bind::<Text,_>(request).get_result::<Json>(db).await.optional()?;
    r.map(|r| decode(&r.payload)).transpose()
}
// A native owner ledger assigns one mutation revision to one command. Existing
// project/grant protocols call this before accepting a later-arriving receipt;
// their wire format and historical receipt replay are unchanged.
pub(in crate::domain::management) async fn validate_external_ledger_receipt(
    db: &mut AsyncPgConnection,
    owner: &str,
    device: &str,
    revision: i64,
    fence_revision: i64,
) -> Result<()> {
    let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND w.receipt IS NOT NULL AND ((w.receipt::jsonb->>'ledgerRevision')::bigint IN ($3,$4) OR (w.receipt::jsonb->>'fenceRevision')::bigint IN ($3,$4)) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=o.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_work_receipts o JOIN hagency_agent_v1.management_shared_work w ON w.id=o.work_id JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_budget_results o JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=o.command_id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results o JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=o.command_id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND o.ledger_revision IN ($3,$4)) AS matched").bind::<Text,_>(owner).bind::<Text,_>(device).bind::<BigInt,_>(revision).bind::<BigInt,_>(fence_revision).get_result::<Flag>(db).await?.matched;
    if collision {
        return Err(Error::Conflict("agent_executor_ledger_revision_changed"));
    }
    Ok(())
}
async fn deliver(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    w: &Work,
    server: &str,
    historical: bool,
) -> Result<AgentExecutorDelivery> {
    owner(w, p)?;
    let (c, digest) = command(w, server)?;
    let now = clock(db).await?;
    let current = match &c {
        AgentExecutorCommand::Reserve(c) => live(db, w, c, now).await?,
        AgentExecutorCommand::Fence(_) => true,
    };
    if !current {
        fence(db, &w.intent_id).await?;
    }
    let executable = !historical
        && w.receipt.is_none()
        && current
        && (w.operation == "agent.fence"
            || policy.agent_admission()
                && policy.project_reservations()
                && p.device_generation == Some(w.executor_generation));
    let mut until = sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now;
    if executable && let AgentExecutorCommand::Reserve(c) = &c {
        until = until.min(c.expires_at_ms);
    }
    let token = secret_token();
    sql_query("UPDATE hagency_agent_v1.management_agent_executor_commands SET state=CASE WHEN state='pending' THEN 'issued' ELSE state END,lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(&w.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).execute(db).await?;
    DomainStore::authorize(db, p, crate::api::now_ms()).await?;
    Ok(AgentExecutorDelivery {
        command: c,
        command_digest: digest,
        token,
        lease_until_ms: until,
        execution_allowed: executable,
        recorded: w.receipt.as_deref().map(decode).transpose()?,
    })
}
async fn validate_result(
    db: &mut AsyncPgConnection,
    w: &Work,
    c: &AgentExecutorCommand,
    r: &AgentExecutorReceipt,
) -> Result<()> {
    match (c, r) {
        (AgentExecutorCommand::Reserve(c), AgentExecutorReceipt::Reserve(r)) => {
            r.validate_for(c)?
        }
        (AgentExecutorCommand::Fence(c), AgentExecutorReceipt::Fence(r)) => r.validate_for(c)?,
        _ => return Err(Error::Invalid("invalid_agent_executor_result")),
    }
    if let AgentExecutorReceipt::Fence(receipt) = r
        && receipt.known_consumed_tokens
            < super::super::usage::known(db, super::super::usage::Scope::Agent(&w.request_id))
                .await?
    {
        return Err(Error::Conflict("agent_executor_usage_changed"));
    }
    let original: ManagedAgentReserveCommand = decode(&w.payload)?;
    // A direct Agent mutates its exact parent grant; legacy children mutate
    // their acknowledged project hold. Neither may reuse a prior ledger revision.
    let base_revision=if let Some(g)=&original.pool_grant {g.ledger_revision} else {
        let base=sql_query("SELECT receipt AS payload FROM hagency_agent_v1.management_executor_commands WHERE allocation_id=$1 AND operation='project.reserve' AND receipt_digest=$2").bind::<Text,_>(&original.allocation_id).bind::<Text,_>(&original.allocation_receipt_digest).get_result::<Json>(db).await.map_err(hidden)?;
        decode::<crate::management_allocation_types::NativeAllocationReceipt>(&base.payload)?.ledger_revision
    };
    let fence_revision = match r {
        AgentExecutorReceipt::Reserve(r) => r.ledger_revision,
        AgentExecutorReceipt::Fence(r) => r.fence_revision,
    };
    if r.revision() <= base_revision
        || r.revision() > MAX_EXACT
        || fence_revision <= base_revision
    {
        return Err(Error::Conflict("agent_executor_ledger_revision_changed"));
    }
    let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND ((receipt::jsonb->>'ledgerRevision')::bigint IN ($3,$4) OR (receipt::jsonb->>'fenceRevision')::bigint IN ($3,$4)) UNION ALL SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=o.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_work_receipts o JOIN hagency_agent_v1.management_shared_work w ON w.id=o.work_id JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_budget_results o JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=o.command_id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND o.ledger_revision IN ($3,$4) UNION ALL SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results o JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=o.command_id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND o.ledger_revision IN ($3,$4)) AS matched").bind::<Text,_>(&w.executor_owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(r.revision()).bind::<BigInt,_>(fence_revision).get_result::<Flag>(db).await?.matched;
    if collision {
        return Err(Error::Conflict("agent_executor_ledger_revision_changed"));
    }
    let child = match r {
        AgentExecutorReceipt::Reserve(r) => r.agent_reservation_id.as_deref(),
        AgentExecutorReceipt::Fence(r) => r.agent_reservation_id.as_deref(),
    };
    let others=sql_query(format!("{WORK} WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND w.receipt IS NOT NULL AND w.id<>$3 AND ((w.receipt::jsonb->>'ledgerRevision')::bigint IN ($4,$7) OR (w.receipt::jsonb->>'fenceRevision')::bigint IN ($4,$7) OR w.intent_id=$5 OR w.receipt::jsonb->>'agentReservationId'=$6) ORDER BY w.id LIMIT 4")).bind::<Text,_>(&w.executor_owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<Text,_>(&w.id).bind::<BigInt,_>(r.revision()).bind::<Text,_>(&w.intent_id).bind::<Nullable<Text>,_>(child).bind::<BigInt,_>(fence_revision).load::<Work>(db).await?;
    for other in others {
        let old: AgentExecutorReceipt = decode(other.receipt.as_ref().unwrap())?;
        let old_fence = match &old {
            AgentExecutorReceipt::Reserve(r) => r.ledger_revision,
            AgentExecutorReceipt::Fence(r) => r.fence_revision,
        };
        if [old.revision(), old_fence]
            .iter()
            .any(|n| [r.revision(), fence_revision].contains(n))
        {
            return Err(Error::Conflict("agent_executor_ledger_revision_changed"));
        }
        let old_child = match &old {
            AgentExecutorReceipt::Reserve(r) => r.agent_reservation_id.as_deref(),
            AgentExecutorReceipt::Fence(r) => r.agent_reservation_id.as_deref(),
        };
        if other.intent_id != w.intent_id && child.is_some() && old_child == child {
            return Err(Error::Conflict("agent_executor_child_reused"));
        }
        if other.intent_id == w.intent_id {
            let pair = match (&old, r) {
                (AgentExecutorReceipt::Reserve(a), AgentExecutorReceipt::Fence(b)) => Some((a, b)),
                (AgentExecutorReceipt::Fence(b), AgentExecutorReceipt::Reserve(a)) => Some((a, b)),
                _ => None,
            };
            if let Some((reserve, fence)) = pair
                && reserve.state == "reserved"
                && (fence.state != "fenced"
                    || reserve.agent_reservation_id != fence.agent_reservation_id
                    || reserve.ledger_revision >= fence.fence_revision)
            {
                return Err(Error::Conflict("agent_executor_fence_conflict"));
            }
        }
    }
    Ok(())
}
impl DomainStore {
    pub async fn poll_managed_agent_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<AgentExecutorDelivery>> {
        enabled_executor(policy)?;
        let (device, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;sweep(db,p,now).await?;
   let w=sql_query(format!("{WORK} WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND w.receipt IS NULL AND w.lease_until_ms<=$3 ORDER BY (w.operation='agent.fence') DESC,(w.id<=$4),w.id LIMIT 1")).bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<BigInt,_>(now).bind::<Text,_>(input.after.as_deref().unwrap_or("")).get_result::<Work>(db).await.optional()?;
   match w {Some(w)=>Ok(Some(deliver(db,p,policy,&w,&self.server,false).await?)),None=>Ok(None)}
  }).await
    }
    pub async fn recover_managed_agent_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ExecutorRecoverInput,
    ) -> Result<AgentExecutorDelivery> {
        enabled_executor(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let w = work(db, id).await?;
                owner(&w, p)?;
                if command(&w, &self.server)?.1 != input.command_digest {
                    return Err(Error::Conflict("agent_executor_command_conflict"));
                }
                deliver(db, p, policy, &w, &self.server, true).await
            })
            .await
    }
    pub async fn record_managed_agent_executor_result(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: AgentExecutorResultInput,
    ) -> Result<AgentExecutorAcknowledgement> {
        enabled_executor(policy)?;
        key(id)?;
        input.validate()?;
        wire(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let w=work(db,id).await?;owner(&w,p)?;
   let (c,digest)=command(&w,&self.server)?;if digest!=input.command_digest{return Err(Error::Conflict("agent_executor_command_conflict"));}
   let receipt_digest=allocation_digest(&input.receipt)?;
   if let Some(saved)=&w.receipt_digest {
    if saved!=&receipt_digest{return Err(Error::Conflict("agent_executor_result_conflict"));}
   }else{
    if w.lease_until_ms<=now || w.lease_hash.as_deref()!=Some(hash(&input.token).as_str()){return Err(Error::Unauthorized("agent_executor_ticket_required"));}
    validate_result(db,&w,&c,&input.receipt).await?;
    sql_query("UPDATE hagency_agent_v1.management_agent_executor_commands SET state='recorded',receipt=$2,receipt_digest=$3 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(wire(&input.receipt)?).bind::<Text,_>(&receipt_digest).execute(db).await?;
    if let (AgentExecutorCommand::Reserve(c),AgentExecutorReceipt::Reserve(r))=(&c,&input.receipt) && r.state=="reserved" {
     if live(db,&w,c,now).await? {
      let request=row(db,&w.request_id).await?;
      let assignment=format!("mas_{}",entity_id()?);
      sql_query("INSERT INTO hagency_agent_v1.management_shared_assignments(id,request_id,agent_id,manager_user_id,executor_owner_user_id,executor_device_id,executor_generation,agent_reservation_id,reserve_receipt_digest,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending_native')").bind::<Text,_>(&assignment).bind::<Text,_>(&w.request_id).bind::<Text,_>(&c.proposed_agent_id).bind::<Text,_>(&request.manager_user_id).bind::<Text,_>(&w.executor_owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(w.executor_generation).bind::<Text,_>(r.agent_reservation_id.as_ref().unwrap()).bind::<Text,_>(&receipt_digest).execute(db).await?;
      shared::intake::initialize(db,&assignment,policy,now).await?;
      let mxid=format!("@{}{}:{}",self.namespace,c.proposed_agent_id,self.server);
      sql_query("INSERT INTO hagency_agent_v1.agents(id,owner_user_id,puppet_mxid,display_name,state,execution_kind) VALUES($1,$2,$3,$4,'suspended','shared_project')").bind::<Text,_>(&c.proposed_agent_id).bind::<Text,_>(&request.manager_user_id).bind::<Text,_>(&mxid).bind::<Text,_>(&c.definition.name).execute(db).await?;
     }else{fence(db,&w.intent_id).await?;}
    }
    if matches!(input.receipt, AgentExecutorReceipt::Fence(_)) { super::settlement::enqueue_initial(db, &w.request_id, &self.server).await?; }
    Self::audit(db,p,"agent.executor.result",id,now).await?;
   }
   // Exact historical acknowledgement does not preserve stale readiness.
   let current=clock(db).await?;
   if let AgentExecutorCommand::Reserve(c)=&c && !live(db,&w,c,current).await? {fence(db,&w.intent_id).await?;}
   Self::authorize(db,p,crate::api::now_ms()).await?;
   Ok(AgentExecutorAcknowledgement {command_id:id.into(),command_digest:digest,receipt_digest,assignment:projection(db,&w.request_id).await?})
  }).await
    }
    pub async fn managed_agent_assignment(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request: &str,
    ) -> Result<Option<PendingManagedAssignment>> {
        enabled_executor(policy)?;
        key(request)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   Self::authorize(db,p,crate::api::now_ms()).await?;let r=row(db,request).await?;
   let executor=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1 AND executor_owner_user_id=$2 AND executor_device_id=$3) AS matched").bind::<Text,_>(request).bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(p.device_id.as_deref()).get_result::<Flag>(db).await?.matched;
   if !executor {visible(db,&r,p).await?;}
   let w=sql_query(format!("{WORK} WHERE i.request_id=$1 AND w.operation='agent.reserve'")).bind::<Text,_>(request).get_result::<Work>(db).await.optional()?;
   if let Some(w)=w {let c:ManagedAgentReserveCommand=decode(&w.payload)?;let now=clock(db).await?;if !live(db,&w,&c,now).await? {fence(db,&w.intent_id).await?;}}
   projection(db,request).await
  }).await
    }
    pub async fn revoke_managed_agent_request(
        &self,
        p: &Principal,
        _policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<ManagedAgentRequest> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
   if let Some(saved)=replay::<ManagedAgentRequest>(db,p,&input.command_id,"agent.revoke",&digest).await?{return view(db,&saved.id,p,now).await;}
   let r=row(db,id).await?;let a=roles(db,&r.plan_id).await?;
   if a.administrator_user_id!=p.user_id{return Err(Error::Unauthorized("assigned_project_administrator_required"));}
   if r.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
   if r.state!="approved_waiting_executor"{return Err(Error::Conflict("agent_request_closed"));}
   sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state='revoked',revision=revision+1 WHERE id=$1").bind::<Text,_>(id).execute(db).await?;
   let result=view(db,id,p,now).await?;notice(db,&result,now).await?;receipt(db,p,&input.command_id,"agent.revoke",&digest,&result).await?;Self::audit(db,p,"agent.revoke",id,now).await?;Ok(result)
  }).await
    }
}
