//! Separate Matrix membership controller; never qualifies a provider runtime.
use super::*;
use crate::{gateway::Gateway, management_agent_matrix_types::*, matrix_client::MatrixClient};
#[derive(QueryableByName)]
struct Membership {
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=Text)]
    puppet_mxid: String,
    #[diesel(sql_type=Text)]
    room_id: String,
    #[diesel(sql_type=Text)]
    space_id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    challenge_id: Option<String>,
    #[diesel(sql_type=BigInt)]
    native_until_ms: i64,
    #[diesel(sql_type=Nullable<BigInt>)]
    verified_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<BigInt>)]
    cleanup_observed_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    blocking_reason: Option<String>,
}
#[derive(QueryableByName)]
struct Check {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    observation: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    observation_digest: Option<String>,
}
fn enabled_matrix(p: &ManagementPolicy) -> Result<()> {
    if p.agent_matrix_provisioning() {
        Ok(())
    } else {
        Err(Error::Unavailable("agent_matrix_provisioning_unavailable"))
    }
}
async fn lock(db: &mut AsyncPgConnection) -> Result<()> {
    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
        .await?;
    Ok(())
}
async fn membership(db: &mut AsyncPgConnection, id: &str) -> Result<Membership> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_matrix WHERE assignment_id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn check(db: &mut AsyncPgConnection, id: &str) -> Result<Check> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_matrix_checks WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn original(db: &mut AsyncPgConnection, m: &Membership) -> Result<Work> {
    sql_query(format!(
        "{WORK} WHERE i.request_id=$1 AND w.operation='agent.reserve'"
    ))
    .bind::<Text, _>(&m.request_id)
    .get_result(db)
    .await
    .map_err(hidden)
}
async fn current(db: &mut AsyncPgConnection, m: &Membership, w: &Work, now: i64) -> Result<bool> {
    let c: ManagedAgentReserveCommand = decode(&w.payload)?;
    if m.state != "fenced" && session_temporarily_expired(db, w, now).await? {
        return Ok(false);
    }
    if m.state == "fenced" || !live(db, w, &c, now).await? {
        fence(db, &w.intent_id).await?;
        return Ok(false);
    }
    Ok(true)
}
fn projection(m: Membership, now: i64) -> AgentMatrixReadiness {
    let state = if m.state == "matrix_verified"
        && m.verified_at_ms
            .is_none_or(|t| t > now || now - t >= MAX_FACT_AGE_MS)
    {
        "membership_recheck_required".into()
    } else {
        m.state
    };
    AgentMatrixReadiness {
        assignment_id: m.assignment_id,
        request_id: m.request_id,
        agent_id: m.agent_id,
        puppet_mxid: m.puppet_mxid,
        room_id: m.room_id,
        state,
        verified_at_ms: m.verified_at_ms,
        cleanup_observed_at_ms: m.cleanup_observed_at_ms,
        blocking_reason: m.blocking_reason,
        runtime_state: "unsupported".into(),
        work_available: false,
    }
}
async fn delivery(
    db: &mut AsyncPgConnection,
    p: &Principal,
    m: &Membership,
    w: &Work,
    q: &Check,
    historical: bool,
) -> Result<AgentMatrixDelivery> {
    owner(w, p)?;
    let c: ManagedAgentMatrixChallenge = decode(&q.payload)?;
    if allocation_digest(&c)? != q.digest {
        return Err(Error::Conflict("agent_matrix_challenge_conflict"));
    }
    let now = clock(db).await?;
    let live_now = current(db, m, w, now).await?;
    let permitted = !historical
        && q.observation.is_none()
        && q.expires_at_ms > now
        && p.device_generation == Some(w.executor_generation)
        && live_now;
    let token = secret_token();
    // Historical tickets may upload observations, never extend their fixed validity.
    let until = sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now;
    let until = if permitted {
        until.min(q.expires_at_ms)
    } else {
        until
    };
    sql_query("UPDATE hagency_agent_v1.management_agent_matrix_checks SET lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(&q.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).execute(db).await?;
    Ok(AgentMatrixDelivery {
        command: c,
        command_digest: q.digest.clone(),
        token,
        lease_until_ms: until,
        execution_allowed: permitted,
        recorded: q.observation.as_deref().map(decode).transpose()?,
    })
}
impl DomainStore {
    pub async fn poll_managed_agent_matrix(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<AgentMatrixDelivery>> {
        enabled_matrix(policy)?;
        enabled_executor(policy)?;
        enabled(policy)?;
        let (device, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;sweep(db,p,now).await?;
            // Desktop returns the last challenge ID, not an assignment ID.
            // Resolve that cursor and wrap so expired checks can be renewed.
            let candidates=sql_query("SELECT m.* FROM hagency_agent_v1.management_agent_matrix m JOIN hagency_agent_v1.management_shared_assignments a ON a.id=m.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND m.state IN ('waiting_native','authorized') AND m.native_until_ms<=$4 ORDER BY (m.assignment_id<=COALESCE((SELECT assignment_id FROM hagency_agent_v1.management_agent_matrix_checks WHERE id=$3),$3)),m.assignment_id LIMIT 32").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(now).load::<Membership>(db).await?;
            for m in candidates {
                let w=original(db,&m).await?;
                if p.device_generation!=Some(w.executor_generation)||!current(db,&m,&w,now).await? {continue;}
                if let Some(id)=&m.challenge_id {
                    let q=check(db,id).await?;
                    if q.expires_at_ms>now {return Ok(Some(delivery(db,p,&m,&w,&q,false).await?));}
                }
                let a=super::projection(db,&m.request_id).await?.ok_or(Error::Conflict("agent_assignment_required"))?;
                let reserve:ManagedAgentReserveCommand=decode(&w.payload)?;
                let lifetime=super::reserve_lifetime(db,&reserve,now).await?;
                let c=ManagedAgentMatrixChallenge {version:1,capability:AGENT_MATRIX_CAPABILITY.into(),operation:"agent.matrix.check".into(),command_id:format!("mmc_{}",entity_id()?),assignment_id:m.assignment_id.clone(),assignment_generation:a.generation,agent_reservation_id:a.agent_reservation_id,reserve_command:reserve.clone(),reserve_command_digest:w.digest.clone(),reserve_receipt_digest:a.reserve_receipt_digest,space_id:m.space_id.clone(),puppet_mxid:m.puppet_mxid.clone(),issued_at_ms:now,expires_at_ms:(now+30_000).min(lifetime)};
                c.validate(&self.server)?;let digest=allocation_digest(&c)?;
                sql_query("INSERT INTO hagency_agent_v1.management_agent_matrix_checks(id,assignment_id,payload,digest,expires_at_ms) VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&m.assignment_id).bind::<Text,_>(wire(&c)?).bind::<Text,_>(&digest).bind::<BigInt,_>(c.expires_at_ms).execute(db).await?;
                sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET state='waiting_native',challenge_id=$2,native_until_ms=0,verified_at_ms=NULL WHERE assignment_id=$1").bind::<Text,_>(&m.assignment_id).bind::<Text,_>(&c.command_id).execute(db).await?;
                let q=check(db,&c.command_id).await?;return Ok(Some(delivery(db,p,&m,&w,&q,false).await?));
            } Ok(None)
        }).await
    }
    pub async fn recover_managed_agent_matrix(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ExecutorRecoverInput,
    ) -> Result<AgentMatrixDelivery> {
        enabled_matrix(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let q = check(db, id).await?;
                let m = membership(db, &q.assignment_id).await?;
                let w = original(db, &m).await?;
                owner(&w, p)?;
                if q.digest != input.command_digest {
                    return Err(Error::Conflict("agent_matrix_challenge_conflict"));
                }
                delivery(db, p, &m, &w, &q, true).await
            })
            .await
    }
    pub async fn record_managed_agent_matrix(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: AgentMatrixResultInput,
    ) -> Result<AgentMatrixAcknowledgement> {
        enabled_matrix(policy)?;
        key(id)?;
        wire(&input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let q=check(db,id).await?;let m=membership(db,&q.assignment_id).await?;let w=original(db,&m).await?;owner(&w,p)?;
            if q.digest!=input.command_digest {return Err(Error::Conflict("agent_matrix_challenge_conflict"));}
            let c:ManagedAgentMatrixChallenge=decode(&q.payload)?;input.observation.validate_for(&c)?;let digest=allocation_digest(&input.observation)?;
            if let Some(old)=&q.observation_digest {if old!=&digest{return Err(Error::Conflict("agent_matrix_observation_conflict"));}}
            else {
                if q.lease_until_ms<=now || q.lease_hash.as_deref()!=Some(hash(&input.token).as_str()) {return Err(Error::Unauthorized("agent_matrix_ticket_required"));}
                let reserve:ManagedAgentReservationReceipt=decode(w.receipt.as_deref().ok_or(Error::Conflict("agent_reservation_required"))?)?;
                if input.observation.observed_ledger_revision<reserve.ledger_revision {return Err(Error::Conflict("agent_matrix_observation_stale"));}
                sql_query("UPDATE hagency_agent_v1.management_agent_matrix_checks SET observation=$2,observation_digest=$3 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(wire(&input.observation)?).bind::<Text,_>(&digest).execute(db).await?;
                let allowed=input.observation.eligible && q.expires_at_ms>now && p.device_generation==Some(w.executor_generation) && policy.agent_executor() && policy.agent_admission() && policy.project_reservations() && m.challenge_id.as_deref()==Some(id) && current(db,&m,&w,now).await?;
                if m.state!="fenced" && m.challenge_id.as_deref()==Some(id) {
                    sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET state=CASE WHEN $2 THEN 'authorized' ELSE 'waiting_native' END,native_until_ms=$3,blocking_reason=$4 WHERE assignment_id=$1 AND state<>'fenced'").bind::<Text,_>(&m.assignment_id).bind::<Bool,_>(allowed).bind::<BigInt,_>(if allowed {q.expires_at_ms}else{0}).bind::<Nullable<Text>,_>(if allowed{None}else{Some("native_reservation_unavailable")}).execute(db).await?;
                }
            }
            let latest=membership(db,&m.assignment_id).await?;let now=clock(db).await?;current(db,&latest,&w,now).await?;
            Self::authorize(db,p,crate::api::now_ms()).await?;
            Ok(AgentMatrixAcknowledgement {command_id:id.into(),command_digest:q.digest,observation_digest:digest,membership:projection(membership(db,&m.assignment_id).await?,clock(db).await?)})
        }).await
    }
    pub async fn managed_agent_matrix(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request: &str,
    ) -> Result<Option<AgentMatrixReadiness>> {
        enabled_matrix(policy)?;
        key(request)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let r = row(db, request).await?;
                let m = sql_query(
                    "SELECT * FROM hagency_agent_v1.management_agent_matrix WHERE request_id=$1",
                )
                .bind::<Text, _>(request)
                .get_result::<Membership>(db)
                .await
                .optional()?;
                if let Some(m) = m {
                    let w = original(db, &m).await?;
                    if owner(&w, p).is_err() {
                        visible(db, &r, p).await?;
                    }
                    let now = clock(db).await?;
                    current(db, &m, &w, now).await?;
                    Ok(Some(projection(
                        membership(db, &m.assignment_id).await?,
                        now,
                    )))
                } else {
                    visible(db, &r, p).await?;
                    Ok(None)
                }
            })
            .await
    }
}

