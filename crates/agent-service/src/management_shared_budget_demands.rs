//! Budget reassessment appends to an original offer; it never rebases its head or event.
use super::*;
use crate::management_shared_budget_types::*;
#[path = "management_shared_budget_demand_start.rs"]
mod start;

#[derive(QueryableByName)]
struct Pin {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
#[derive(QueryableByName)]
struct Block {
    #[diesel(sql_type=Text)]
    reason: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
fn original_block_evidence(b: Block) -> Result<SharedOriginalBudgetBlock> {
    // The retained v2 row hashes its exact serialized bytes. V3's public proof
    // uses the canonical DTO digest shared with native. Verify the old bytes
    // before deriving that proof; never rewrite the original block or digest.
    if hash(&b.payload) != b.digest {
        return Err(Error::Conflict("shared_demand_block_changed"));
    }
    let value: serde_json::Value = serde_json::from_str(&b.payload)
        .map_err(|_| Error::Conflict("shared_demand_block_changed"))?;
    if value["reason"] != b.reason {
        return Err(Error::Conflict("shared_demand_block_changed"));
    }
    Ok(SharedOriginalBudgetBlock {
        reason: b.reason,
        payload_digest: allocation_digest(&value)?,
    })
}
fn enabled_budget(p: &ManagementPolicy) -> Result<()> {
    enabled_demand(p)?;
    budget_attempts::fresh_enabled(p)
}
async fn original_block(
    db: &mut AsyncPgConnection,
    id: &str,
) -> Result<Option<SharedOriginalBudgetBlock>> {
    sql_query("SELECT reason,payload,digest FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1")
        .bind::<Text,_>(id).get_result::<Block>(db).await.optional()?
        .map(original_block_evidence).transpose()
}
async fn started(db: &mut AsyncPgConnection, id: &str) -> Result<Option<Started>> {
    Ok(sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_budget_demand_starts WHERE demand_id=$1")
        .bind::<Text,_>(id).get_result(db).await.optional()?)
}
pub(in super::super) async fn binding(
    db: &mut AsyncPgConnection,
    id: &str,
) -> Result<Option<SharedDemandBinding>> {
    let Some(row)=sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE demand_id=$1")
        .bind::<Text,_>(id).get_result::<Started>(db).await.optional()? else{return Ok(None)};
    let v: SharedDemandBinding = decode(&row.payload)?;
    if v.demand_id != id
        || v.runtime_command_id != row.runtime_id
        || allocation_digest(&v)? != row.digest
    {
        return Err(Error::Conflict("shared_binding_changed"));
    }
    Ok(Some(v))
}
async fn latest_pin(db: &mut AsyncPgConnection, id: &str) -> Result<SharedExecutionBudget> {
    let p=sql_query("SELECT payload,digest FROM hagency_agent_v1.management_shared_budget_demand_heads WHERE demand_id=$1 ORDER BY budget_revision DESC LIMIT 1")
        .bind::<Text,_>(id).get_result::<Pin>(db).await.map_err(hidden)?;
    let pin: SharedExecutionBudget = decode(&p.payload)?;
    pin.validate()?;
    if allocation_digest(&pin)? != p.digest {
        return Err(Error::Conflict("shared_budget_head_changed"));
    }
    Ok(pin)
}
async fn save_pin(
    db: &mut AsyncPgConnection,
    d: &Demand,
    pin: &SharedExecutionBudget,
) -> Result<()> {
    let p = wire(pin)?;
    let digest = allocation_digest(pin)?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_budget_demand_heads VALUES($1,$2,$3,$4,$5) ON CONFLICT(demand_id,budget_revision) DO NOTHING")
        .bind::<Text,_>(&d.id).bind::<BigInt,_>(pin.budget_revision).bind::<Text,_>(&pin.budget_digest).bind::<Text,_>(&p).bind::<Text,_>(&digest).execute(db).await?;
    if latest_pin(db, &d.id).await? != *pin {
        return Err(Error::Conflict("shared_budget_head_changed"));
    }
    Ok(())
}
async fn terminal(db: &mut AsyncPgConnection, d: &Demand, reason: &str) -> Result<()> {
    let payload = wire(&json!({"demandDigest":d.digest,"reason":reason}))?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_budget_demand_terminals VALUES($1,$2,$3,$4) ON CONFLICT(demand_id) DO NOTHING")
        .bind::<Text,_>(&d.id).bind::<Text,_>(reason).bind::<Text,_>(&payload).bind::<Text,_>(hash(&payload)).execute(db).await?;
    Ok(())
}
async fn status(db: &mut AsyncPgConnection, d: &Demand) -> Result<SharedBudgetDemandStatus> {
    let q = queued(db, &d.intake_id).await?;
    let pin = latest_pin(db, &d.id).await?;
    let s = started(db, &d.id).await?;
    let b = binding(db, &d.id).await?;
    let terminal=sql_query("SELECT reason AS payload FROM hagency_agent_v1.management_shared_budget_demand_terminals WHERE demand_id=$1")
        .bind::<Text,_>(&d.id).get_result::<Json>(db).await.optional()?;
    let blocked=sql_query("SELECT reason AS payload FROM hagency_agent_v1.management_shared_budget_demand_blocks WHERE demand_id=$1 AND budget_revision=$2")
        .bind::<Text,_>(&d.id).bind::<BigInt,_>(pin.budget_revision).get_result::<Json>(db).await.optional()?;
    let original_block = original_block(db, &d.id).await?;
    let state = if b.is_some() {
        "bound".into()
    } else if let Some(t) = terminal {
        t.payload
    } else if s.is_some() {
        "start_committed".into()
    } else if let Some(b) = blocked {
        b.payload
    } else if !matches!(q.state.as_str(), "pending" | "blocked") {
        q.state.clone()
    } else {
        "offered".into()
    };
    Ok(SharedBudgetDemandStatus {
        offer: offer(d, &q)?,
        demand_digest: d.digest.clone(),
        state,
        execution_budget: pin,
        original_block,
        runtime_command_id: s
            .map(|s| s.runtime_id)
            .or_else(|| b.as_ref().map(|b| b.runtime_command_id.clone())),
        binding: b,
    })
}
async fn oldest(db: &mut AsyncPgConnection, id: &str, revision: i64) -> Result<Option<Queued>> {
    Ok(sql_query("SELECT q.* FROM hagency_agent_v1.management_shared_intake q LEFT JOIN hagency_agent_v1.management_shared_demands d ON d.intake_id=q.id WHERE q.assignment_id=$1 AND q.control_revision=$2 AND (q.state='pending' OR q.state='blocked') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings b WHERE b.demand_id=d.id) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_terminals t WHERE t.demand_id=d.id) ORDER BY q.received_at_ms,q.id LIMIT 1")
        .bind::<Text,_>(id).bind::<BigInt,_>(revision).get_result(db).await.optional()?)
}
async fn block(
    db: &mut AsyncPgConnection,
    d: &Demand,
    pin: &SharedExecutionBudget,
    reason: &str,
    payload: &str,
) -> Result<()> {
    let old=sql_query("SELECT payload,digest FROM hagency_agent_v1.management_shared_budget_demand_blocks WHERE demand_id=$1 AND budget_revision=$2")
        .bind::<Text,_>(&d.id).bind::<BigInt,_>(pin.budget_revision).get_result::<Pin>(db).await.optional()?;
    if let Some(old) = old {
        if old.payload != payload {
            return Err(Error::Conflict("shared_demand_block_changed"));
        }
        return Ok(());
    }
    sql_query("INSERT INTO hagency_agent_v1.management_shared_budget_demand_blocks VALUES($1,$2,$3,$4,$5)")
        .bind::<Text,_>(&d.id).bind::<BigInt,_>(pin.budget_revision).bind::<Text,_>(reason).bind::<Text,_>(payload).bind::<Text,_>(hash(payload)).execute(db).await?;
    Ok(())
}
// Always enforce retained control history, even with issuance disabled.
pub(in super::super) async fn current(
    db: &mut AsyncPgConnection,
    r: &Runtime,
    now: i64,
) -> Result<bool> {
    Ok(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_intake q ON q.id=d.intake_id JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=q.assignment_id WHERE (EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_starts s WHERE s.demand_id=d.id AND s.runtime_id=$1) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings b WHERE b.demand_id=d.id AND b.runtime_id=$1)) AND (c.mode<>'running' OR c.revision<>q.control_revision OR q.state NOT IN('pending','blocked','bound') OR (q.expires_at_ms<=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings b WHERE b.demand_id=d.id)) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_terminals t WHERE t.demand_id=d.id))) AS matched")
        .bind::<Text,_>(&r.id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
pub(in super::super) async fn linked(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_starts WHERE runtime_id=$1 UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE runtime_id=$1) AS matched")
        .bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
impl DomainStore {
    pub async fn poll_shared_budget_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<SharedBudgetDemandPoll> {
        enabled_budget(policy)?;
        if let Some(a) = &input.after {
            key(a)?;
        }
        let mut g = self.db.lock().await?;
        (*g).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            Self::authorize(db,p,0).await?;let(device,generation)=device(p)?;let now=clock(db).await?;
            let ids=sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=a.id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND a.executor_generation=$3 AND c.mode='running' AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands bc JOIN hagency_agent_v1.management_agent_budget_results br ON br.command_id=bc.id WHERE bc.request_id=a.request_id AND br.state='applied') AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake q WHERE q.assignment_id=a.id AND q.control_revision=c.revision AND q.state IN('pending','blocked')) ORDER BY (a.id<=$4),a.id LIMIT 16")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<BigInt,_>(generation).bind::<Text,_>(input.after.as_deref().unwrap_or("")).load::<Id>(db).await?;
            let mut after=None;
            for id in ids {after=Some(id.id.clone());let revision=intake::running_revision(db,&id.id).await?;
                // Bounded expired-head draining; the next page resumes without
                // replacing event IDs or spinning forever on one expired block.
                for _ in 0..16 {
                    let Some(q)=oldest(db,&id.id,revision).await? else{break};
                    let w=authorize(db,p,&q,&self.server,now,true).await?;let Some(h)=head(db,&id.id).await? else{break};
                    let existing=sql_query("SELECT * FROM hagency_agent_v1.management_shared_demands WHERE intake_id=$1").bind::<Text,_>(&q.id).get_result::<Demand>(db).await.optional()?;
                    if q.expires_at_ms<=now {
                        if let Some(d)=existing{terminal(db,&d,"expired").await?;}else{intake::expire(db,&id.id,now).await?;}continue;
                    }
                    let d=if let Some(d)=existing{d}else{ensure(db,&q,&h).await?};let o=offer(&d,&q)?;
                    if super::started(db,&d.id).await?.is_some()||super::binding(db,&d.id).await?.is_some(){break;}
                    let (pin,_,known)=super::super::super::super::budget::execution_head(db,&w.request_id,&self.server).await?;
                    if started(db,&d.id).await?.is_none(){save_pin(db,&d,&pin).await?;}
                    let s=started(db,&d.id).await?;
                    let expected=s.as_ref().map_or(o.predecessor_command_id.as_str(),|v|v.runtime_id.as_str());
                    if h.id!=expected || s.is_some()&&h.state=="stopped" {terminal(db,&d,"head_changed").await?;continue;}
                    if original_block(db,&d.id).await?.is_some_and(|b|b.reason!="budget_blocked"){break;}
                    if s.is_none()&&known>=pin.effective_ceiling_tokens {
                        let payload=wire(&json!({"reason":"known_consumption_exhausted","executionBudget":pin}))?;block(db,&d,&pin,"budget_blocked",&payload).await?;break;
                    }
                    // An old live process is never upgraded in place.
                    if s.is_none()&&h.state!="stopped"&&runtime_version(&h)?!=3 {break;}
                    let value=status(db,&d).await?;
                    if matches!(value.state.as_str(),"offered"|"start_committed"){return Ok(SharedBudgetDemandPoll{demand:Some(value),next_after:after});}
                    break;
                }
            }
            Ok(SharedBudgetDemandPoll{demand:None,next_after:after})
        }).await
    }
    pub async fn recover_shared_budget_demand(
        &self,
        p: &Principal,
        id: &str,
        input: SharedDemandRecoverInput,
    ) -> Result<SharedBudgetDemandStatus> {
        key(id)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, 0).await?;
            let d = load(db, id).await?;
            let q = queued(db, &d.intake_id).await?;
            let now = clock(db).await?;
            authorize(db, p, &q, &self.server, now, false).await?;
            if d.digest != input.demand_digest {
                return Err(Error::Conflict("shared_demand_changed"));
            }
            status(db, &d).await
        })
        .await
    }
    pub async fn shared_budget_demand_work_binding(
        &self,
        p: &Principal,
        id: &str,
        input: SharedDemandWorkInput,
    ) -> Result<Option<SharedDemandWorkBinding>> {
        key(id)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            Self::authorize(db,p,0).await?;let row=sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_work WHERE id=$1").bind::<Text,_>(id).get_result::<Started>(db).await.map_err(hidden)?;let r=runtime(db,&row.runtime_id).await?;owner(&original(db,&r).await?,p)?;WorkProtocol::Budget.check(&r)?;
            if row.digest!=input.work_digest||r.digest!=input.runtime_command_digest{return Err(Error::Conflict("shared_work_changed"));}
            let did=sql_query("SELECT demand_id AS id FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE work_id=$1").bind::<Text,_>(id).get_result::<Id>(db).await.optional()?;let Some(did)=did else{return Ok(None)};let d=load(db,&did.id).await?;let q=queued(db,&d.intake_id).await?;
            Ok(Some(SharedDemandWorkBinding{offer:offer(&d,&q)?,demand_digest:d.digest.clone(),binding:binding(db,&d.id).await?.ok_or(Error::Conflict("shared_binding_missing"))?}))
        }).await
    }
}
/// Called in the existing live work-poll transaction, after current Matrix
/// observation. Only one work item can ever be attached to a one-turn process.
pub(in super::super) async fn promote(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    r: &Runtime,
    server: &str,
    now: i64,
) -> Result<()> {
    if !policy.shared_demands() || !policy.shared_durable_intake() {
        return Ok(());
    }
    enabled_budget(policy)?;
    let revision = intake::running_revision(db, &r.assignment_id).await?;
    intake::expire(db, &r.assignment_id, now).await?;
    let Some(q) = oldest(db, &r.assignment_id, revision).await? else {
        return Ok(());
    };
    if !matches!(q.state.as_str(), "pending" | "blocked") || q.expires_at_ms <= now {
        return Ok(());
    }
    let w = authorize(db, p, &q, server, now, true).await?;
    if head(db, &r.assignment_id)
        .await?
        .is_none_or(|h| h.id != r.id)
    {
        return Err(Error::Conflict("shared_attempt_head_changed"));
    }
    let used=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1) AS matched").bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched;
    if used {
        return Ok(());
    }
    let d = ensure(db, &q, r).await?;
    let pin = budget_attempts::attempt_command(r, server)?.execution_budget;
    if started(db, &d.id).await?.is_none() {
        save_pin(db, &d, &pin).await?;
    }
    let o = offer(&d, &q)?;
    let prior = started(db, &d.id).await?;
    if prior
        .as_ref()
        .map_or(o.predecessor_command_id.as_str(), |s| s.runtime_id.as_str())
        != r.id
        || super::started(db, &d.id).await?.is_some()
        || super::binding(db, &d.id).await?.is_some()
        || original_block(db, &d.id)
            .await?
            .is_some_and(|b| b.reason != "budget_blocked")
    {
        return Err(Error::Conflict("shared_demand_not_current"));
    }
    let s = status(db, &d).await?;
    if !matches!(s.state.as_str(), "offered" | "start_committed") {
        return Ok(());
    }
    let _ = w;
    if known(db, &r.assignment_id).await? >= pin.effective_ceiling_tokens {
        block(
            db,
            &d,
            &pin,
            "budget_blocked",
            "\"known_consumption_exhausted\"",
        )
        .await?;
        return Ok(());
    }
    let event: RetainedEvent = decode(&q.event_payload)?;
    let m = membership(db, r).await?;
    if allocation_digest(&event)? != q.event_digest
        || event.event_id != q.event_id
        || event.room_id != m.room_id
        || !m.addressed_by(&event.sender_mxid, &event.mentioned_mxids)
        || event.encrypted
        || event.is_edit
    {
        return Err(Error::Conflict("shared_event_changed"));
    }
    let v = SharedWorkItem {
        id: format!("swork_{}", entity_id()?),
        assignment_id: r.assignment_id.clone(),
        runtime_command_digest: r.digest.clone(),
        event_id: event.event_id.clone(),
        room_id: event.room_id,
        requester_mxid: event.sender_mxid,
        thread_root: event.thread_root.unwrap_or(event.event_id),
        body_digest: hash(&event.body),
        body: event.body,
        execution_id: format!("sx_{}", entity_id()?),
        created_at_ms: q.received_at_ms,
    };
    v.validate()?;
    let digest = allocation_digest(&v)?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_work(id,runtime_id,event_id,payload,digest,created_at_ms) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&v.id).bind::<Text,_>(&r.id).bind::<Text,_>(&v.event_id).bind::<Text,_>(wire(&v)?).bind::<Text,_>(&digest).bind::<BigInt,_>(v.created_at_ms).execute(db).await?;
    work::context::register_mode(db, policy, &v.id).await?;
    let b = SharedDemandBinding {
        version: 1,
        demand_id: d.id.clone(),
        demand_digest: d.digest.clone(),
        control_revision: q.control_revision,
        assignment_id: q.assignment_id,
        event_id: q.event_id,
        received_at_ms: q.received_at_ms,
        runtime_command_id: r.id.clone(),
        runtime_command_digest: r.digest.clone(),
        work_id: v.id.clone(),
        work_digest: digest,
    };
    sql_query(
        "INSERT INTO hagency_agent_v1.management_shared_budget_demand_bindings(demand_id,work_id,runtime_id,payload,digest,budget_revision) VALUES($1,$2,$3,$4,$5,$6)",
    )
    .bind::<Text, _>(&d.id)
    .bind::<Text, _>(&v.id)
    .bind::<Text, _>(&r.id)
    .bind::<Text, _>(wire(&b)?)
    .bind::<Text, _>(allocation_digest(&b)?)
    .bind::<BigInt,_>(pin.budget_revision)
    .execute(db)
    .await?;
    sql_query("UPDATE hagency_agent_v1.management_shared_intake SET state='bound',reason=NULL WHERE id=$1 AND state='pending'").bind::<Text,_>(&q.id).execute(db).await?;
    Ok(())
}
