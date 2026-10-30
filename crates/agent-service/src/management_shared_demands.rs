//! One immutable retained event/head -> one original native start and work.
use super::*;
use crate::{
    management_shared_attempt_types::SharedAttemptDelivery, management_shared_demand_types::*,
    management_shared_execution_types::SharedWorkItem,
};
#[path = "management_shared_budget_demands.rs"]
pub(super) mod budget;
#[path = "management_shared_demand_start.rs"]
mod start;
// Decode only server-owned retained bytes, never an external AS authority.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedEvent {
    event_id: String,
    room_id: String,
    sender_mxid: String,
    body: String,
    mentioned_mxids: std::collections::BTreeSet<String>,
    thread_root: Option<String>,
    encrypted: bool,
    is_edit: bool,
}
#[derive(QueryableByName)]
struct Queued {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    event_id: String,
    #[diesel(sql_type=Text)]
    event_payload: String,
    #[diesel(sql_type=Text)]
    event_digest: String,
    #[diesel(sql_type=BigInt)]
    control_revision: i64,
    #[diesel(sql_type=BigInt)]
    received_at_ms: i64,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=Text)]
    state: String,
}
#[derive(QueryableByName)]
struct Demand {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    intake_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
#[derive(QueryableByName)]
struct Started {
    #[diesel(sql_type=Text)]
    runtime_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
fn enabled_demand(p: &ManagementPolicy) -> Result<()> {
    attempts::fresh_enabled(p)?;
    if !p.shared_durable_intake() || !p.shared_demands() {
        return Err(Error::Unavailable("shared_demand_issuance_unavailable"));
    }
    Ok(())
}
async fn queued(db: &mut AsyncPgConnection, id: &str) -> Result<Queued> {
    sql_query("SELECT * FROM hagency_agent_v1.management_shared_intake WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn load(db: &mut AsyncPgConnection, id: &str) -> Result<Demand> {
    sql_query("SELECT * FROM hagency_agent_v1.management_shared_demands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn offer(d: &Demand, q: &Queued) -> Result<SharedDemandOffer> {
    let o: SharedDemandOffer = decode(&d.payload)?;
    o.validate()?;
    if o.demand_id != d.id
        || o.assignment_id != q.assignment_id
        || o.event_id != q.event_id
        || o.event_digest != q.event_digest
        || o.control_revision != q.control_revision
        || o.received_at_ms != q.received_at_ms
        || allocation_digest(&o)? != d.digest
    {
        return Err(Error::Conflict("shared_demand_changed"));
    }
    Ok(o)
}
async fn started(db: &mut AsyncPgConnection, id: &str) -> Result<Option<Started>> {
    Ok(sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=$1").bind::<Text,_>(id).get_result(db).await.optional()?)
}
pub(super) async fn binding(
    db: &mut AsyncPgConnection,
    id: &str,
) -> Result<Option<SharedDemandBinding>> {
    let row = sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=$1")
        .bind::<Text,_>(id).get_result::<Started>(db).await.optional()?;
    let Some(row) = row else { return Ok(None) };
    let value: SharedDemandBinding = decode(&row.payload)?;
    if value.demand_id != id
        || value.runtime_command_id != row.runtime_id
        || allocation_digest(&value)? != row.digest
    {
        return Err(Error::Conflict("shared_binding_changed"));
    }
    Ok(Some(value))
}
async fn head(db: &mut AsyncPgConnection, id: &str) -> Result<Option<Runtime>> {
    let i=sql_query("SELECT id FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=$1 ORDER BY coalesce((payload::jsonb->>'processAttempt')::bigint,1) DESC LIMIT 1").bind::<Text,_>(id).get_result::<Id>(db).await.optional()?;
    match i {
        Some(i) => Ok(Some(runtime(db, &i.id).await?)),
        None => Ok(None),
    }
}
async fn oldest(db: &mut AsyncPgConnection, id: &str, revision: i64) -> Result<Option<Queued>> {
    Ok(sql_query("SELECT q.* FROM hagency_agent_v1.management_shared_intake q WHERE q.assignment_id=$1 AND q.control_revision=$2 AND (q.state='pending' OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_blocks b ON b.demand_id=d.id WHERE d.intake_id=q.id)) ORDER BY received_at_ms,id LIMIT 1").bind::<Text,_>(id).bind::<BigInt,_>(revision).get_result(db).await.optional()?)
}
async fn known(db: &mut AsyncPgConnection, id: &str) -> Result<i64> {
    Ok(sql_query("SELECT LEAST(coalesce(sum(w.known_consumed_tokens),0),9007199254740992)::bigint AS now FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1").bind::<Text,_>(id).get_result::<Clock>(db).await?.now)
}
async fn block(
    db: &mut AsyncPgConnection,
    d: &Demand,
    q: &Queued,
    reason: &str,
    payload: &str,
) -> Result<()> {
    let old = sql_query(
        "SELECT payload FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1",
    )
    .bind::<Text, _>(&d.id)
    .get_result::<Json>(db)
    .await
    .optional()?;
    if let Some(old) = old {
        if old.payload != payload {
            return Err(Error::Conflict("shared_demand_block_changed"));
        }
        return Ok(());
    }
    sql_query("INSERT INTO hagency_agent_v1.management_shared_demand_blocks VALUES($1,$2,$3,$4)")
        .bind::<Text, _>(&d.id)
        .bind::<Text, _>(reason)
        .bind::<Text, _>(payload)
        .bind::<Text, _>(hash(payload))
        .execute(db)
        .await?;
    if q.state == "pending" {
        sql_query("UPDATE hagency_agent_v1.management_shared_intake SET state='blocked',reason=$2 WHERE id=$1").bind::<Text,_>(&q.id).bind::<Text,_>(reason).execute(db).await?;
    }
    Ok(())
}
async fn status(db: &mut AsyncPgConnection, d: &Demand) -> Result<SharedDemandStatus> {
    let q = queued(db, &d.intake_id).await?;
    let o = offer(d, &q)?;
    let s = started(db, &d.id).await?;
    let b = binding(db, &d.id).await?;
    let reason=sql_query("SELECT reason AS payload FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1").bind::<Text,_>(&d.id).get_result::<Json>(db).await.optional()?;
    let state = if b.is_some() {
        "bound".into()
    } else if let Some(r) = reason {
        r.payload
    } else if q.state != "pending" {
        q.state
    } else if s.is_some() {
        "start_committed".into()
    } else {
        "offered".into()
    };
    Ok(SharedDemandStatus {
        offer: o,
        demand_digest: d.digest.clone(),
        state,
        runtime_command_id: s
            .map(|s| s.runtime_id)
            .or_else(|| b.as_ref().map(|b| b.runtime_command_id.clone())),
        binding: b,
    })
}
async fn authorize(
    db: &mut AsyncPgConnection,
    p: &Principal,
    q: &Queued,
    server: &str,
    now: i64,
    fresh: bool,
) -> Result<Work> {
    let w = intake::assignment(db, &q.assignment_id).await?;
    owner(&w, p)?;
    if fresh
        && (p.device_generation != Some(w.executor_generation)
            || !intake::retainable(db, &q.assignment_id, &w, server, now).await?
            || intake::running_revision(db, &q.assignment_id).await? != q.control_revision)
    {
        return Err(Error::Conflict("shared_demand_authority_changed"));
    }
    Ok(w)
}
async fn ensure(db: &mut AsyncPgConnection, q: &Queued, h: &Runtime) -> Result<Demand> {
    let old =
        sql_query("SELECT * FROM hagency_agent_v1.management_shared_demands WHERE intake_id=$1")
            .bind::<Text, _>(&q.id)
            .get_result::<Demand>(db)
            .await
            .optional()?;
    if let Some(d) = old {
        offer(&d, q)?;
        return Ok(d);
    }
    let id = format!(
        "sdm_{}",
        q.id.strip_prefix("sin_")
            .ok_or(Error::Conflict("shared_intake_changed"))?
    );
    let o = SharedDemandOffer {
        version: 1,
        demand_id: id.clone(),
        assignment_id: q.assignment_id.clone(),
        event_id: q.event_id.clone(),
        event_digest: q.event_digest.clone(),
        control_revision: q.control_revision,
        received_at_ms: q.received_at_ms,
        predecessor_command_id: h.id.clone(),
        predecessor_command_digest: h.digest.clone(),
    };
    o.validate()?;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_demands VALUES($1,$2,$3,$4)")
        .bind::<Text, _>(&id)
        .bind::<Text, _>(&q.id)
        .bind::<Text, _>(wire(&o)?)
        .bind::<Text, _>(allocation_digest(&o)?)
        .execute(db)
        .await?;
    load(db, &id).await
}
async fn refresh(db: &mut AsyncPgConnection, d: &Demand, q: &Queued, h: &Runtime) -> Result<()> {
    if q.state != "pending" || binding(db, &d.id).await?.is_some() {
        return Ok(());
    }
    let o = offer(d, q)?;
    let s = started(db, &d.id).await?;
    let expected = s
        .as_ref()
        .map(|s| s.runtime_id.as_str())
        .unwrap_or(&o.predecessor_command_id);
    if h.id != expected || (s.is_some() && h.state == "stopped") {
        block(db, d, q, "head_changed", "\"head_changed\"").await?;
    }
    Ok(())
}
// Restriction remains effective when the feature is disabled; old linked
// processes/work cannot fall back to legacy authority after Stop or rearming.
pub(super) async fn current(db: &mut AsyncPgConnection, r: &Runtime, now: i64) -> Result<bool> {
    if !budget::current(db, r, now).await? {
        return Ok(false);
    }
    Ok(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_intake q ON q.id=d.intake_id JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=q.assignment_id WHERE (EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_starts s WHERE s.demand_id=d.id AND s.runtime_id=$1) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings b WHERE b.demand_id=d.id AND b.runtime_id=$1)) AND (c.mode<>'running' OR c.revision<>q.control_revision OR q.state NOT IN('pending','bound') OR (q.state='pending' AND q.expires_at_ms<=$2) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_blocks k WHERE k.demand_id=d.id))) AS matched").bind::<Text,_>(&r.id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
pub(super) async fn allowed(
    db: &mut AsyncPgConnection,
    policy: &ManagementPolicy,
    r: &Runtime,
) -> Result<bool> {
    if policy.shared_demands() && policy.shared_durable_intake() {
        return Ok(true);
    }
    if budget::linked(db, &r.id).await? {
        return Ok(false);
    }
    Ok(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_starts WHERE runtime_id=$1 UNION ALL SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings WHERE runtime_id=$1) AS matched").bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched)
}
impl DomainStore {
    pub async fn poll_shared_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<SharedDemandPoll> {
        enabled_demand(policy)?;
        if let Some(a) = &input.after {
            key(a)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
 Self::authorize(db,p,0).await?;let (device,generation)=device(p)?;let now=clock(db).await?;
 let ids=sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=a.id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND a.executor_generation=$3 AND c.mode='running' AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake q WHERE q.assignment_id=a.id AND q.control_revision=c.revision AND q.state='pending') ORDER BY (a.id<=$4),a.id LIMIT 16").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<BigInt,_>(generation).bind::<Text,_>(input.after.as_deref().unwrap_or("")).load::<Id>(db).await?;
 let mut after=None;for id in ids {after=Some(id.id.clone());intake::expire(db,&id.id,now).await?;let revision=intake::running_revision(db,&id.id).await?;let Some(q)=oldest(db,&id.id,revision).await? else{continue};if q.state!="pending"{continue}let w=match authorize(db,p,&q,&self.server,now,true).await {Ok(w)=>w,Err(Error::Conflict(_)|Error::NotFound(_))=>continue,Err(e)=>return Err(e)};let Some(h)=head(db,&id.id).await? else{continue};if runtime_version(&h)?==3 || has_applied_budget(db,&w.request_id).await? {continue;}let d=ensure(db,&q,&h).await?;refresh(db,&d,&q,&h).await?;if status(db,&d).await?.state!="offered" && started(db,&d.id).await?.is_none(){continue}let c:ManagedAgentReserveCommand=decode(&w.payload)?;if known(db,&id.id).await?>=c.requested_tokens && started(db,&d.id).await?.is_none(){block(db,&d,&q,"budget_blocked","\"known_consumption_exhausted\"").await?;}let value=status(db,&d).await?;if matches!(value.state.as_str(),"offered"|"start_committed"){return Ok(SharedDemandPoll{demand:Some(value),next_after:after})}}
 Ok(SharedDemandPoll{demand:None,next_after:after})
 }).await
    }
    pub async fn recover_shared_demand(
        &self,
        p: &Principal,
        id: &str,
        input: SharedDemandRecoverInput,
    ) -> Result<SharedDemandStatus> {
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
            intake::expire(db, &q.assignment_id, now).await?;
            status(db, &d).await
        })
        .await
    }
    pub async fn shared_demand_work_binding(
        &self,
        p: &Principal,
        id: &str,
        input: SharedDemandWorkInput,
    ) -> Result<Option<SharedDemandWorkBinding>> {
        key(id)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,0).await?;let row=sql_query("SELECT w.runtime_id,w.payload,w.digest FROM hagency_agent_v1.management_shared_work w WHERE w.id=$1").bind::<Text,_>(id).get_result::<Started>(db).await.map_err(hidden)?;let r=runtime(db,&row.runtime_id).await?;owner(&original(db,&r).await?,p)?;WorkProtocol::Legacy.check(&r)?;if row.digest!=input.work_digest||r.digest!=input.runtime_command_digest{return Err(Error::Conflict("shared_work_changed"));}let did=sql_query("SELECT demand_id AS id FROM hagency_agent_v1.management_shared_demand_bindings WHERE work_id=$1").bind::<Text,_>(id).get_result::<Id>(db).await.optional()?;let Some(did)=did else{return Ok(None)};let d=load(db,&did.id).await?;let q=queued(db,&d.intake_id).await?;Ok(Some(SharedDemandWorkBinding{offer:offer(&d,&q)?,demand_digest:d.digest.clone(),binding:binding(db,&d.id).await?.ok_or(Error::Conflict("shared_binding_missing"))?}))}).await
    }
}
/// Called in the existing live work-poll transaction, after current Matrix
/// observation. Only one work item can ever be attached to a one-turn process.
pub(super) async fn promote(
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
    enabled_demand(policy)?;
    let revision = intake::running_revision(db, &r.assignment_id).await?;
    intake::expire(db, &r.assignment_id, now).await?;
    let Some(q) = oldest(db, &r.assignment_id, revision).await? else {
        return Ok(());
    };
    if q.state != "pending" {
        return Ok(());
    }
    let w = authorize(db, p, &q, server, now, true).await?;
    if runtime_version(r)? == 3 || has_applied_budget(db, &w.request_id).await? {
        return Ok(());
    }
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
    refresh(db, &d, &q, r).await?;
    let s = status(db, &d).await?;
    if !matches!(s.state.as_str(), "offered" | "start_committed") {
        return Ok(());
    }
    let c: ManagedAgentReserveCommand = decode(&w.payload)?;
    if known(db, &r.assignment_id).await? >= c.requested_tokens {
        block(
            db,
            &d,
            &q,
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
        || event.is_edit
    {
        return Err(Error::Conflict("shared_event_changed"));
    }
    let v = SharedWorkItem {
        encrypted: event.encrypted,
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
        "INSERT INTO hagency_agent_v1.management_shared_demand_bindings VALUES($1,$2,$3,$4,$5)",
    )
    .bind::<Text, _>(&d.id)
    .bind::<Text, _>(&v.id)
    .bind::<Text, _>(&r.id)
    .bind::<Text, _>(wire(&b)?)
    .bind::<Text, _>(allocation_digest(&b)?)
    .execute(db)
    .await?;
    sql_query("UPDATE hagency_agent_v1.management_shared_intake SET state='bound',reason=NULL WHERE id=$1").bind::<Text,_>(&q.id).execute(db).await?;
    Ok(())
}

async fn has_applied_budget(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands c JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.request_id=$1 AND r.state='applied') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