struct Phase {
    membership: Membership,
    context: Option<Context>,
    manager_mxid: String,
    agent: Agent,
}
fn issuing(p: Option<&ManagementPolicy>) -> bool {
    p.is_some_and(|p| {
        p.agent_matrix_provisioning()
            && p.agent_executor()
            && p.agent_admission()
            && p.project_reservations()
    })
}
fn puppet_membership(events: &[serde_json::Value], puppet: &str) -> Result<String> {
    let mut matching = events
        .iter()
        .filter(|e| e["type"] == "m.room.member" && e["state_key"] == puppet);
    let first = matching.next();
    if matching.next().is_some() {
        return Err(Error::Conflict("matrix_membership_ambiguous"));
    }
    Ok(first
        .and_then(|e| e["content"]["membership"].as_str())
        .unwrap_or("leave")
        .into())
}
impl DomainStore {
    async fn matrix_phase(
        &self,
        id: &str,
        challenge: Option<&str>,
        effect: bool,
        policy: Option<&ManagementPolicy>,
    ) -> Result<Option<Phase>> {
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            lock(db).await?;let m=membership(db,id).await?;let w=original(db,&m).await?;let now=clock(db).await?;
            if !current(db,&m,&w,now).await? {return Ok(None);}
            if effect && (!issuing(policy)||m.native_until_ms<=now||m.state!="authorized"||m.challenge_id.as_deref()!=challenge) {return Err(Error::Conflict("agent_matrix_native_check_required"));}
            let r=row(db,&m.request_id).await?;let cx=if r.pool_contribution_id.is_some() {None} else {Some(context(db,&r.plan_id,now).await?)};
            let manager_mxid=sql_query("SELECT mxid AS payload FROM hagency_agent_v1.users WHERE id=$1").bind::<Text,_>(&r.manager_user_id).get_result::<Json>(db).await?.payload;
            let agent:Agent=sql_query("SELECT id,owner_user_id,puppet_mxid,display_name,state,generation,owner_direct_room_id,execution_device_id FROM hagency_agent_v1.agents WHERE id=$1 AND execution_kind='shared_project'").bind::<Text,_>(&m.agent_id).get_result(db).await?;
            if agent.owner_user_id!=r.manager_user_id||agent.puppet_mxid!=m.puppet_mxid||agent.state!="suspended"||agent.execution_device_id.is_some()||agent.owner_direct_room_id.is_some()||cx.as_ref().is_some_and(|c|c.setup.space_id.as_deref()!=Some(&m.space_id)||c.setup.room_id.as_deref()!=Some(&m.room_id))||cx.is_none()&&(!m.space_id.is_empty()||m.room_id!=r.room_id) {fence(db,&w.intent_id).await?;return Ok(None);}
            if effect {sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET effect_started=true WHERE assignment_id=$1").bind::<Text,_>(id).execute(db).await?;}
            Ok(Some(Phase {membership:m,context:cx,manager_mxid,agent}))
        }).await
    }
    async fn matrix_fence(&self, id: &str) -> Result<()> {
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                lock(db).await?;
                let m = membership(db, id).await?;
                let w = original(db, &m).await?;
                fence(db, &w.intent_id).await
            })
            .await
    }
    async fn matrix_observe(
        &self,
        id: &str,
        gateway: &Gateway,
        policy: Option<&ManagementPolicy>,
    ) -> Result<Option<(Phase, i64, String)>> {
        let Some(before) = self.matrix_phase(id, None, false, policy).await? else {
            return Ok(None);
        };
        let observation = if let Some(context) = &before.context {
            context.observe_details(gateway).await
        } else {
            gateway.pool_room(&before.membership.room_id, &before.manager_mxid).await
        };
        let Some(after) = self.matrix_phase(id, None, false, policy).await? else {
            return Ok(None);
        };
        if before.context.as_ref().map(Context::pin) != after.context.as_ref().map(Context::pin)
            || before.manager_mxid != after.manager_mxid
            || before.membership.room_id != after.membership.room_id {
            return Err(Error::Conflict("agent_matrix_authority_changed"));
        }
        let (at, events) = match observation {
            Ok(v) => v,
            Err(Error::Conflict(_) | Error::Unauthorized(_)) => {
                self.matrix_fence(id).await?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let membership = puppet_membership(&events, &after.membership.puppet_mxid)?;
        Ok(Some((after, at, membership)))
    }
    async fn matrix_confirm(
        &self,
        id: &str,
        challenge: Option<&str>,
        at: i64,
        joined: bool,
    ) -> Result<()> {
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            lock(db).await?;let m=membership(db,id).await?;let w=original(db,&m).await?;let now=clock(db).await?;fresh(at,now)?;
            if !current(db,&m,&w,now).await? {return Ok(());}
            if m.challenge_id.as_deref()!=challenge || m.state!="matrix_verified"&&(m.state!="authorized"||m.native_until_ms<=now) {return Err(Error::Conflict("agent_matrix_native_check_required"));}
            sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET state=CASE WHEN $2 THEN 'matrix_verified' ELSE 'waiting_native' END,verified_at_ms=$3,native_until_ms=CASE WHEN $2 THEN native_until_ms ELSE 0 END,blocking_reason=CASE WHEN $2 THEN NULL ELSE 'matrix_membership_missing' END WHERE assignment_id=$1").bind::<Text,_>(id).bind::<Bool,_>(joined).bind::<Nullable<BigInt>,_>(joined.then_some(at)).execute(db).await?;Ok(())
        }).await
    }
    async fn matrix_cleanup_desired(&self, id: &str) -> Result<bool> {
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                lock(db).await?;
                Ok(membership(db, id).await?.state == "fenced")
            })
            .await
    }
    async fn matrix_cleanup(
        &self,
        id: &str,
        gateway: &Gateway,
        matrix: &MatrixClient,
    ) -> Result<()> {
        let m = {
            let mut db = self.db.lock().await?;
            membership(&mut db, id).await?
        };
        if !self.matrix_cleanup_desired(id).await? {
            return Ok(());
        }
        let before = gateway.state(&m.room_id).await;
        if !self.matrix_cleanup_desired(id).await? {
            return Ok(());
        }
        let member = puppet_membership(&before?, &m.puppet_mxid)?;
        if matches!(member.as_str(), "join" | "invite") {
            if !self.matrix_cleanup_desired(id).await? {
                return Ok(());
            }
            let result = matrix.leave(&m.puppet_mxid, &m.room_id).await;
            if !self.matrix_cleanup_desired(id).await? {
                return Ok(());
            }
            result?;
        }
        let after = gateway.state(&m.room_id).await;
        if !self.matrix_cleanup_desired(id).await? {
            return Ok(());
        }
        let member = puppet_membership(&after?, &m.puppet_mxid)?;
        if !matches!(member.as_str(), "join" | "invite") {
            let mut db = self.db.lock().await?;
            (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {lock(db).await?;sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET cleanup_observed_at_ms=$2 WHERE assignment_id=$1 AND state='fenced'").bind::<Text,_>(id).bind::<BigInt,_>(clock(db).await?).execute(db).await?;Ok(())}).await?;
        }
        Ok(())
    }
    pub(crate) async fn reconcile_managed_agent_matrix(
        &self,
        id: &str,
        gateway: &Gateway,
        matrix: &MatrixClient,
        policy: Option<&ManagementPolicy>,
    ) -> Result<()> {
        let Some(initial) = self.matrix_phase(id, None, false, policy).await? else {
            return self.matrix_cleanup(id, gateway, matrix).await;
        };
        if initial.membership.state != "matrix_verified"
            && (!issuing(policy)
                || initial.membership.state != "authorized"
                || initial.membership.native_until_ms <= crate::api::now_ms())
        {
            return Ok(());
        }
        // Pool rooms have no legacy project binding whose controller would
        // accept the service invitation. Join only the validated invite-only
        // room, under the same fresh native authorization as puppet provisioning.
        if initial.context.is_none() && initial.membership.state == "authorized" {
            let invited = match gateway.pool_service_invited(
                &initial.membership.room_id,
                &initial.manager_mxid,
            ).await {
                Ok(invited) => invited,
                Err(Error::Conflict(_) | Error::Unauthorized(_)) => {
                    self.matrix_fence(id).await?;
                    return self.matrix_cleanup(id, gateway, matrix).await;
                }
                Err(error) => return Err(error),
            };
            if invited {
                let challenge = initial.membership.challenge_id.as_deref();
                let Some(phase) = self.matrix_phase(id, challenge, true, policy).await? else {
                    return self.matrix_cleanup(id, gateway, matrix).await;
                };
                let result = matrix.join_invited_service(&phase.membership.room_id).await;
                if self.matrix_phase(id, challenge, true, policy).await?.is_none() {
                    return self.matrix_cleanup(id, gateway, matrix).await;
                }
                result?;
            }
        }
        let Some((observed, at, member)) = self.matrix_observe(id, gateway, policy).await? else {
            return self.matrix_cleanup(id, gateway, matrix).await;
        };
        let challenge = observed.membership.challenge_id.as_deref();
        if member == "join" || observed.membership.state == "matrix_verified" {
            return self
                .matrix_confirm(id, challenge, at, member == "join")
                .await;
        }
        // Each method below performs exactly one HTTP call. No runtime is touched.
        let Some(phase) = self.matrix_phase(id, challenge, true, policy).await? else {
            return self.matrix_cleanup(id, gateway, matrix).await;
        };
        let result = matrix.managed_verify_identity(&phase.agent).await;
        if self
            .matrix_phase(id, challenge, true, policy)
            .await?
            .is_none()
        {
            return self.matrix_cleanup(id, gateway, matrix).await;
        }
        result?;
        let result = matrix.managed_set_display_name(&phase.agent).await;
        if self
            .matrix_phase(id, challenge, true, policy)
            .await?
            .is_none()
        {
            return self.matrix_cleanup(id, gateway, matrix).await;
        }
        result?;
        if member != "invite" {
            let result = matrix
                .managed_invite(&phase.agent, &phase.membership.room_id)
                .await;
            if self
                .matrix_phase(id, challenge, true, policy)
                .await?
                .is_none()
            {
                return self.matrix_cleanup(id, gateway, matrix).await;
            }
            result?;
        }
        let result = matrix
            .managed_join(&phase.agent, &phase.membership.room_id)
            .await;
        if self
            .matrix_phase(id, challenge, true, policy)
            .await?
            .is_none()
        {
            return self.matrix_cleanup(id, gateway, matrix).await;
        }
        result?;
        let Some((latest, at, member)) = self.matrix_observe(id, gateway, policy).await? else {
            return self.matrix_cleanup(id, gateway, matrix).await;
        };
        self.matrix_confirm(
            id,
            latest.membership.challenge_id.as_deref(),
            at,
            member == "join",
        )
        .await
    }
    pub(crate) async fn managed_matrix_candidates(
        &self,
        after: &str,
        limit: i64,
    ) -> Result<Vec<String>> {
        let mut db = self.db.lock().await?;
        let rows=sql_query("SELECT assignment_id AS payload FROM hagency_agent_v1.management_agent_matrix WHERE assignment_id>$1 ORDER BY assignment_id LIMIT $2").bind::<Text,_>(after).bind::<BigInt,_>(limit.clamp(1,20)).load::<Json>(&mut *db).await?;
        Ok(rows.into_iter().map(|r| r.payload).collect())
    }
}
