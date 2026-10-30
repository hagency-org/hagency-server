//! Assignment-scoped evidence intake. There is deliberately no demand consumer.
use super::*;
use crate::{management_shared_intake_types::*, transport::RoutedEvent};
use diesel::sql_types::Array;
const TTL: i64 = 86_400_000;
#[derive(QueryableByName)]
struct Control {
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=Text)]
    mode: String,
    #[diesel(sql_type=Nullable<BigInt>)]
    running_since_ms: Option<i64>,
    #[diesel(sql_type=BigInt)]
    updated_at_ms: i64,
    #[diesel(sql_type=Text)]
    origin: String,
}
impl Control {
    fn view(self) -> SharedRunControl {
        SharedRunControl {
            assignment_id: self.assignment_id,
            revision: self.revision,
            mode: self.mode,
            running_since_ms: self.running_since_ms,
            updated_at_ms: self.updated_at_ms,
            origin: self.origin,
        }
    }
}
#[derive(QueryableByName)]
struct Intake {
    #[diesel(sql_type=Text)]
    id: String,
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
    #[diesel(sql_type=Nullable<Text>)]
    reason: Option<String>,
}
impl Intake {
    fn view(self) -> SharedIntakeRecord {
        SharedIntakeRecord {
            id: self.id,
            event_id: self.event_id,
            control_revision: self.control_revision,
            state: self.state,
            reason: self.reason,
            received_at_ms: self.received_at_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }
}
async fn control(db: &mut AsyncPgConnection, id: &str) -> Result<Control> {
    sql_query("SELECT * FROM hagency_agent_v1.management_shared_controls WHERE assignment_id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
pub(crate) async fn initialize(
    db: &mut AsyncPgConnection,
    id: &str,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<()> {
    let armed = policy.shared_durable_intake();
    sql_query("INSERT INTO hagency_agent_v1.management_shared_controls(assignment_id,revision,mode,running_since_ms,updated_at_ms,origin) VALUES($1,1,$2,$3,$4,$5)").bind::<Text,_>(id).bind::<Text,_>(if armed {"running"}else{"manual"}).bind::<Nullable<BigInt>,_>(armed.then_some(now)).bind::<BigInt,_>(now).bind::<Text,_>(if armed {"approved_native_assignment"}else{"legacy_manual"}).execute(db).await?;
    Ok(())
}
pub(super) async fn paused(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls WHERE assignment_id=$1 AND mode='paused') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
pub(super) async fn running(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(control(db, id).await?.mode == "running")
}
pub(super) async fn running_revision(db: &mut AsyncPgConnection, id: &str) -> Result<i64> {
    let current = control(db, id).await?;
    if current.mode != "running" {
        return Err(Error::Conflict("shared_assignment_not_running"));
    }
    Ok(current.revision)
}
pub(super) async fn assignment(db: &mut AsyncPgConnection, id: &str) -> Result<Work> {
    sql_query(format!("{WORK} JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=i.request_id WHERE a.id=$1 AND w.operation='agent.reserve'")).bind::<Text,_>(id).get_result(db).await.map_err(hidden)
}
async fn member(db: &mut AsyncPgConnection, id: &str) -> Result<Membership> {
    sql_query("SELECT m.room_id,m.space_id,m.puppet_mxid,u.mxid AS manager_mxid,q.pool_contribution_id IS NOT NULL AS pool_agent FROM hagency_agent_v1.management_agent_matrix m JOIN hagency_agent_v1.management_shared_assignments a ON a.id=m.assignment_id JOIN hagency_agent_v1.management_agent_requests q ON q.id=a.request_id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE a.id=$1 AND m.state='matrix_verified'").bind::<Text,_>(id).get_result(db).await.optional()?.ok_or(Error::NotFound("shared_matrix_not_verified"))
}
// Retention is not execution authorization. Deliberately ignore only transient
// executor-session expiry; revoked/replaced identity and all durable pins remain.
pub(super) async fn retainable(
    db: &mut AsyncPgConnection,
    id: &str,
    w: &Work,
    server: &str,
    now: i64,
) -> Result<bool> {
    let c: ManagedAgentReserveCommand = decode(&w.payload)?;
    c.validate(server)?;
    if allocation_digest(&c)? != w.digest
        || c.command_id != w.intent_id
        || w.intent_state == "fenced"
        || reserve_lifetime(db, &c, now).await? <= now
    {
        return Ok(false);
    }
    if c.pool_grant.is_some() {
        let r=row(db,&w.request_id).await?;
        if r.state!="approved_waiting_executor" || r.revision!=c.request_revision || !pool::command_live(db,&r,&c,now).await? {return Ok(false);}
        return Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.devices d ON d.id=a.executor_device_id JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id JOIN hagency_agent_v1.management_contributions co ON co.id=r.pool_contribution_id WHERE a.id=$1 AND a.state='pending_native' AND d.user_id=a.executor_owner_user_id AND d.generation=a.executor_generation AND NOT d.revoked AND co.runtime='codex') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched);
    }
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id JOIN hagency_agent_v1.management_project_authorities pa ON pa.plan_id=p.id JOIN hagency_agent_v1.users adm ON adm.id=pa.administrator_user_id JOIN hagency_agent_v1.users mgr ON mgr.id=r.manager_user_id JOIN hagency_agent_v1.management_project_setups s ON s.id=r.setup_id JOIN hagency_agent_v1.management_setup_bindings b ON b.setup_id=s.id AND b.revision=s.current_binding_revision JOIN hagency_agent_v1.management_allocations al ON al.id=r.allocation_id JOIN hagency_agent_v1.management_resource_grants g ON g.id=al.grant_id JOIN hagency_agent_v1.management_contributions co ON co.id=g.contribution_id JOIN hagency_agent_v1.devices d ON d.id=a.executor_device_id JOIN hagency_agent_v1.sessions ds ON ds.id=d.session_id JOIN hagency_agent_v1.users own ON own.id=a.executor_owner_user_id JOIN hagency_agent_v1.projects proj ON proj.id=r.domain_project_id JOIN hagency_agent_v1.rooms rm ON rm.room_id=r.room_id AND rm.project_id=proj.id WHERE a.id=$1 AND a.state='pending_native' AND r.state='approved_waiting_executor' AND r.revision=$2 AND r.authority_revision=$3 AND pa.revision=$3 AND pa.administrator_user_id=r.administrator_user_id AND adm.active AND mgr.active AND own.active AND p.state='approved' AND p.manager_user_id=r.manager_user_id AND s.state='ready' AND b.revision=$4 AND b.authority_revision=$3 AND b.allocation_id=al.id AND b.allocation_receipt_digest=r.allocation_receipt_digest AND al.state='reserved' AND al.release_command_id IS NULL AND al.expires_at_ms>$5 AND g.state='active' AND g.expires_at_ms>$5 AND g.receipt_digest=$6 AND co.state='admitted' AND co.runtime='codex' AND co.resource_id=$7 AND co.resource_revision=$8 AND co.executor_device_id=a.executor_device_id AND co.executor_generation=a.executor_generation AND g.executor_device_id=a.executor_device_id AND g.executor_generation=a.executor_generation AND d.user_id=a.executor_owner_user_id AND d.generation=a.executor_generation AND NOT d.revoked AND NOT ds.revoked AND proj.active AND rm.active) AS matched")
 .bind::<Text,_>(id).bind::<BigInt,_>(c.request_revision).bind::<BigInt,_>(c.authority_revision).bind::<BigInt,_>(c.setup_binding_revision).bind::<BigInt,_>(now).bind::<Text,_>(&c.grant_receipt_digest).bind::<Text,_>(&c.resource_id).bind::<Text,_>(&c.resource_revision).get_result::<Flag>(db).await?.matched)
}
async fn rights(
    db: &mut AsyncPgConnection,
    p: &Principal,
    w: &Work,
) -> Result<(bool, bool, &'static str)> {
    let r = row(db, &w.request_id).await?;
    if r.pool_contribution_id.is_some() {
        // Keep the existing persisted control-origin vocabulary: the applicant
        // manages this Agent, and its resource owner can only stop execution.
        if p.user_id==r.manager_user_id {return Ok((true,true,"manager"));}
        if p.user_id==r.administrator_user_id {return Ok((false,true,"executor_stop"));}
        return Err(Error::NotFound("shared_assignment_not_found"));
    }
    let roles = roles(db, &r.plan_id).await?;
    if p.user_id == r.manager_user_id {
        return Ok((true, true, "manager"));
    }
    if roles.active && roles.administrator_user_id == p.user_id {
        return Ok((true, true, "project_administrator"));
    }
    if owner(w, p).is_ok() {
        return Ok((false, true, "executor_stop"));
    }
    Err(Error::NotFound("shared_assignment_not_found"))
}
pub(super) async fn expire(db: &mut AsyncPgConnection, id: &str, now: i64) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_shared_intake SET state='expired',reason='event_expired' WHERE assignment_id=$1 AND state='pending' AND expires_at_ms<=$2").bind::<Text,_>(id).bind::<BigInt,_>(now).execute(db).await?;
    Ok(())
}
async fn replay(
    db: &mut AsyncPgConnection,
    p: &Principal,
    id: &str,
    input: &SharedControlInput,
    digest: &str,
) -> Result<Option<SharedControlResult>> {
    let (device, _) = device(p)?;
    #[derive(QueryableByName)]
    struct Saved {
        #[diesel(sql_type=Text)]
        assignment_id: String,
        #[diesel(sql_type=Text)]
        input_digest: String,
        #[diesel(sql_type=Text)]
        result: String,
    }
    let saved=sql_query("SELECT assignment_id,input_digest,result FROM hagency_agent_v1.management_shared_control_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(&input.command_id).get_result::<Saved>(db).await.optional()?;
    match saved {
        None => Ok(None),
        Some(s) => {
            if s.assignment_id != id || s.input_digest != digest {
                return Err(Error::Conflict("shared_control_command_conflict"));
            }
            let mut v: SharedControlResult = decode(&s.result)?;
            v.control = control(db, id).await?.view();
            Ok(Some(v))
        }
    }
}
impl DomainStore {
    pub async fn shared_run_control_readiness(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<crate::management_shared_demand_types::SharedRunControlReadiness> {
        key(id)?;
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let w = assignment(db, id).await?;
                let (run, stop, _) = rights(db, p, &w).await?;
                let control = control(db, id).await?.view();
                let now = clock(db).await?;
                let reason = if !run {
                    Some("role_required")
                } else if !policy.shared_durable_intake() {
                    Some("intake_disabled")
                } else if !retainable(db, id, &w, &self.server, now).await? {
                    Some("assignment_authority_changed")
                } else if control.mode == "running" {
                    Some("already_running")
                } else {
                    None
                };
                Ok(
                    crate::management_shared_demand_types::SharedRunControlReadiness {
                        control,
                        can_run: reason.is_none(),
                        can_stop: stop,
                        requires_local_stop: owner(&w, p).is_ok(),
                        run_blocking_reason: reason.map(str::to_owned),
                        intake_enabled: policy.shared_durable_intake(),
                        automatic_delivery_enabled: policy.shared_durable_intake()
                            && policy.shared_demands()
                            && policy.shared_successors()
                            && issuing(policy),
                    },
                )
            })
            .await
    }
    pub async fn shared_run_control(&self, p: &Principal, id: &str) -> Result<SharedRunControl> {
        key(id)?;
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let w = assignment(db, id).await?;
                rights(db, p, &w).await?;
                Ok(control(db, id).await?.view())
            })
            .await
    }
    pub async fn change_shared_run_control(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedControlInput,
        gateway: &Gateway,
    ) -> Result<SharedControlResult> {
        key(id)?;
        input.validate()?;
        let digest = allocation_digest(&input)?;
        let observed = if input.mode == "running" {
            let m = {
                let mut guard = self.db.lock().await?;
                let prepared = (*guard)
                    .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                        Self::authorize(db, p, 0).await?;
                        let w = assignment(db, id).await?;
                        let (start, _, _) = rights(db, p, &w).await?;
                        if let Some(saved) = replay(db, p, id, &input, &digest).await? {
                            return Ok(Err(saved));
                        }
                        if !start {
                            return Err(Error::Unauthorized(
                                "shared_run_manager_or_project_administrator_required",
                            ));
                        }
                        if !policy.shared_durable_intake() {
                            return Err(Error::Unavailable("shared_durable_intake_unavailable"));
                        }
                        let now = clock(db).await?;
                        if !retainable(db, id, &w, &self.server, now).await? {
                            return Err(Error::Conflict("shared_assignment_authority_changed"));
                        }
                        Ok(Ok(member(db, id).await?))
                    })
                    .await?;
                match prepared {
                    Err(saved) => return Ok(saved),
                    Ok(m) => m,
                }
            };
            Some(
                gateway
                    .shared_delivery(
                        &m.room_id,
                        &m.space_id,
                        &m.manager_mxid,
                        &m.manager_mxid,
                        &m.puppet_mxid,
                    )
                    .await?,
            )
        } else {
            None
        };
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
   Self::authorize(db,p,0).await?;let w=assignment(db,id).await?;let (start,stop,origin)=rights(db,p,&w).await?;
   if let Some(saved)=replay(db,p,id,&input,&digest).await?{return Ok(saved);}
   if input.mode=="running"&&!start||input.mode=="paused"&&!stop{return Err(Error::Unauthorized("shared_run_control_forbidden"));}
   let old=control(db,id).await?;if old.revision!=input.expected_revision{return Err(Error::Conflict("shared_control_revision_changed"));}
   let now=clock(db).await?;if input.mode=="running"{if !policy.shared_durable_intake(){return Err(Error::Unavailable("shared_durable_intake_unavailable"));}
   if !retainable(db,id,&w,&self.server,now).await?{return Err(Error::Conflict("shared_assignment_authority_changed"));}facts(&member(db,id).await?,observed.as_ref().unwrap())?;}
   // Repeated Start does not reset the receive-time floor or discard messages.
   if old.mode!=input.mode { sql_query("UPDATE hagency_agent_v1.management_shared_controls SET revision=revision+1,mode=$2,running_since_ms=$3,updated_at_ms=$4,origin=$5 WHERE assignment_id=$1").bind::<Text,_>(id).bind::<Text,_>(&input.mode).bind::<Nullable<BigInt>,_>((input.mode=="running").then_some(now)).bind::<BigInt,_>(now).bind::<Text,_>(origin).execute(db).await?; }
   let current=control(db,id).await?.view();let result=SharedControlResult{command_id:input.command_id.clone(),applied_revision:current.revision,control:current};
   sql_query("INSERT INTO hagency_agent_v1.management_shared_control_commands(actor_user_id,device_id,command_id,assignment_id,input_digest,result) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&p.user_id).bind::<Text,_>(device(p)?.0).bind::<Text,_>(&input.command_id).bind::<Text,_>(id).bind::<Text,_>(&digest).bind::<Text,_>(wire(&result)?).execute(db).await?;Self::audit(db,p,"shared.run.control",id,now).await?;Ok(result)
  }).await
    }
    pub async fn shared_intake_history(
        &self,
        p: &Principal,
        id: &str,
        q: SharedIntakeQuery,
    ) -> Result<SharedIntakePage> {
        key(id)?;
        if let Some(after) = &q.after {
            key(after)?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,0).await?;let w=assignment(db,id).await?;rights(db,p,&w).await?;let now=clock(db).await?;expire(db,id,now).await?;let mut rows=sql_query("SELECT * FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1 AND ($2::text IS NULL OR id>$2) ORDER BY id LIMIT 17").bind::<Text,_>(id).bind::<Nullable<Text>,_>(q.after.as_deref()).load::<Intake>(db).await?;let more=rows.len()>16;rows.truncate(16);let next_after=more.then(||rows.last().unwrap().id.clone());Ok(SharedIntakePage{items:rows.into_iter().map(Intake::view).collect(),next_after})}).await
    }
    pub(crate) async fn retain_shared_event(
        &self,
        event: &RoutedEvent,
        received: i64,
        gateway: &Gateway,
    ) -> Result<()> {
        if event.encrypted
            || event.is_edit
            || event.body.is_empty()
            || event.body.len() > MAX_SHARED_TEXT
        {
            return Ok(());
        }
        if received <= 0
            || !event.event_id.starts_with('$')
            || event.event_id.len() > 255
            || !event.room_id.starts_with('!')
            || event.room_id.len() > 255
            || !event.sender_mxid.starts_with('@')
            || event.sender_mxid.len() > 255
            || event
                .thread_root
                .as_ref()
                .is_some_and(|s| !s.starts_with('$') || s.len() > 255)
        {
            return Err(Error::Invalid("invalid_shared_intake_event"));
        }
        let payload = serde_json::to_string(event)
            .map_err(|_| Error::Invalid("invalid_shared_intake_event"))?;
        let digest = allocation_digest(event)?;
        let mut after = String::new();
        let mut retry = false;
        loop {
            let ids = {
                let mut db = self.db.lock().await?;
                let conflict=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake WHERE event_id=$1 AND event_digest<>$2) AS matched").bind::<Text,_>(&event.event_id).bind::<Text,_>(&digest).get_result::<Flag>(&mut *db).await?.matched;
                if conflict {
                    return Err(Error::Conflict("shared_event_changed"));
                }
                let mentions = event
                    .mentioned_mxids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=a.id WHERE a.id>$5 AND m.room_id=$1 AND (m.puppet_mxid=ANY($3) OR (r.pool_contribution_id IS NOT NULL AND m.space_id='' AND u.mxid=$2)) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake q WHERE q.assignment_id=a.id AND q.event_id=$4) ORDER BY a.id LIMIT 32").bind::<Text,_>(&event.room_id).bind::<Text,_>(&event.sender_mxid).bind::<Array<Text>,_>(mentions).bind::<Text,_>(&event.event_id).bind::<Text,_>(&after).load::<Id>(&mut *db).await?
            };
            let more = ids.len() == 32;
            if let Some(last) = ids.last() {
                after = last.id.clone();
            }
            for id in &ids {
                let (m, expired_before_observation, control_revision) = {
                    let mut db = self.db.lock().await?;
                    let expired = received <= clock(&mut db).await? - TTL;
                    let revision = control(&mut db, &id.id).await?.revision;
                    (member(&mut db, &id.id).await, expired, revision)
                };
                // An already-expired original event needs no remote room proof.
                // Persist expiry even during an extended Matrix outage.
                let observed = if expired_before_observation {
                    None
                } else {
                    match m {
                        Ok(m) => match gateway
                            .shared_delivery(
                                &m.room_id,
                                &m.space_id,
                                &m.manager_mxid,
                                &event.sender_mxid,
                                &m.puppet_mxid,
                            )
                            .await
                        {
                            Ok(v) => Some(v),
                            Err(_) => {
                                retry = true;
                                continue;
                            }
                        },
                        Err(Error::NotFound(_)) => None,
                        Err(e) => return Err(e),
                    }
                };
                let mut guard = self.db.lock().await?;
                let result=(*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;let now=clock(db).await?;if received>now{return Err(Error::Invalid("invalid_shared_intake_event"));}
    if expired_before_observation && received>now-TTL { return Err(Error::Unavailable("shared_intake_clock_changed")); }
    if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake WHERE event_id=$1 AND event_digest<>$2) AS matched").bind::<Text,_>(&event.event_id).bind::<Text,_>(&digest).get_result::<Flag>(db).await?.matched { return Err(Error::Conflict("shared_event_changed")); }
    let old=sql_query("SELECT * FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1 AND event_id=$2").bind::<Text,_>(&id.id).bind::<Text,_>(&event.event_id).get_result::<Intake>(db).await.optional()?;
    if let Some(old)=old {if old.event_digest!=digest||old.event_payload!=payload{return Err(Error::Conflict("shared_event_changed"));}return Ok(());}
    let room_ok=match member(db,&id.id).await {Ok(m)=>observed.as_ref().is_some_and(|f|facts(&m,f).is_ok())&&m.room_id==event.room_id&&m.addressed_by(&event.sender_mxid,&event.mentioned_mxids),Err(Error::NotFound(_))=>false,Err(e)=>return Err(e)};
    let ctl=control(db,&id.id).await?;let w=assignment(db,&id.id).await?;expire(db,&id.id,now).await?;
    let old_work=sql_query("SELECT w.payload FROM hagency_agent_v1.management_shared_event_pins p JOIN hagency_agent_v1.management_shared_work w ON w.id=p.work_id WHERE p.assignment_id=$1 AND p.event_id=$2").bind::<Text,_>(&id.id).bind::<Text,_>(&event.event_id).get_result::<Json>(db).await.optional()?;
    if let Some(old)=&old_work {let v:SharedWorkItem=decode(&old.payload)?;if v.room_id!=event.room_id||v.requester_mxid!=event.sender_mxid||v.body!=event.body||v.thread_root!=event.thread_root.as_deref().unwrap_or(&event.event_id){return Err(Error::Conflict("shared_event_changed"));}}
    let (state,reason)=if old_work.is_some(){("legacy_delivered",Some("already_delivered"))}else if received<=now-TTL{("expired",Some("event_expired"))}else if ctl.revision!=control_revision{("cancelled",Some("run_control_changed"))}else if !retainable(db,&id.id,&w,&self.server,now).await?{("blocked",Some("assignment_authority_changed"))}else if !room_ok{("blocked",Some("room_authority_changed"))}else if ctl.mode=="manual"{("paused",Some("manual_start_required"))}else if ctl.mode=="paused"{("paused",Some("paused_at_receipt"))}else if received<=ctl.running_since_ms.unwrap(){("cancelled",Some("before_running_intent"))}else{("pending",None)};
    let full=sql_query("SELECT count(*)>=10000 OR ($2 AND count(*) FILTER(WHERE state='pending')>=100) AS matched FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1").bind::<Text,_>(&id.id).bind::<Bool,_>(state=="pending").get_result::<Flag>(db).await?.matched;
    if full{return Err(Error::Unavailable("shared_intake_capacity"));}
    sql_query("INSERT INTO hagency_agent_v1.management_shared_intake(id,assignment_id,event_id,event_payload,event_digest,control_revision,received_at_ms,expires_at_ms,state,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind::<Text,_>(format!("sin_{}",entity_id()?)).bind::<Text,_>(&id.id).bind::<Text,_>(&event.event_id).bind::<Text,_>(&payload).bind::<Text,_>(&digest).bind::<BigInt,_>(ctl.revision).bind::<BigInt,_>(received).bind::<BigInt,_>(received.checked_add(TTL).ok_or(Error::Invalid("invalid_shared_intake_event"))?).bind::<Text,_>(state).bind::<Nullable<Text>,_>(reason).execute(db).await?;Ok(())
   }).await;
                match result {
                    Ok(()) => (),
                    Err(Error::Unavailable(_)) => retry = true,
                    Err(e) => return Err(e),
                }
            }
            if !more {
                break;
            }
        }
        if retry {
            Err(Error::Unavailable("shared_intake_retry_required"))
        } else {
            Ok(())
        }
    }
}
