//! V3 process custody pinned to an immutable applied budget head. Old attempt bytes stay v2.
use super::*;
use crate::management_shared_attempt_types::{MAX_ATTEMPT_BYTES, SharedAttemptAcknowledgement};
use crate::management_shared_budget_types::*;

#[derive(QueryableByName)]
pub(super) struct Evidence {
    #[diesel(sql_type=Text)]
    observation_id: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=BigInt)]
    pub(super) ledger_revision: i64,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    process_digest: Option<String>,
}
pub(super) async fn latest_evidence(
    db: &mut AsyncPgConnection,
    id: &str,
    observation: Option<&str>,
) -> Result<Option<Evidence>> {
    Ok(sql_query("SELECT id AS observation_id,digest,ledger_revision,payload::jsonb->>'state' AS state,payload::jsonb->>'processDigest' AS process_digest FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND ($2::text IS NULL OR id=$2) ORDER BY ledger_revision DESC LIMIT 1")
        .bind::<Text,_>(id).bind::<Nullable<Text>,_>(observation).get_result(db).await.optional()?)
}
pub(super) fn attempt_command(r: &Runtime, server: &str) -> Result<SharedBudgetAttemptCommand> {
    let c: SharedBudgetAttemptCommand = decode(&r.payload)?;
    c.validate(server)?;
    if c.command_id != r.id
        || c.assignment_id != r.assignment_id
        || allocation_digest(&c)? != r.digest
    {
        return Err(Error::Conflict("shared_attempt_command_changed"));
    }
    Ok(c)
}
fn encoded<T: serde::Serialize>(v: &T) -> Result<String> {
    let s = serde_json::to_string(v).map_err(|_| Error::Invalid("invalid_shared_attempt"))?;
    if s.len() > MAX_ATTEMPT_BYTES {
        return Err(Error::Invalid("invalid_shared_attempt"));
    }
    Ok(s)
}
pub(super) fn fresh_enabled(p: &ManagementPolicy) -> Result<()> {
    enabled(p)?;
    if !p.shared_budget_execution() || !p.shared_successors() || !issuing(p) {
        return Err(Error::Unavailable(
            "shared_budget_execution_issuance_unavailable",
        ));
    }
    Ok(())
}
async fn original_runtime(
    db: &mut AsyncPgConnection,
    assignment: &str,
    server: &str,
) -> Result<SharedRuntimeCommand> {
    let id=sql_query("SELECT id FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=$1 AND payload::jsonb->>'version'='1'").bind::<Text,_>(assignment).get_result::<Id>(db).await.map_err(hidden)?;
    original_command(&runtime(db, &id.id).await?, server)
}
async fn recorded(
    db: &mut AsyncPgConnection,
    id: &str,
    observation: Option<&str>,
) -> Result<Option<SharedBudgetAttemptProcessReceipt>> {
    let raw=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND ($2::text IS NULL OR id=$2) ORDER BY ledger_revision DESC LIMIT 1").bind::<Text,_>(id).bind::<Nullable<Text>,_>(observation).get_result::<Json>(db).await.optional()?;
    raw.map(|v| decode(&v.payload)).transpose()
}
pub(super) async fn attempt_delivery(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    r: &Runtime,
    server: &str,
    historical: bool,
    observation: Option<&str>,
) -> Result<SharedBudgetAttemptDelivery> {
    let w = original(db, r).await?;
    owner(&w, p)?;
    let c = attempt_command(r, server)?;
    let scope = command(r, server)?;
    let now = clock(db).await?;
    let live = current(db, r, &w, &scope, now).await?
        && (r.activity_epoch > 0 || head_matches(db, r, server).await?);
    let expired = r.state == "qualified" && r.activity_until_ms <= now
        || matches!(r.state.as_str(), "pending" | "issued") && c.expires_at_ms <= now;
    if !live || expired {
        fence_runtime(db, &r.id).await?;
    }
    let recorded = recorded(db, &r.id, observation).await?;
    let executable = !historical
        && (!policy.shared_durable_intake() || intake::running(db, &r.assignment_id).await?)
        && policy.shared_successors()
        && runtime_issuing(db, policy, r).await?
        && live
        && !expired
        && p.device_generation == Some(w.executor_generation)
        && matches!(r.state.as_str(), "pending" | "issued")
        && recorded.is_none();
    let cleanup = !historical
        && (!live || expired || r.permanent_fence || r.state == "unknown")
        && r.state != "stopped";
    let token = secret_token();
    let expiry = if executable {
        until(db, p, now).await?.min(c.expires_at_ms)
    } else {
        until(db, p, now).await?
    };
    tickets::issue(
        db,
        p,
        &r.id,
        &token,
        if executable {
            "execute"
        } else if cleanup {
            "cleanup"
        } else {
            "observe"
        },
        expiry,
        now,
    )
    .await?;
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET lease_hash=$2,lease_until_ms=$3,lease_mode=$5,state=CASE WHEN $4 AND state='pending' THEN 'issued' ELSE state END WHERE id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(expiry).bind::<Bool,_>(executable).bind::<Text,_>(if executable {"execute"}else if cleanup {"cleanup"}else{"observe"}).execute(db).await?;
    Ok(SharedBudgetAttemptDelivery {
        command: c,
        command_digest: r.digest.clone(),
        token,
        lease_until_ms: expiry,
        execution_allowed: executable,
        cleanup_allowed: cleanup,
        recorded,
    })
}
#[derive(QueryableByName)]
struct Start {
    #[diesel(sql_type=Text)]
    runtime_id: String,
    #[diesel(sql_type=Text)]
    input_digest: String,
}
pub(super) async fn prior_start(
    db: &mut AsyncPgConnection,
    input: &SharedBudgetAttemptStartInput,
    digest: &str,
    p: &Principal,
) -> Result<Option<Runtime>> {
    let saved=sql_query("SELECT runtime_id,input_digest FROM hagency_agent_v1.management_shared_attempts WHERE id=$1").bind::<Text,_>(&input.start_intent_id).get_result::<Start>(db).await.optional()?;
    let Some(saved) = saved else { return Ok(None) };
    let r = runtime(db, &saved.runtime_id).await?;
    owner(&original(db, &r).await?, p)?;
    if saved.input_digest != digest || r.assignment_id != input.assignment_id {
        return Err(Error::Conflict("shared_start_intent_changed"));
    }
    Ok(Some(r))
}
pub(super) async fn ready(
    db: &mut AsyncPgConnection,
    p: &Principal,
    input: &SharedBudgetAttemptStartInput,
    server: &str,
    now: i64,
) -> Result<(Runtime, SharedRuntimeCommand)> {
    let r = runtime(db, &input.predecessor.command_id).await?;
    let w = original(db, &r).await?;
    owner(&w, p)?;
    let original = original_runtime(db, &r.assignment_id, server).await?;
    let number = if r.id == original.command_id {
        1
    } else {
        if runtime_version(&r)? == 3 {
            attempt_command(&r, server)?.process_attempt
        } else {
            attempts::attempt_command(&r, server)?.process_attempt
        }
    };
    if r.assignment_id != input.assignment_id
        || r.digest != input.predecessor.command_digest
        || number != input.predecessor.process_attempt
        || r.state != "stopped"
        || !r.permanent_fence
        || r.process_digest.as_deref().unwrap_or("") != input.predecessor.process_digest
    {
        return Err(Error::Conflict("shared_predecessor_not_stopped"));
    }
    if intake::paused(db, &r.assignment_id).await?
        || p.device_generation != Some(w.executor_generation)
        || !live(db, &w, &original.reserve_command, now).await?
    {
        return Err(Error::Conflict("shared_assignment_authority_changed"));
    }
    let codex=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests r LEFT JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(r.pool_contribution_id,p.contribution_id) WHERE r.id=$1 AND c.runtime='codex') AS matched").bind::<Text,_>(&original.reserve_command.request_id).get_result::<Flag>(db).await?.matched;
    if !codex {
        return Err(Error::Conflict("shared_assignment_authority_changed"));
    }
    let stale=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=$1 AND coalesce((payload::jsonb->>'processAttempt')::bigint,1)>$2) AS matched").bind::<Text,_>(&r.assignment_id).bind::<BigInt,_>(number).get_result::<Flag>(db).await?.matched;
    if stale {
        return Err(Error::Conflict("shared_attempt_head_changed"));
    }
    let stop = latest_evidence(db, &r.id, None)
        .await?
        .ok_or(Error::Conflict("shared_predecessor_not_stopped"))?;
    if (if input.predecessor.process_digest.is_empty() {stop.state != "not_started"} else {stop.state != "stopped"})
        || stop.digest != input.predecessor.stopped_receipt_digest
        || stop.process_digest != r.process_digest
        || input.budget_observation.observed_ledger_revision < stop.ledger_revision
    {
        return Err(Error::Conflict("shared_predecessor_evidence_changed"));
    }
    let b = &input.budget_observation;
    let (head, ledger, known) =
        super::super::super::budget::execution_head(db, &w.request_id, server).await?;
    if input.execution_budget != head || b.observed_ledger_revision < ledger {
        return Err(Error::Conflict("shared_budget_head_changed"));
    }
    if b.allocated_tokens != head.effective_ceiling_tokens || b.remaining_tokens <= 0 {
        return Err(Error::Conflict("shared_capacity_exhausted"));
    }
    // Native observation can include work not yet uploaded; it cannot erase
    // positive consumption already accepted centrally for any old attempt.
    if b.known_consumed_tokens < known {
        return Err(Error::Conflict("shared_budget_observation_changed"));
    }
    Ok((r, original))
}
pub(super) async fn commit_start(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    input: &SharedBudgetAttemptStartInput,
    prepared: (Runtime, SharedRuntimeCommand),
    server: &str,
    now: i64,
) -> Result<SharedBudgetAttemptDelivery> {
    let (previous, original) = prepared;
    let c = SharedBudgetAttemptCommand {
        web_search: input.web_search, tool_policy: input.tool_policy.clone(),
        version: 3,
        capability: SHARED_BUDGET_CAPABILITY.into(),
        operation: "agent.execution.provision".into(),
        command_id: format!("sat_{}", entity_id()?),
        assignment_id: original.assignment_id.clone(),
        assignment_generation: original.assignment_generation,
        agent_reservation_id: original.agent_reservation_id.clone(),
        process_attempt: input
            .predecessor
            .process_attempt
            .checked_add(1)
            .ok_or(Error::Conflict("shared_attempt_limit"))?,
        original_runtime_digest: allocation_digest(&original)?,
        predecessor: input.predecessor.clone(),
        budget_observation: input.budget_observation.clone(),
        execution_budget: input.execution_budget.clone(),
        start_intent_id: input.start_intent_id.clone(),
        issued_at_ms: now,
        expires_at_ms: (now + 30000).min(original.reserve_command.expires_at_ms),
        original_runtime_command: original,
    };
    c.validate(server)?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_attempts(id,assignment_id,runtime_id,predecessor_id,original_runtime_id,process_attempt,input_payload,input_digest,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind::<Text,_>(&input.start_intent_id).bind::<Text,_>(&c.assignment_id).bind::<Text,_>(&c.command_id).bind::<Text,_>(&previous.id).bind::<Text,_>(&c.original_runtime_command.command_id).bind::<BigInt,_>(c.process_attempt).bind::<Text,_>(wire(input)?).bind::<Text,_>(allocation_digest(input)?).bind::<BigInt,_>(now).execute(db).await?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_budget_attempts(runtime_id,assignment_id,budget_revision,budget_digest,applied_command_id,pin_payload,pin_digest) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id)
        .bind::<BigInt,_>(c.execution_budget.budget_revision).bind::<Text,_>(&c.execution_budget.budget_digest)
        .bind::<Text,_>(&c.execution_budget.applied_command_id).bind::<Text,_>(wire(&c.execution_budget)?)
        .bind::<Text,_>(allocation_digest(&c.execution_budget)?).execute(db).await?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_runtimes(id,assignment_id,payload,digest,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id).bind::<Text,_>(encoded(&c)?).bind::<Text,_>(allocation_digest(&c)?).bind::<BigInt,_>(now).bind::<BigInt,_>(c.expires_at_ms).execute(db).await?;
    let inserted = runtime(db, &c.command_id).await?;
    attempt_delivery(db, p, policy, &inserted, server, false, None).await
}
impl DomainStore {
    pub async fn start_shared_budget_attempt(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: SharedBudgetAttemptStartInput,
        gateway: &Gateway,
    ) -> Result<SharedBudgetAttemptDelivery> {
        enabled(policy)?;
        input.validate()?;
        let digest = allocation_digest(&input)?;
        let scope = {
            let mut guard = self.db.lock().await?;
            (*guard)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, 0).await?;
                    if let Some(old) = prior_start(db, &input, &digest, p).await? {
                        return Ok(Err(attempt_delivery(
                            db,
                            p,
                            policy,
                            &old,
                            &self.server,
                            true,
                            None,
                        )
                        .await?));
                    }
                    fresh_enabled(policy)?;
                    let control_revision = if policy.shared_durable_intake() {
                        Some(intake::running_revision(db, &input.assignment_id).await?)
                    } else {
                        None
                    };
                    let now = clock(db).await?;
                    let (r, _) = ready(db, p, &input, &self.server, now).await?;
                    Ok(Ok((membership(db, &r).await?, control_revision)))
                })
                .await?
        };
        let (scope, control_revision) = match scope {
            Err(saved) => return Ok(saved),
            Ok(scope) => scope,
        };
        let observed = gateway
            .shared_delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.manager_mxid,
                &scope.manager_mxid,
                &scope.puppet_mxid,
            )
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                if let Some(old) = prior_start(db, &input, &digest, p).await? {
                    return attempt_delivery(db, p, policy, &old, &self.server, true, None).await;
                }
                fresh_enabled(policy)?;
                if let Some(revision) = control_revision
                    && intake::running_revision(db, &input.assignment_id).await? != revision
                {
                    return Err(Error::Conflict("shared_control_revision_changed"));
                }
                let now = clock(db).await?;
                let (previous, original) = ready(db, p, &input, &self.server, now).await?;
                facts(&membership(db, &previous).await?, &observed)?;
                commit_start(
                    db,
                    p,
                    policy,
                    &input,
                    (previous, original),
                    &self.server,
                    now,
                )
                .await
            })
            .await
    }
    pub async fn poll_shared_budget_attempt(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<SharedBudgetAttemptDelivery>> {
        enabled(policy)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            Self::authorize(db,p,0).await?;let (device,_)=device(p)?;let now=clock(db).await?;sweep(db,p,now).await?;
            let id=sql_query("SELECT r.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE r.payload::jsonb->>'version'='3' AND a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND r.state<>'stopped' AND (r.state<>'qualified' OR r.permanent_fence OR r.activity_until_ms<=$4) ORDER BY (r.id<=$3),r.id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(now).get_result::<Id>(db).await.optional()?;
            let Some(id)=id else{return Ok(None)};let r=runtime(db,&id.id).await?;Ok(Some(attempt_delivery(db,p,policy,&r,&self.server,false,None).await?))
        }).await
    }
    pub async fn recover_shared_budget_attempt(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ManagedRuntimeRecoverInput,
    ) -> Result<SharedBudgetAttemptDelivery> {
        enabled(policy)?;
        key(id)?;
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
                    return Err(Error::Conflict("shared_attempt_command_changed"));
                }
                attempt_delivery(
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
    pub async fn record_shared_budget_attempt_process(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedBudgetAttemptResultInput,
    ) -> Result<SharedAttemptAcknowledgement> {
        enabled(policy)?;
        key(id)?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            Self::authorize(db,p,0).await?;let r=runtime(db,id).await?;let w=original(db,&r).await?;owner(&w,p)?;let c=attempt_command(&r,&self.server)?;input.receipt.validate_for(&c)?;let now=clock(db).await?;
            if input.command_digest!=r.digest {return Err(Error::Conflict("shared_result_ticket_changed"));}
            let ticket_mode=tickets::mode(db,p,id,&input.token,now).await?;
            let digest=allocation_digest(&input.receipt)?;
            let old=sql_query("SELECT payload FROM hagency_agent_v1.management_shared_process_receipts WHERE id=$1").bind::<Text,_>(&input.receipt.observation_id).get_result::<Json>(db).await.optional()?;
            if let Some(old)=old {let old:SharedBudgetAttemptProcessReceipt=decode(&old.payload)?;if allocation_digest(&old)?!=digest {return Err(Error::Conflict("shared_process_receipt_changed"));}}
            else {
                validate_attempt_receipt(db,&r,&w,&c,&input.receipt).await?;
                sql_query("INSERT INTO hagency_agent_v1.management_shared_process_receipts VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&input.receipt.observation_id).bind::<Text,_>(id).bind::<Text,_>(wire(&input.receipt)?).bind::<Text,_>(&digest).bind::<BigInt,_>(input.receipt.ledger_revision).execute(db).await?;
            }
            let fresh=latest_evidence(db,id,None).await?.unwrap();
                if fresh.observation_id==input.receipt.observation_id {
                    let scope=command(&r,&self.server)?;let live=current(db,&r,&w,&scope,now).await? && (r.activity_epoch>0 || head_matches(db,&r,&self.server).await?);
                    let pending=input.receipt.state=="qualified"&&ticket_mode!="execute"&&r.activity_epoch==0&&!r.permanent_fence&&now<c.expires_at_ms&&live&&tickets::has_execute(db,p,id,now).await?;
                    let fenced=r.permanent_fence||!pending&&(ticket_mode!="execute"&&r.activity_epoch==0||input.receipt.state!="qualified"||now>=c.expires_at_ms&&r.activity_epoch==0||!live);
                    let state=if input.receipt.state=="not_started" {"stopped"} else if pending {r.state.as_str()}else{input.receipt.state.as_str()};
                    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET state=$2,process_digest=coalesce(process_digest,$3),permanent_fence=permanent_fence OR $4,activity_until_ms=CASE WHEN $4 THEN 0 ELSE activity_until_ms END WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(state).bind::<Nullable<Text>,_>(&input.receipt.process_digest).bind::<Bool,_>(fenced).execute(db).await?;
                }
            let r=runtime(db,id).await?;let scope=command(&r,&self.server)?;
            let activity=if runtime_issuing(db,policy,&r).await?&&input.receipt.state=="qualified"&&ticket_mode=="execute" {let now=clock(db).await?;mint_activity(db,p,&r,&scope,now).await?}else{None};
            Ok(SharedAttemptAcknowledgement{observation_id:input.receipt.observation_id,receipt_digest:digest,activity,state:if r.permanent_fence&&r.state!="stopped" {"cleanup_pending".into()}else{r.state}})
        }).await
    }
}
async fn validate_attempt_receipt(
    db: &mut AsyncPgConnection,
    r: &Runtime,
    w: &Work,
    c: &SharedBudgetAttemptCommand,
    v: &SharedBudgetAttemptProcessReceipt,
) -> Result<()> {
    if v.state == "not_started" {
        let effect=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1) AS matched")
            .bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched;
        if !r.permanent_fence || r.activity_epoch!=0 || r.process_digest.is_some() || effect || !matches!(r.state.as_str(),"pending"|"issued") {
            return Err(Error::Conflict("shared_attempt_not_unstarted"));
        }
    }
    if v.ledger_revision <= c.budget_observation.observed_ledger_revision
        || r.process_digest
            .as_ref()
            .is_some_and(|d| v.process_digest.as_ref().is_some_and(|v| v != d))
    {
        return Err(Error::Conflict("shared_process_evidence_changed"));
    }
    super::super::validate_external_ledger_receipt(
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
        let old: SharedBudgetAttemptProcessReceipt = decode(&old.payload)?;
        if old.state == "not_started" || v.state == "not_started"
            || old.ledger_revision == v.ledger_revision
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

pub(super) async fn head_matches(db: &mut AsyncPgConnection, r: &Runtime, server: &str) -> Result<bool> {
    let c = attempt_command(r, server)?;
    let w = original(db, r).await?;
    let (head, _, known) =
        super::super::super::budget::execution_head(db, &w.request_id, server).await?;
    Ok(c.execution_budget == head && known < head.effective_ceiling_tokens)
}
