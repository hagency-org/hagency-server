//! Bounded qualification process protocol. No shared work/model authority.
use super::*;
use crate::management_agent_runtime_types::*;
#[derive(QueryableByName)]
struct RuntimeRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    operation: String,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    payload: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    digest: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    expires_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
}
fn enabled_runtime(p: &ManagementPolicy) -> Result<()> {
    if p.agent_runtime_provisioning() {
        Ok(())
    } else {
        Err(Error::Unavailable("agent_runtime_provisioning_unavailable"))
    }
}
fn issuing(p: &ManagementPolicy) -> bool {
    p.agent_runtime_provisioning()
        && p.agent_matrix_provisioning()
        && p.agent_executor()
        && p.agent_admission()
        && p.project_reservations()
}
async fn runtime_row(db: &mut AsyncPgConnection, id: &str) -> Result<RuntimeRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_runtime_commands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn provision_row(db: &mut AsyncPgConnection, assignment: &str) -> Result<RuntimeRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1 AND operation='agent.runtime.provision'").bind::<Text,_>(assignment).get_result(db).await.map_err(hidden)
}
async fn original(db: &mut AsyncPgConnection, assignment: &str) -> Result<Work> {
    sql_query(format!("{WORK} JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=i.request_id WHERE a.id=$1 AND w.operation='agent.reserve'")).bind::<Text,_>(assignment).get_result(db).await.map_err(hidden)
}
async fn runtime_command(
    db: &mut AsyncPgConnection,
    r: &RuntimeRow,
    server: &str,
) -> Result<(ManagedRuntimeCommand, String)> {
    let c = if r.operation == "agent.runtime.provision" {
        let c: ManagedRuntimeProvisionCommand = decode(
            r.payload
                .as_deref()
                .ok_or(Error::Conflict("runtime_command_changed"))?,
        )?;
        if r.digest.as_deref() != Some(allocation_digest(&c)?.as_str()) {
            return Err(Error::Conflict("runtime_command_changed"));
        }
        ManagedRuntimeCommand::Provision(c)
    } else {
        let p = provision_row(db, &r.assignment_id).await?;
        let c: ManagedRuntimeProvisionCommand = decode(p.payload.as_ref().unwrap())?;
        ManagedRuntimeCommand::Stop(ManagedRuntimeStopCommand {
            version: 1,
            capability: AGENT_RUNTIME_CAPABILITY.into(),
            operation: "agent.runtime.stop".into(),
            command_id: r.id.clone(),
            provision_command_digest: allocation_digest(&c)?,
            provision_command: c,
            issued_at_ms: r.created_at_ms,
        })
    };
    c.validate(server)?;
    wire(&c)?;
    let digest = allocation_digest(&c)?;
    Ok((c, digest))
}
async fn matrix_current(db: &mut AsyncPgConnection, assignment: &str, now: i64) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_matrix WHERE assignment_id=$1 AND state='matrix_verified' AND verified_at_ms<=$2 AND verified_at_ms>$2-$3) AS matched").bind::<Text,_>(assignment).bind::<BigInt,_>(now).bind::<BigInt,_>(MAX_FACT_AGE_MS).get_result::<Flag>(db).await?.matched)
}
async fn current(db: &mut AsyncPgConnection, r: &RuntimeRow, w: &Work, now: i64) -> Result<bool> {
    let p = provision_row(db, &r.assignment_id).await?;
    let c: ManagedRuntimeProvisionCommand = decode(p.payload.as_ref().unwrap())?;
    if now >= c.expires_at_ms || !live(db, w, &c.reserve_command, now).await? {
        fence(db, &w.intent_id).await?;
        return Ok(false);
    }
    let codex=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id LEFT JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(r.pool_contribution_id,p.contribution_id) WHERE a.id=$1 AND c.runtime='codex') AS matched").bind::<Text,_>(&r.assignment_id).get_result::<Flag>(db).await?.matched;
    if !codex {
        fence(db, &w.intent_id).await?;
        return Ok(false);
    }
    matrix_current(db, &r.assignment_id, now).await
}
async fn latest(
    db: &mut AsyncPgConnection,
    command: &str,
    observation: Option<&str>,
) -> Result<Option<ManagedRuntimeReceipt>> {
    let r=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_runtime_observations WHERE command_id=$1 AND ($2::text IS NULL OR id=$2) ORDER BY ledger_revision DESC LIMIT 1").bind::<Text,_>(command).bind::<Nullable<Text>,_>(observation).get_result::<Json>(db).await.optional()?;
    r.map(|v| decode(&v.payload)).transpose()
}
async fn runtime_view(
    db: &mut AsyncPgConnection,
    assignment: &str,
    now: i64,
) -> Result<ManagedRuntimeReadiness> {
    let p = provision_row(db, assignment).await?;
    let w = original(db, assignment).await?;
    let live = current(db, &p, &w, now).await?;
    let r=sql_query("SELECT o.payload FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id WHERE c.assignment_id=$1 ORDER BY o.ledger_revision DESC LIMIT 1").bind::<Text,_>(assignment).get_result::<Json>(db).await.optional()?.map(|r|decode::<ManagedRuntimeReceipt>(&r.payload)).transpose()?;
    let stopped=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1 AND operation='agent.runtime.stop') AS matched").bind::<Text,_>(assignment).get_result::<Flag>(db).await?.matched;
    let state = if stopped {
        match r.as_ref().map(|r| r.state.as_str()) {
            Some("stopped" | "not_started") => "stopped",
            _ => "cleanup_pending",
        }
    } else {
        match r.as_ref().map(|r| r.state.as_str()) {
            Some("qualified") if live => "qualified",
            Some("qualified") => "qualification_stale",
            Some("failed") => "failed",
            Some("unknown") => "outcome_unknown",
            _ => "pending_native",
        }
    };
    Ok(ManagedRuntimeReadiness {
        assignment_id: assignment.into(),
        command_id: p.id,
        state: state.into(),
        process_state: r
            .as_ref()
            .map(|r| r.process_state.clone())
            .unwrap_or_else(|| "unknown".into()),
        provision_expires_at_ms: p.expires_at_ms.unwrap(),
        latest: r,
        work_available: false,
        capacity_released: false,
    })
}
async fn runtime_delivery(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    r: &RuntimeRow,
    server: &str,
    historical: bool,
    observation: Option<&str>,
) -> Result<ManagedRuntimeDelivery> {
    let w = original(db, &r.assignment_id).await?;
    owner(&w, p)?;
    let (c, digest) = runtime_command(db, r, server).await?;
    let now = clock(db).await?;
    let live = current(db, r, &w, now).await?;
    let recorded = latest(db, &r.id, observation).await?;
    let allowed = !historical
        && match &c {
            ManagedRuntimeCommand::Provision(_) => {
                issuing(policy)
                    && live
                    && p.device_generation == Some(w.executor_generation)
                    && recorded.is_none()
            }
            ManagedRuntimeCommand::Stop(_) => !recorded
                .as_ref()
                .is_some_and(|r| matches!(r.state.as_str(), "stopped" | "not_started")),
        };
    let until = sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now;
    let until = if allowed && matches!(c, ManagedRuntimeCommand::Provision(_)) {
        until.min(c.original().expires_at_ms)
    } else {
        until
    };
    let token = secret_token();
    sql_query("UPDATE hagency_agent_v1.management_agent_runtime_commands SET lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).execute(db).await?;
    Ok(ManagedRuntimeDelivery {
        command: c,
        command_digest: digest,
        token,
        lease_until_ms: until,
        execution_allowed: allowed,
        recorded,
    })
}
impl DomainStore {
    /// A bounded expiry sweep fences issuance and queues exact cleanup atomically.
    pub(crate) async fn sweep_managed_runtime(&self) -> Result<()> {
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                    .await?;
                let now = clock(db).await?;
                runtime_sweep(db, now).await
            })
            .await
    }
    pub async fn poll_managed_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<ManagedRuntimeDelivery>> {
        enabled_runtime(policy)?;
        let (device, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;runtime_sweep(db,now).await?;sweep(db,p,now).await?;
   let assignments=sql_query("SELECT id AS payload FROM hagency_agent_v1.management_shared_assignments WHERE executor_owner_user_id=$1 AND executor_device_id=$2 AND id>$3 ORDER BY id LIMIT 32").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(input.after.as_deref().unwrap_or("")).load::<Json>(db).await?;
   for a in assignments {
    let existing=sql_query("SELECT * FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1 ORDER BY operation DESC").bind::<Text,_>(&a.payload).load::<RuntimeRow>(db).await?;
    for r in &existing {let d=runtime_delivery(db,p,policy,r,&self.server,false,None).await?;if d.execution_allowed{return Ok(Some(d));}}
    if !existing.is_empty()||!issuing(policy){continue;}
    let shared=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=$1) AS matched").bind::<Text,_>(&a.payload).get_result::<Flag>(db).await?.matched;if shared{continue;}
    let w=original(db,&a.payload).await?;owner(&w,p)?;let reserve:ManagedAgentReserveCommand=decode(&w.payload)?;if p.device_generation!=Some(w.executor_generation)||!live(db,&w,&reserve,now).await?||!matrix_current(db,&a.payload,now).await?{continue;}
    let r=row(db,&w.request_id).await?;let cx=context(db,&r.plan_id,now).await?;
    if !matches!(cx.contribution.runtime,Runtime::Codex){continue;}
    let a=super::projection(db,&w.request_id).await?.ok_or(Error::Conflict("agent_assignment_required"))?;
    let c=ManagedRuntimeProvisionCommand{version:1,capability:AGENT_RUNTIME_CAPABILITY.into(),operation:"agent.runtime.provision".into(),command_id:format!("mrp_{}",entity_id()?),assignment_id:a.id.clone(),assignment_generation:a.generation,agent_reservation_id:a.agent_reservation_id,reserve_command_digest:w.digest.clone(),reserve_receipt_digest:a.reserve_receipt_digest,policy:"codex_no_tools_v1".into(),issued_at_ms:now,expires_at_ms:(now+30000).min(reserve.expires_at_ms),reserve_command:reserve};c.validate(&self.server)?;
    sql_query("INSERT INTO hagency_agent_v1.management_agent_runtime_commands(id,assignment_id,operation,created_at_ms,payload,digest,expires_at_ms) VALUES($1,$2,'agent.runtime.provision',$3,$4,$5,$6)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id).bind::<BigInt,_>(now).bind::<Text,_>(wire(&c)?).bind::<Text,_>(allocation_digest(&c)?).bind::<BigInt,_>(c.expires_at_ms).execute(db).await?;
    let r=runtime_row(db,&c.command_id).await?;return Ok(Some(runtime_delivery(db,p,policy,&r,&self.server,false,None).await?));
   }Ok(None)
  }).await
    }
    pub async fn recover_managed_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ManagedRuntimeRecoverInput,
    ) -> Result<ManagedRuntimeDelivery> {
        enabled_runtime(policy)?;
        key(id)?;
        if let Some(id) = &input.observation_id {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let r = runtime_row(db, id).await?;
                let w = original(db, &r.assignment_id).await?;
                owner(&w, p)?;
                let (_, digest) = runtime_command(db, &r, &self.server).await?;
                if digest != input.command_digest {
                    return Err(Error::Conflict("runtime_command_changed"));
                }
                runtime_delivery(
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
    pub async fn record_managed_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ManagedRuntimeResultInput,
    ) -> Result<ManagedRuntimeAcknowledgement> {
        enabled_runtime(policy)?;
        key(id)?;
        wire(&input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let r=runtime_row(db,id).await?;let w=original(db,&r.assignment_id).await?;owner(&w,p)?;let(c,digest)=runtime_command(db,&r,&self.server).await?;
   if digest!=input.command_digest{return Err(Error::Conflict("runtime_command_changed"));}input.receipt.validate_for(&c)?;let receipt_digest=allocation_digest(&input.receipt)?;
   let old=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_runtime_observations WHERE id=$1").bind::<Text,_>(&input.receipt.observation_id).get_result::<Json>(db).await.optional()?;
   if let Some(old)=old {if allocation_digest(&decode::<ManagedRuntimeReceipt>(&old.payload)?)?!=receipt_digest{return Err(Error::Conflict("runtime_observation_changed"));}}
   else {if r.lease_until_ms<=now||r.lease_hash.as_deref()!=Some(hash(&input.token).as_str()){return Err(Error::Unauthorized("runtime_ticket_required"));}validate_receipt(db,&w,&r,&c,&input.receipt).await?;
    sql_query("INSERT INTO hagency_agent_v1.management_agent_runtime_observations(id,command_id,payload,digest,ledger_revision) VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&input.receipt.observation_id).bind::<Text,_>(id).bind::<Text,_>(wire(&input.receipt)?).bind::<Text,_>(&receipt_digest).bind::<BigInt,_>(input.receipt.ledger_revision).execute(db).await?;
   }
   let now=clock(db).await?;let runtime=runtime_view(db,&r.assignment_id,now).await?;Self::authorize(db,p,crate::api::now_ms()).await?;Ok(ManagedRuntimeAcknowledgement{command_id:id.into(),command_digest:digest,receipt_digest,runtime})
  }).await
    }
    pub async fn managed_runtime(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request: &str,
    ) -> Result<Option<ManagedRuntimeReadiness>> {
        enabled_runtime(policy)?;
        key(request)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let r=row(db,request).await?;let a=super::projection(db,request).await?;let Some(a)=a else{visible(db,&r,p).await?;return Ok(None)};let w=original(db,&a.id).await?;if owner(&w,p).is_err(){visible(db,&r,p).await?;}let exists=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1) AS matched").bind::<Text,_>(&a.id).get_result::<Flag>(db).await?.matched;if !exists{return Ok(None)}let now=clock(db).await?;Ok(Some(runtime_view(db,&a.id,now).await?))}).await
    }
}
async fn runtime_sweep(db: &mut AsyncPgConnection, now: i64) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE id IN(SELECT i.id FROM hagency_agent_v1.management_agent_runtime_commands c JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id WHERE c.operation='agent.runtime.provision' AND c.expires_at_ms<=$1 AND i.state<>'fenced' ORDER BY c.id LIMIT 64)").bind::<BigInt,_>(now).execute(db).await?;
    Ok(())
}
async fn validate_receipt(
    db: &mut AsyncPgConnection,
    w: &Work,
    r: &RuntimeRow,
    c: &ManagedRuntimeCommand,
    v: &ManagedRuntimeReceipt,
) -> Result<()> {
    let base: ManagedAgentReservationReceipt = decode(
        w.receipt
            .as_deref()
            .ok_or(Error::Conflict("agent_reservation_required"))?,
    )?;
    if v.ledger_revision <= base.ledger_revision {
        return Err(Error::Conflict("runtime_ledger_revision_changed"));
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
        return Err(Error::Conflict("runtime_ledger_revision_changed"));
    }
    let observations=sql_query("SELECT o.payload FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id WHERE c.assignment_id=$1 ORDER BY o.ledger_revision DESC LIMIT 256").bind::<Text,_>(&r.assignment_id).load::<Json>(db).await?;
    if observations.len() >= 256 {
        return Err(Error::Conflict("runtime_observation_limit"));
    }
    for old in observations {
        let old: ManagedRuntimeReceipt = decode(&old.payload)?;
        if old.ledger_revision == v.ledger_revision {
            return Err(Error::Conflict("runtime_ledger_revision_changed"));
        }
        let terminal = |v: &ManagedRuntimeReceipt| {
            matches!(v.state.as_str(), "stopped" | "not_started" | "failed")
        };
        if (terminal(&old) && v.ledger_revision > old.ledger_revision && !terminal(v))
            || (terminal(v) && old.ledger_revision > v.ledger_revision && !terminal(&old))
        {
            return Err(Error::Conflict("runtime_process_cannot_restart"));
        }
        if matches!(c, ManagedRuntimeCommand::Stop(_))
            && v.process_state == "not_started"
            && old.state == "qualified"
            || v.state == "qualified" && old.process_state == "not_started"
        {
            return Err(Error::Conflict("runtime_process_evidence_conflict"));
        }
    }
    Ok(())
}
