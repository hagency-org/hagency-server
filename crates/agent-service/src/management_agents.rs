//! Assigned-admin admission only. Immutable native intent is deliberately not
//! deliverable and no agent identity, binding, hold or runtime is created here.
use super::*;
use crate::{
    gateway::Gateway, management_agent_types::*, management_allocation_types::allocation_digest,
    management_setup_types::ProjectSetup,
};
use serde_json::{Value, json};
#[path = "management_agent_budget.rs"]
mod budget;
#[path = "management_agent_cleanup.rs"]
mod cleanup;
#[path = "management_agent_settlement.rs"]
mod settlement;
#[path = "management_pool_agents.rs"]
mod pool;
#[path = "management_agent_executor.rs"]
pub(super) mod executor;

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type=Nullable<Text>)]
    pool_contribution_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pool_grant_id: Option<String>,
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    plan_id: String,
    #[diesel(sql_type=Text)]
    setup_id: String,
    #[diesel(sql_type=BigInt)]
    setup_binding_revision: i64,
    #[diesel(sql_type=Text)]
    allocation_id: String,
    #[diesel(sql_type=Text)]
    allocation_receipt_digest: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    domain_project_id: String,
    #[diesel(sql_type=Text)]
    room_id: String,
    #[diesel(sql_type=Text)]
    proposed_agent_id: String,
    #[diesel(sql_type=Text)]
    definition: String,
    #[diesel(sql_type=Text)]
    definition_digest: String,
    #[diesel(sql_type=BigInt)]
    requested_tokens: i64,
    #[diesel(sql_type=Text)]
    period_key: String,
    #[diesel(sql_type=Text)]
    administrator_user_id: String,
    #[diesel(sql_type=BigInt)]
    authority_revision: i64,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
}
#[derive(QueryableByName)]
struct AgentPerson { #[diesel(sql_type=Text)] payload:String }
#[derive(QueryableByName)]
struct Roles {
    #[diesel(sql_type=Text)]
    project_name: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    manager_mxid: String,
    #[diesel(sql_type=Text)]
    administrator_user_id: String,
    #[diesel(sql_type=Text)]
    administrator_mxid: String,
    #[diesel(sql_type=BigInt)]
    authority_revision: i64,
    #[diesel(sql_type=Bool)]
    active: bool,
}
struct Context {
    setup: ProjectSetup,
    roles: Roles,
    capacity: allocations::SetupCapacity,
    owner: String,
    contribution: Contribution,
}
fn enabled(policy: &ManagementPolicy) -> Result<()> {
    if policy.agent_admission() && policy.project_reservations() {
        Ok(())
    } else {
        Err(Error::Unavailable("agent_admission_unavailable"))
    }
}
const ROLES: &str = "SELECT p.name AS project_name,p.manager_user_id,u.mxid AS manager_mxid,a.administrator_user_id,v.mxid AS administrator_mxid,a.revision AS authority_revision,(u.active AND v.active) AS active FROM hagency_agent_v1.management_projects p JOIN hagency_agent_v1.users u ON u.id=p.manager_user_id JOIN hagency_agent_v1.management_project_authorities a ON a.plan_id=p.id JOIN hagency_agent_v1.users v ON v.id=a.administrator_user_id WHERE p.id=$1";
async fn roles(db: &mut AsyncPgConnection, id: &str) -> Result<Roles> {
    sql_query(ROLES)
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn row(db: &mut AsyncPgConnection, id: &str) -> Result<Row> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_request_read WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn context(db: &mut AsyncPgConnection, id: &str, now: i64) -> Result<Context> {
    let roles = roles(db, id).await?;
    if !roles.active {
        return Err(Error::Conflict("project_administrator_required"));
    }
    let s = setup::setup(db, id)
        .await?
        .ok_or(Error::Conflict("matrix_setup_required"))?;
    if s.state != "ready"
        || s.room_id.is_none()
        || s.space_id.is_none()
        || s.domain_project_id.is_none()
    {
        return Err(Error::Conflict("matrix_verification_required"));
    }
    let b = s
        .current_binding
        .as_ref()
        .ok_or(Error::Conflict("project_setup_changed"))?;
    if b.authority_revision != roles.authority_revision
        || b.administrator_mxid != roles.administrator_mxid
    {
        return Err(Error::Conflict("project_administrator_changed"));
    }
    let capacity = allocations::setup_capacity(db, id, Some(&b.allocation_id), now)
        .await?
        .ok_or(Error::Conflict("live_reservation_required"))?;
    if capacity.receipt_digest != b.allocation_receipt_digest {
        return Err(Error::Conflict("project_setup_changed"));
    }
    let active=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects p JOIN hagency_agent_v1.rooms r ON r.project_id=p.id WHERE p.id=$1 AND r.room_id=$2 AND p.active AND r.active) AS matched").bind::<Text,_>(s.domain_project_id.as_ref().unwrap()).bind::<Text,_>(s.room_id.as_ref().unwrap()).get_result::<Flag>(db).await?.matched;
    if !active {
        return Err(Error::Conflict("project_disabled"));
    }
    let (owner, contribution) = contribution(db, &capacity.grant.contribution_id).await?;
    Ok(Context {
        setup: s,
        roles,
        capacity,
        owner,
        contribution,
    })
}
impl Context {
    fn matches(&self, r: &Row) -> bool {
        r.setup_id == self.setup.id
            && r.setup_binding_revision <= self.setup.current_binding.as_ref().unwrap().revision
            && r.allocation_id == self.capacity.id
            && r.allocation_receipt_digest == self.capacity.receipt_digest
            && Some(&r.domain_project_id) == self.setup.domain_project_id.as_ref()
            && Some(&r.room_id) == self.setup.room_id.as_ref()
            && r.manager_user_id == self.roles.manager_user_id
            && r.period_key == self.capacity.grant.period_key
    }
    fn pin(&self) -> (i64, i64, i64, i64) {
        (
            self.setup.revision,
            self.setup.current_binding.as_ref().unwrap().revision,
            self.roles.authority_revision,
            self.capacity.revision,
        )
    }
    async fn observe(&self, gateway: &Gateway) -> Result<i64> {
        Ok(self.observe_details(gateway).await?.0)
    }
    async fn observe_details(&self, gateway: &Gateway) -> Result<(i64, Vec<Value>)> {
        let started = crate::api::now_ms();
        let space = self.setup.space_id.as_ref().unwrap();
        let room = self.setup.room_id.as_ref().unwrap();
        let manager = &self.roles.manager_mxid;
        let (sf, rf, f, ss, rs) = gateway.project_setup(space, room, manager).await?;
        // Facts are for the stored human manager, never a forged Principal.
        for (a, id) in [(&sf, space), (&rf, room)] {
            fresh(a.observed_at_ms, crate::api::now_ms())?;
            if a.actor_mxid != *manager || a.room_id != *id || !a.joined || !a.can_manage_policy {
                return Err(Error::Unauthorized("room_administration_denied"));
            }
        }
        if !sf.is_space
            || rf.is_space
            || rf.linked_space_id.as_deref() != Some(space)
            || f.owner_mxid != *manager
            || f.room_id != *room
            || f.space_id != *space
            || !f.owner_in_room
            || !f.owner_in_space
            || !f.room_in_space
            || !f.service_can_invite
            || f.encrypted
        {
            return Err(Error::Conflict("project_matrix_not_ready"));
        }
        for events in [&ss, &rs] {
            setup::joined(events, &self.roles.administrator_mxid)?;
            if setup::state(events, "m.room.join_rules", "")?["content"]["join_rule"] != "invite" {
                return Err(Error::Conflict("project_private_room_required"));
            }
        }
        if setup::state(&rs, "m.space.parent", space)?["content"]["via"]
            .as_array()
            .is_none_or(|a| a.is_empty())
        {
            return Err(Error::Conflict("project_matrix_proof_missing"));
        }
        if self.setup.mode == "create" {
            setup::marker(&ss, &self.setup, "space", space)?;
            setup::marker(&rs, &self.setup, "room", space)?;
        }
        fresh(started, crate::api::now_ms())?;
        Ok((started, rs))
    }
}
async fn visible(db: &mut AsyncPgConnection, r: &Row, p: &Principal) -> Result<Roles> {
    if r.pool_contribution_id.is_some() {
        if r.manager_user_id != p.user_id && r.administrator_user_id != p.user_id {
            return Err(Error::NotFound("agent_request_not_found"));
        }
        let person=sql_query("SELECT mxid AS payload FROM hagency_agent_v1.users WHERE id=$1").bind::<Text,_>(&r.manager_user_id).get_result::<AgentPerson>(db).await?.payload;
        return Ok(Roles {project_name:String::new(), manager_user_id:r.manager_user_id.clone(),manager_mxid:person.clone(),administrator_user_id:r.manager_user_id.clone(),administrator_mxid:person,authority_revision:r.authority_revision,active:true});
    }
    let roles = roles(db, &r.plan_id).await?;
    let historic=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_decisions WHERE request_id=$1 AND administrator_user_id=$2) AS matched").bind::<Text,_>(&r.id).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
    if r.manager_user_id != p.user_id && roles.administrator_user_id != p.user_id && !historic {
        return Err(Error::NotFound("agent_request_not_found"));
    }
    Ok(roles)
}
async fn view(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    now: i64,
) -> Result<ManagedAgentRequest> {
    let r = row(db, id).await?;
    let roles = visible(db, &r, p).await?;
    let mut reason = None;
    let current = match context(db, &r.plan_id, now).await {
        Ok(c) => {
            if !c.matches(&r) {
                reason = Some("agent_request_binding_changed");
            }
            Some(c)
        }
        Err(Error::Conflict(code) | Error::NotFound(code)) => {
            reason = Some(code);
            None
        }
        Err(e) => return Err(e),
    };
    let expired = r.expires_at_ms <= now && r.state == "pending_approval";
    if expired {
        reason = Some("agent_request_expired");
    }
    if r.state == "approved_waiting_executor" && reason.is_none() {
        reason = Some("managed_agent_execution_unavailable");
    }
    let pending = r.state == "pending_approval" && !expired;
    let can_approve = pending
        && roles.active
        && p.user_id == roles.administrator_user_id
        && reason.is_none()
        && current.is_some();
    let can_reject = pending && roles.active && p.user_id == roles.administrator_user_id;
    Ok(ManagedAgentRequest {
        id: r.id,
        project_id: r.plan_id,
        project_name: Some(roles.project_name),
        proposed_agent_id: r.proposed_agent_id,
        manager_mxid: roles.manager_mxid,
        administrator_mxid: roles.administrator_mxid,
        authority_revision: roles.authority_revision,
        definition: decode(&r.definition)?,
        definition_digest: r.definition_digest,
        requested_tokens: r.requested_tokens,
        period_key: r.period_key,
        state: if expired {
            "expired".into()
        } else {
            r.state.clone()
        },
        revision: r.revision,
        created_at_ms: r.created_at_ms,
        expires_at_ms: r.expires_at_ms,
        can_decide: can_approve || can_reject,
        can_approve,
        can_reject,
        can_cancel: p.user_id == r.manager_user_id
            && matches!(
                r.state.as_str(),
                "pending_approval" | "approved_waiting_executor"
            ),
        blocking_reason: reason.map(str::to_owned),
        execution_available: false,
    })
}
async fn notice(db: &mut AsyncPgConnection, r: &ManagedAgentRequest, now: i64) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_agent_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'").bind::<Text,_>(&r.id).execute(db).await?;
    for recipient in BTreeSet::from([r.manager_mxid.clone(), r.administrator_mxid.clone()]) {
        let id = format!("mano_{}", entity_id()?);
        sql_query("INSERT INTO hagency_agent_v1.management_agent_outbox(id,request_id,request_revision,recipient_mxid,transaction_id,created_at_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(request_id,request_revision,recipient_mxid) DO NOTHING").bind::<Text,_>(&id).bind::<Text,_>(&r.id).bind::<BigInt,_>(r.revision).bind::<Text,_>(recipient).bind::<Text,_>(format!("hagency-{id}")).bind::<BigInt,_>(now).execute(db).await?;
    }
    Ok(())
}
impl DomainStore {
    /// Read-only submission evidence. Matrix administration is re-observed by
    /// request_managed_agent; this snapshot is never a reservation or lease.
    pub async fn management_agent_readiness(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<AgentRequestReadiness> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                // Apply existing project visibility before exposing even a blocker.
                let plan = projects::project(db, id, p, policy, now).await?;
                let roles = sql_query(ROLES)
                    .bind::<Text, _>(id)
                    .get_result::<Roles>(db)
                    .await
                    .optional()?;
                let manager = plan.manager_mxid == p.mxid;
                let can_view = manager
                    || roles
                        .as_ref()
                        .is_some_and(|r| r.administrator_user_id == p.user_id);
                let mut reason = if !manager {
                    Some("project_manager_required")
                } else {
                    None
                };
                let context = if can_view && roles.is_some() {
                    match context(db, id, now).await {
                        Ok(c) => Some(c),
                        Err(Error::Conflict(code) | Error::NotFound(code)) => {
                            if manager {
                                reason = Some(if roles.is_none() {
                                    "project_administrator_required"
                                } else {
                                    code
                                });
                            }
                            None
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    if manager {
                        reason = Some("project_administrator_required");
                    }
                    None
                };
                let request_context = context.map(|c| AgentRequestContext {
                    setup_id: c.setup.id,
                    setup_revision: c.setup.revision,
                    setup_binding_revision: c.setup.current_binding.as_ref().unwrap().revision,
                    authority_revision: c.roles.authority_revision,
                    allocation_id: c.capacity.id,
                    allocation_revision: c.capacity.revision,
                    allocation_receipt_digest: c.capacity.receipt_digest,
                    requested_token_limit: c.capacity.requested_tokens,
                    // Neither an immutable receipt nor central grant history can
                    // establish current native headroom after other agent holds.
                    native_available_tokens: None,
                    period_key: c.capacity.grant.period_key,
                    period_start_ms: c.capacity.grant.period_start_ms,
                    period_end_ms: c.capacity.grant.period_end_ms,
                    capabilities: c.contribution.capabilities.into_iter().collect(),
                    tool_modes: vec!["none".into()],
                });
                Ok(AgentRequestReadiness {
                    protocol: PROTOCOL.into(),
                    observed_at_ms: now,
                    project_id: plan.id,
                    project_name: plan.name,
                    manager_mxid: plan.manager_mxid,
                    administrator_mxid: roles.map(|r| r.administrator_mxid),
                    can_view_requests: can_view,
                    can_request: manager && request_context.is_some(),
                    blocking_reason: reason.map(str::to_owned),
                    request_context,
                })
            })
            .await
    }
    pub async fn management_agent_request(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ManagedAgentRequest> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                view(db, id, p, now).await
            })
            .await
    }
    pub async fn management_agent_requests(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: AgentRequestQuery,
    ) -> Result<ManagementPage<ManagedAgentRequest>> {
        enabled(policy)?;
        input.validate()?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if let Some(project) = &input.project_id {
                let allowed = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_projects p WHERE p.id=$1 AND (p.manager_user_id=$2 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_authorities a WHERE a.plan_id=p.id AND a.administrator_user_id=$2))) AS matched").bind::<Text,_>(project).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
                if !allowed { return Err(Error::NotFound("agent_request_not_found")); }
            }
            let rows=sql_query("SELECT r.* FROM hagency_agent_v1.management_agent_requests r JOIN hagency_agent_v1.management_project_authorities a ON a.plan_id=r.plan_id WHERE (r.manager_user_id=$1 OR a.administrator_user_id=$1) AND (NOT $2 OR a.administrator_user_id=$1 AND r.state='pending_approval' AND r.expires_at_ms>$3) AND r.id>$4 AND ($5::text IS NULL OR r.plan_id=$5) ORDER BY r.id LIMIT 17").bind::<Text,_>(&p.user_id).bind::<Bool,_>(input.needs_my_action).bind::<BigInt,_>(now).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<Nullable<Text>,_>(input.project_id.as_deref()).load::<Row>(db).await?;
            let more=rows.len()>16;let mut last=None;let mut items=Vec::new();for r in rows.into_iter().take(16){last=Some(r.id.clone());let v=view(db,&r.id,p,now).await?;if !input.needs_my_action||v.can_decide{items.push(v);}}
            Ok(ManagementPage{protocol:PROTOCOL,observed_at_ms:now,items,next_after:if more{last}else{None}})
        }).await
    }
    pub async fn request_managed_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        plan: &str,
        input: RequestManagedAgent,
        gateway: &Gateway,
    ) -> Result<ManagedAgentRequest> {
        enabled(policy)?;
        key(plan)?;
        input.validate()?;
        device(p)?;
        let digest = digest(plan, &input)?;
        let pre = {
            let mut db = self.db.lock().await?;
            (*db)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, crate::api::now_ms()).await?;
                    let now = clock(db).await?;
                    if let Some(saved) = replay::<ManagedAgentRequest>(
                        db,
                        p,
                        &input.command_id,
                        "agent.request",
                        &digest,
                    )
                    .await?
                    {
                        return Ok(Err(view(db, &saved.id, p, now).await?));
                    }
                    let a = roles(db, plan).await?;
                    if a.manager_user_id != p.user_id {
                        return Err(Error::Unauthorized("project_manager_required"));
                    }
                    let c = context(db, plan, now).await?;
                    if c.pin()
                        != (
                            input.expected_setup_revision,
                            input.expected_setup_binding_revision,
                            input.expected_authority_revision,
                            input.expected_allocation_revision,
                        )
                    {
                        return Err(Error::Conflict("management_revision_changed"));
                    }
                    if input.requested_tokens > c.capacity.requested_tokens
                        || !c
                            .contribution
                            .capabilities
                            .contains(&input.definition.capability)
                    {
                        return Err(Error::Conflict("agent_offer_unavailable"));
                    }
                    Ok(Ok(c))
                })
                .await?
        };
        let pre = match pre {
            Err(saved) => return Ok(saved),
            Ok(c) => c,
        };
        let observed = pre.observe(gateway).await?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if let Some(saved)=replay::<ManagedAgentRequest>(db,p,&input.command_id,"agent.request",&digest).await?{return view(db,&saved.id,p,now).await;}
            let c=context(db,plan,now).await?;if c.pin()!=pre.pin(){return Err(Error::Conflict("management_revision_changed"));}fresh(observed,now)?;
            let full=sql_query("SELECT count(*)>=1000 AS matched FROM hagency_agent_v1.management_agent_requests WHERE manager_user_id=$1").bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;if full{return Err(Error::Conflict("agent_request_capacity"));}
            let id=format!("mar_{}",entity_id()?);let proposed=format!("agt_{}",entity_id()?);let definition=wire(&input.definition)?;let definition_digest=allocation_digest(&input.definition)?;let expires=(now+TTL).min(c.capacity.grant.period_end_ms).min(c.capacity.grant.delegation_expires_at_ms);
            sql_query("INSERT INTO hagency_agent_v1.management_agent_requests(id,plan_id,setup_id,setup_binding_revision,allocation_id,allocation_receipt_digest,manager_user_id,domain_project_id,room_id,proposed_agent_id,definition,definition_digest,requested_tokens,period_key,administrator_user_id,authority_revision,state,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,'pending_approval',$17,$18)")
                .bind::<Text,_>(&id).bind::<Text,_>(plan).bind::<Text,_>(&c.setup.id).bind::<BigInt,_>(c.setup.current_binding.as_ref().unwrap().revision).bind::<Text,_>(&c.capacity.id).bind::<Text,_>(&c.capacity.receipt_digest).bind::<Text,_>(&p.user_id).bind::<Text,_>(c.setup.domain_project_id.as_ref().unwrap()).bind::<Text,_>(c.setup.room_id.as_ref().unwrap()).bind::<Text,_>(&proposed).bind::<Text,_>(definition).bind::<Text,_>(definition_digest).bind::<BigInt,_>(input.requested_tokens).bind::<Text,_>(&c.capacity.grant.period_key).bind::<Text,_>(&c.roles.administrator_user_id).bind::<BigInt,_>(c.roles.authority_revision).bind::<BigInt,_>(now).bind::<BigInt,_>(expires).execute(db).await?;
            let result=view(db,&id,p,now).await?;notice(db,&result,now).await?;Self::audit(db,p,"agent.request",&id,now).await?;receipt(db,p,&input.command_id,"agent.request",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn decide_managed_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: DecideManagedAgent,
        gateway: &Gateway,
    ) -> Result<ManagedAgentRequest> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 || input.expected_authority_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let digest = digest(id, &input)?;
        let pre = {
            let mut db = self.db.lock().await?;
            (*db)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, crate::api::now_ms()).await?;
                    let now = clock(db).await?;
                    if let Some(saved) = replay::<ManagedAgentRequest>(
                        db,
                        p,
                        &input.command_id,
                        "agent.decide",
                        &digest,
                    )
                    .await?
                    {
                        return Ok(Err(view(db, &saved.id, p, now).await?));
                    }
                    let r = row(db, id).await?;
                    decision_ready(db, &r, p, &input, now).await?;
                    let c = if matches!(input.decision, Decision::Approve) {
                        let c = context(db, &r.plan_id, now).await?;
                        if !c.matches(&r) {
                            return Err(Error::Conflict("agent_request_binding_changed"));
                        }
                        Some(c)
                    } else {
                        None
                    };
                    Ok(Ok(c))
                })
                .await?
        };
        let pre = match pre {
            Err(saved) => return Ok(saved),
            Ok(c) => c,
        };
        let observed = if let Some(c) = &pre {
            Some(c.observe(gateway).await?)
        } else {
            None
        };
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if let Some(saved)=replay::<ManagedAgentRequest>(db,p,&input.command_id,"agent.decide",&digest).await?{return view(db,&saved.id,p,now).await;}
            let r=row(db,id).await?;decision_ready(db,&r,p,&input,now).await?;
            let (verdict,state)=if matches!(input.decision,Decision::Approve){("approve","approved_waiting_executor")}else{("reject","rejected")};
            let verdict_digest=allocation_digest(&json!({"version":1,"requestId":r.id,"requestRevision":r.revision,"administratorMxid":p.mxid,"authorityRevision":input.expected_authority_revision,"commandId":input.command_id,"decision":verdict}))?;
            if let Some(pre)=&pre {
                let c=context(db,&r.plan_id,now).await?;if !c.matches(&r)||c.pin()!=pre.pin(){return Err(Error::Conflict("agent_request_binding_changed"));}fresh(observed.unwrap(),now)?;
                let cmd=ManagedAgentReserveCommand{version:1,pool_grant:None,capability:AGENT_CAPABILITY.into(),operation:"agent.reserve".into(),command_id:format!("mae_{}",entity_id()?),request_id:r.id.clone(),request_revision:r.revision+1,proposed_agent_id:r.proposed_agent_id.clone(),manager_mxid:c.roles.manager_mxid.clone(),project_id:r.plan_id.clone(),project_revision:c.setup.current_binding.as_ref().unwrap().project_revision,setup_id:c.setup.id.clone(),setup_binding_revision:c.setup.current_binding.as_ref().unwrap().revision,domain_project_id:r.domain_project_id.clone(),room_id:r.room_id.clone(),authority_revision:c.roles.authority_revision,decision_actor_mxid:p.mxid.clone(),decision_digest:verdict_digest.clone(),allocation_id:c.capacity.id,allocation_receipt_digest:c.capacity.receipt_digest,grant_receipt_digest:c.capacity.grant_digest,enrollment_id:c.capacity.grant.enrollment_id,executor_device_id:c.capacity.grant.executor_device_id,executor_generation:c.capacity.grant.executor_generation,resource_id:c.capacity.grant.resource_id,resource_revision:c.capacity.grant.resource_revision,definition:decode(&r.definition)?,definition_digest:r.definition_digest.clone(),requested_tokens:r.requested_tokens,period_key:r.period_key.clone(),issued_at_ms:now,expires_at_ms:r.expires_at_ms};cmd.validate(&self.server)?;
                sql_query("INSERT INTO hagency_agent_v1.management_agent_intents(id,request_id,executor_owner_user_id,executor_device_id,executor_generation,payload,digest) VALUES($1,$2,$3,$4,$5,$6,$7)").bind::<Text,_>(&cmd.command_id).bind::<Text,_>(id).bind::<Text,_>(&c.owner).bind::<Text,_>(&cmd.executor_device_id).bind::<BigInt,_>(cmd.executor_generation).bind::<Text,_>(wire(&cmd)?).bind::<Text,_>(allocation_digest(&cmd)?).execute(db).await?;
            }
            sql_query("INSERT INTO hagency_agent_v1.management_agent_decisions(request_id,request_revision,administrator_user_id,authority_revision,command_id,digest,decision,decided_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind::<Text,_>(id).bind::<BigInt,_>(r.revision).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(input.expected_authority_revision).bind::<Text,_>(&input.command_id).bind::<Text,_>(&verdict_digest).bind::<Text,_>(verdict).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state=$2,revision=revision+1 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(state).execute(db).await?;
            let result=view(db,id,p,now).await?;notice(db,&result,now).await?;Self::audit(db,p,"agent.decide",id,now).await?;receipt(db,p,&input.command_id,"agent.decide",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn cancel_managed_agent_request(
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
            if let Some(saved)=replay::<ManagedAgentRequest>(db,p,&input.command_id,"agent.cancel",&digest).await?{return view(db,&saved.id,p,now).await;}
            let r=row(db,id).await?;if r.manager_user_id!=p.user_id{return Err(Error::Unauthorized("project_manager_required"));}
            if r.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
            if !matches!(r.state.as_str(),"pending_approval"|"approved_waiting_executor"){return Err(Error::Conflict("agent_request_closed"));}
            sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE request_id=$1").bind::<Text,_>(id).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state='cancelled',revision=revision+1 WHERE id=$1").bind::<Text,_>(id).execute(db).await?;
            let result=view(db,id,p,now).await?;notice(db,&result,now).await?;Self::audit(db,p,"agent.cancel",id,now).await?;receipt(db,p,&input.command_id,"agent.cancel",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn recover_managed_agent_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<Value> {
        if !matches!(input.operation.as_str(), "agent.cancel" | "agent.revoke") {
            enabled(policy)?;
        }
        key(id)?;
        let (device, _) = device(p)?;
        if !matches!(
            input.operation.as_str(),
            "agent.request" | "agent.decide" | "agent.cancel" | "agent.revoke"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(id).load::<Receipt>(db).await?;
            if rows.len()>1{return Err(Error::Conflict("management_command_conflict"));}let Some(r)=rows.as_slice().first()else{return Ok(json!({"recorded":false,"agentRequest":null}))};if r.operation!=input.operation{return Err(Error::Conflict("management_command_conflict"));}let saved:ManagedAgentRequest=decode(&r.result)?;Ok(json!({"recorded":true,"agentRequest":view(db,&saved.id,p,now).await?}))
        }).await
    }
}
async fn decision_ready(
    db: &mut AsyncPgConnection,
    r: &Row,
    p: &Principal,
    input: &DecideManagedAgent,
    now: i64,
) -> Result<()> {
    let a = roles(db, &r.plan_id).await?;
    if !a.active || a.administrator_user_id != p.user_id {
        return Err(Error::Unauthorized(
            "assigned_project_administrator_required",
        ));
    }
    if r.revision != input.expected_revision
        || a.authority_revision != input.expected_authority_revision
        || r.authority_revision != a.authority_revision
        || r.administrator_user_id != a.administrator_user_id
    {
        return Err(Error::Conflict("management_revision_changed"));
    }
    if r.state != "pending_approval" || r.expires_at_ms <= now {
        return Err(Error::Conflict("agent_request_closed"));
    }
    Ok(())
}
