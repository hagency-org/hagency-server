//! Manager-owned Matrix setup; no room creation, synthetic facts or agent launch.
#[path = "management_setup_recovery.rs"]
mod recovery;
#[path = "management_setup_service.rs"]
mod service;
use super::*;
use crate::{
    gateway::Gateway, management_allocation_types::allocation_digest, management_setup_types::*,
};
use serde_json::{Value, json};

const SETUPS: &str = "SELECT json_build_object('id',s.id,'projectId',s.plan_id,'allocationId',s.allocation_id,'allocationReceiptDigest',s.allocation_receipt_digest,'projectRevision',s.plan_revision,'authorityRevision',s.authority_revision,'definitionDigest',s.definition_digest,'mode',s.mode,'state',s.state,'revision',s.revision,'name',s.name,'managerMxid',s.manager_mxid,'administratorMxid',s.administrator_mxid,'spaceCreationCommandId',s.space_creation_command_id,'roomCreationCommandId',s.room_creation_command_id,'spaceId',s.space_id,'roomId',s.room_id,'domainProjectId',s.domain_project_id,'verifiedAtMs',s.verified_at_ms,'currentBinding',json_build_object('revision',b.revision,'allocationId',b.allocation_id,'allocationReceiptDigest',b.allocation_receipt_digest,'projectRevision',b.plan_revision,'authorityRevision',b.authority_revision,'administratorMxid',b.administrator_mxid))::text AS payload FROM hagency_agent_v1.management_project_setups s JOIN hagency_agent_v1.management_setup_bindings b ON b.setup_id=s.id AND b.revision=s.current_binding_revision";
#[derive(QueryableByName)]
struct Plan {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    manager_mxid: String,
    #[diesel(sql_type=Text)]
    contribution_id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    name: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
}
#[derive(QueryableByName)]
struct Authority {
    #[diesel(sql_type=Text)]
    administrator_user_id: String,
    #[diesel(sql_type=Text)]
    mxid: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=Bool)]
    active: bool,
}
#[derive(QueryableByName)]
struct Payload {
    #[diesel(sql_type=Text)]
    payload: String,
}
fn enabled(policy: &ManagementPolicy) -> Result<()> {
    if policy.project_setup() {
        Ok(())
    } else {
        Err(Error::Unavailable("project_setup_unavailable"))
    }
}
async fn plan(db: &mut AsyncPgConnection, id: &str) -> Result<Plan> {
    sql_query("SELECT p.id,p.manager_user_id,u.mxid AS manager_mxid,p.contribution_id,p.request_id,p.name,p.state,p.revision FROM hagency_agent_v1.management_projects p JOIN hagency_agent_v1.users u ON u.id=p.manager_user_id WHERE p.id=$1")
        .bind::<Text,_>(id).get_result(db).await.map_err(hidden)
}
async fn authority(db: &mut AsyncPgConnection, id: &str) -> Result<Option<Authority>> {
    Ok(sql_query("SELECT a.administrator_user_id,u.mxid,u.active,a.revision FROM hagency_agent_v1.management_project_authorities a JOIN hagency_agent_v1.users u ON u.id=a.administrator_user_id WHERE a.plan_id=$1 FOR SHARE OF a,u")
        .bind::<Text,_>(id).get_result(db).await.optional()?)
}
pub(super) async fn is_administrator(
    db: &mut AsyncPgConnection,
    id: Option<&str>,
    p: &Principal,
) -> Result<bool> {
    let Some(id) = id else { return Ok(false) };
    Ok(authority(db, id)
        .await?
        .is_some_and(|a| a.active && a.administrator_user_id == p.user_id))
}
pub(super) async fn setup(
    db: &mut AsyncPgConnection,
    plan_id: &str,
) -> Result<Option<ProjectSetup>> {
    sql_query(format!("{SETUPS} WHERE s.plan_id=$1"))
        .bind::<Text, _>(plan_id)
        .get_result::<Payload>(db)
        .await
        .optional()?
        .map(|r| decode(&r.payload))
        .transpose()
}
async fn setup_id(db: &mut AsyncPgConnection, id: &str) -> Result<ProjectSetup> {
    let raw = sql_query(format!("{SETUPS} WHERE s.id=$1"))
        .bind::<Text, _>(id)
        .get_result::<Payload>(db)
        .await
        .map_err(hidden)?;
    decode(&raw.payload)
}
async fn visible_plan(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
) -> Result<Plan> {
    let plan = plan(db, id).await?;
    let (owner, c) = contribution(db, &plan.contribution_id).await?;
    if plan.manager_user_id != p.user_id
        && !visible(p, policy, &owner, &c)
        && !is_administrator(db, Some(id), p).await?
    {
        return Err(Error::NotFound("project_setup_not_found"));
    }
    Ok(plan)
}
fn binding(s: &ProjectSetup) -> Result<&SetupBinding> {
    s.current_binding
        .as_ref()
        .ok_or(Error::Unavailable("project_setup_binding_missing"))
}
async fn view(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ProjectReadiness> {
    let plan = visible_plan(db, id, p, policy).await?;
    let auth = authority(db, id).await?;
    let setup = setup(db, id).await?;
    let current = setup.as_ref().map(binding).transpose()?;
    let eligible = if policy.project_reservations() {
        allocations::setup_capacity(db, id, None, now).await?
    } else {
        None
    };
    let pinned = if let Some(current) = current.filter(|_| policy.project_reservations()) {
        allocations::setup_capacity(db, id, Some(&current.allocation_id), now).await?
    } else {
        None
    };
    let capacity = if current.is_some() {
        pinned.as_ref()
    } else {
        eligible.as_ref()
    };
    let capacity_ready = capacity.is_some_and(|c| {
        current.is_none_or(|b| {
            b.allocation_receipt_digest == c.receipt_digest && b.project_revision == plan.revision
        })
    });
    let assigned = auth.as_ref().is_some_and(|a| a.active);
    let authority_current = auth.as_ref().is_some_and(|a| {
        a.active
            && current.is_none_or(|b| {
                b.authority_revision == a.revision && b.administrator_mxid == a.mxid
            })
    });
    let matrix_ready = setup.as_ref().is_some_and(|s| {
        s.state == "ready"
            && authority_current
            && s.verified_at_ms
                .is_some_and(|t| t <= now && now - t < MAX_FACT_AGE_MS)
    });
    let cancelled = setup.as_ref().is_some_and(|s| s.state == "cancelled");
    let reason = if cancelled {
        Some("project_setup_cancelled")
    } else if plan.state != "approved" {
        Some("project_unavailable")
    } else if !assigned {
        Some("project_administrator_required")
    } else if !authority_current {
        Some("project_administrator_changed")
    } else if !capacity_ready {
        Some("live_reservation_required")
    } else if setup.is_none() {
        Some("matrix_setup_required")
    } else if !matrix_ready {
        Some("matrix_verification_required")
    } else {
        None
    };
    Ok(ProjectReadiness {
        project_id: id.into(),
        administrator: auth.map(|a| ProjectAdministrator {
            mxid: a.mxid,
            revision: a.revision,
        }),
        can_assign_administrator: false,
        can_begin: plan.manager_user_id == p.user_id
            && plan.state == "approved"
            && authority_current
            && capacity_ready
            && setup.is_none(),
        can_confirm: plan.manager_user_id == p.user_id
            && authority_current
            && capacity_ready
            && setup.is_some()
            && !cancelled,
        can_cancel: setup.is_some()
            && !cancelled
            && (plan.manager_user_id == p.user_id || p.mxid == policy.administrator()),
        can_rebase: setup.is_some()
            && plan.manager_user_id == p.user_id
            && plan.state == "approved"
            && assigned
            && eligible.is_some(),
        eligible_allocation: eligible.map(|c| EligibleSetupAllocation {
            id: c.id,
            revision: c.revision,
            project_revision: plan.revision,
            receipt_digest: c.receipt_digest,
        }),
        setup,
        capacity_ready,
        matrix_ready,
        blocking_reason: reason.map(str::to_owned),
        observed_at_ms: now,
    })
}
async fn changed(
    db: &mut AsyncPgConnection,
    plan: &Plan,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_requests SET revision=revision+1 WHERE id=$1")
        .bind::<Text, _>(&plan.request_id)
        .execute(db)
        .await?;
    let r = request(db, &plan.request_id, p, policy, now).await?;
    let mut recipients = vec![plan.manager_mxid.clone(), policy.administrator().into()];
    if let Some(a) = authority(db, &plan.id).await? {
        recipients.push(a.mxid);
    }
    notify(db, &r, recipients, now).await
}

impl DomainStore {
    pub async fn management_project_setup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                view(db, id, p, policy, now).await
            })
            .await
    }
    pub async fn assign_management_project_admin(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: AssignProjectAdmin,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        local_user(&input.administrator_mxid, &self.server)?;
        if input.expected_authority_revision < 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if p.mxid!=policy.administrator(){return Err(Error::Unauthorized("management_administrator_required"));}
            if let Some(saved)=replay::<ProjectReadiness>(db,p,&input.command_id,"project.admin.assign",&digest).await?{return view(db,&saved.project_id,p,policy,now).await;}
            let plan=plan(db,id).await?;
            if plan.state!="approved"{return Err(Error::Conflict("project_unavailable"));}
            // Compatibility for saved/older clients: acknowledging the manager
            // is harmless; assigning a separate project administrator is retired.
            if input.administrator_mxid!=plan.manager_mxid{return Err(Error::Conflict("project_manager_is_administrator"));}
            let prior=authority(db,id).await?.ok_or(Error::Conflict("project_administrator_required"))?;
            if input.expected_authority_revision!=0 && prior.revision!=input.expected_authority_revision{return Err(Error::Conflict("management_revision_changed"));}
            let result=view(db,id,p,policy,now).await?;
            Self::audit(db,p,"project.admin.assign",id,now).await?;receipt(db,p,&input.command_id,"project.admin.assign",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn begin_management_project_setup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: BeginProjectSetup,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        if let Some(allocation_id) = &input.allocation_id {
            key(allocation_id)?;
        }
        if !matches!(input.mode.as_str(), "create" | "adopt")
            || [
                input.expected_project_revision,
                input.expected_allocation_revision,
                input.expected_authority_revision,
            ]
            .iter()
            .any(|n| *n <= 0)
        {
            return Err(Error::Invalid("invalid_project_setup"));
        }
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let plan=plan(db,id).await?;
            if plan.manager_user_id!=p.user_id{return Err(Error::Unauthorized("project_manager_required"));}
            if let Some(saved)=replay::<ProjectReadiness>(db,p,&input.command_id,"project.setup.begin",&digest).await?{return view(db,&saved.project_id,p,policy,now).await;}
            let allocation_id=input.allocation_id.as_deref().ok_or(Error::Invalid("project_setup_allocation_required"))?;
            let auth=authority(db,id).await?.filter(|a|a.active).ok_or(Error::Conflict("project_administrator_required"))?;
            let capacity=allocations::setup_capacity(db,id,Some(allocation_id),now).await?.filter(|_|policy.project_reservations()).ok_or(Error::Conflict("live_reservation_required"))?;
            if plan.revision!=input.expected_project_revision || auth.revision!=input.expected_authority_revision || capacity.revision!=input.expected_allocation_revision{return Err(Error::Conflict("management_revision_changed"));}
            if setup(db,id).await?.is_some(){return Err(Error::Conflict("project_setup_already_exists"));}
            let setup_id=format!("mps_{}",entity_id()?);
            let definition=allocation_digest(&json!({"version":1,"setupId":setup_id,"projectId":id,"projectRevision":plan.revision,"allocationId":capacity.id,"allocationReceiptDigest":capacity.receipt_digest,"managerMxid":plan.manager_mxid,"administratorMxid":auth.mxid,"authorityRevision":auth.revision,"name":plan.name,"mode":input.mode}))?;
            sql_query("INSERT INTO hagency_agent_v1.management_project_setups(id,plan_id,allocation_id,manager_user_id,plan_revision,allocation_receipt_digest,authority_revision,definition_digest,mode,state,name,manager_mxid,administrator_mxid,space_creation_command_id,room_creation_command_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'awaiting_matrix_setup',$10,$11,$12,$13,$14)")
                .bind::<Text,_>(&setup_id).bind::<Text,_>(id).bind::<Text,_>(&capacity.id).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(plan.revision).bind::<Text,_>(&capacity.receipt_digest).bind::<BigInt,_>(auth.revision).bind::<Text,_>(&definition).bind::<Text,_>(&input.mode).bind::<Text,_>(&plan.name).bind::<Text,_>(&plan.manager_mxid).bind::<Text,_>(&auth.mxid).bind::<Text,_>(format!("{setup_id}_space")).bind::<Text,_>(format!("{setup_id}_room")).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_setup_bindings(setup_id,revision,allocation_id,allocation_receipt_digest,plan_revision,authority_revision,administrator_mxid) VALUES($1,1,$2,$3,$4,$5,$6)").bind::<Text,_>(&setup_id).bind::<Text,_>(&capacity.id).bind::<Text,_>(&capacity.receipt_digest).bind::<BigInt,_>(plan.revision).bind::<BigInt,_>(auth.revision).bind::<Text,_>(&auth.mxid).execute(db).await?;
            changed(db,&plan,p,policy,now).await?;let result=view(db,id,p,policy,now).await?;Self::audit(db,p,"project.setup.begin",&setup_id,now).await?;receipt(db,p,&input.command_id,"project.setup.begin",&digest,&result).await?;Ok(result)
        }).await
    }
}

pub(super) fn state<'a>(events: &'a [Value], kind: &str, key: &str) -> Result<&'a Value> {
    let mut rows = events
        .iter()
        .filter(|e| e["type"] == kind && e["state_key"] == key);
    let value = rows
        .next()
        .ok_or(Error::Conflict("project_matrix_proof_missing"))?;
    if rows.next().is_some() {
        return Err(Error::Conflict("project_matrix_proof_conflict"));
    }
    Ok(value)
}
pub(super) fn marker(events: &[Value], s: &ProjectSetup, kind: &str, space: &str) -> Result<()> {
    let create = state(events, "m.room.create", "")?;
    let mark = state(events, "im.hagency.management.project_setup", "")?;
    let command = if kind == "space" {
        &s.space_creation_command_id
    } else {
        &s.room_creation_command_id
    };
    let expected = json!({"version":1,"setupId":s.id,"projectId":s.project_id,"definitionDigest":s.definition_digest,"commandId":command,"owner":s.manager_mxid,"kind":kind,"spaceId":if kind=="space"{Value::Null}else{json!(space)}});
    if create["sender"] != s.manager_mxid
        || create["content"]
            .get("creator")
            .is_some_and(|v| v != &s.manager_mxid)
        || mark["sender"] != s.manager_mxid
        || mark["content"] != expected
    {
        return Err(Error::Conflict("project_matrix_marker_mismatch"));
    }
    Ok(())
}
pub(super) fn joined(events: &[Value], mxid: &str) -> Result<()> {
    if state(events, "m.room.member", mxid)?["content"]["membership"] != "join" {
        return Err(Error::Conflict("project_membership_pending"));
    }
    Ok(())
}
impl DomainStore {
    pub async fn confirm_management_project_setup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ConfirmProjectSetup,
        gateway: &Gateway,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        matrix_id(&input.space_id, '!')?;
        matrix_id(&input.room_id, '!')?;
        if input.expected_revision <= 0 || input.space_id == input.room_id {
            return Err(Error::Invalid("invalid_project_setup"));
        }
        let digest = digest(id, &input)?;
        // Reauthorize before any privileged state read. Replay never repeats a
        // Matrix mutation; this endpoint performs only trusted observations.
        let (s, replayed) = {
            let mut db = self.db.lock().await?;
            (*db)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, crate::api::now_ms()).await?;
                    let s = setup_id(db, id).await?;
                    if s.manager_mxid != p.mxid {
                        return Err(Error::Unauthorized("project_manager_required"));
                    }
                    let replayed = if let Some(saved) = replay::<ProjectReadiness>(
                        db,
                        p,
                        &input.command_id,
                        "project.setup.confirm",
                        &digest,
                    )
                    .await?
                    {
                        {
                            let now = clock(db).await?;
                            Some(view(db, &saved.project_id, p, policy, now).await?)
                        }
                    } else {
                        None
                    };
                    if replayed.is_none() {
                        if s.revision != input.expected_revision {
                            return Err(Error::Conflict("management_revision_changed"));
                        }
                        if s.state == "cancelled" {
                            return Err(Error::Conflict("project_setup_cancelled"));
                        }
                        let current = authority(db, &s.project_id)
                            .await?
                            .filter(|a| a.active)
                            .ok_or(Error::Conflict("project_administrator_required"))?;
                        if current.revision != binding(&s)?.authority_revision
                            || current.mxid != binding(&s)?.administrator_mxid
                        {
                            return Err(Error::Conflict("project_administrator_changed"));
                        }
                    }
                    Ok((s, replayed))
                })
                .await?
        };
        if let Some(result) = replayed {
            return Ok(result);
        }
        let facts_started = crate::api::now_ms();
        let (sf, rf, room_facts, space_state, room_state) = gateway
            .project_setup(&input.space_id, &input.room_id, &p.mxid)
            .await?;
        let checked_at = crate::api::now_ms();
        administer(p, &sf, &input.space_id, checked_at)?;
        administer(p, &rf, &input.room_id, checked_at)?;
        membership(p, &room_facts, &input.room_id, &input.space_id, checked_at)?;
        for events in [&space_state, &room_state] {
            joined(events, &binding(&s)?.administrator_mxid)?;
            if state(events, "m.room.join_rules", "")?["content"]["join_rule"] != "invite" {
                return Err(Error::Conflict("project_private_room_required"));
            }
        }
        if state(&room_state, "m.space.parent", &input.space_id)?["content"]["via"]
            .as_array()
            .is_none_or(|a| a.is_empty())
        {
            return Err(Error::Conflict("project_matrix_proof_missing"));
        }
        if s.mode == "create" {
            marker(&space_state, &s, "space", &input.space_id)?;
            marker(&room_state, &s, "room", &input.space_id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if let Some(saved)=replay::<ProjectReadiness>(db,p,&input.command_id,"project.setup.confirm",&digest).await?{return view(db,&saved.project_id,p,policy,now).await;}
            let current=setup_id(db,id).await?;let plan=plan(db,&current.project_id).await?;
            if plan.manager_user_id!=p.user_id{return Err(Error::Unauthorized("project_manager_required"));}
            if current.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
            if current.state=="cancelled"{return Err(Error::Conflict("project_setup_cancelled"));}
            let current_binding=binding(&current)?;
            if current.definition_digest!=s.definition_digest{return Err(Error::Conflict("project_setup_changed"));}
            if current.space_id.as_ref().is_some_and(|r|r!=&input.space_id)||current.room_id.as_ref().is_some_and(|r|r!=&input.room_id){return Err(Error::Conflict("project_matrix_link_immutable"));}
            let auth=authority(db,&plan.id).await?.filter(|a|a.active).ok_or(Error::Conflict("project_administrator_required"))?;
            if auth.revision!=current_binding.authority_revision || auth.mxid!=current_binding.administrator_mxid{return Err(Error::Conflict("project_administrator_changed"));}
            let capacity=allocations::setup_capacity(db,&plan.id,Some(&current_binding.allocation_id),now).await?.filter(|_|policy.project_reservations()).ok_or(Error::Conflict("live_reservation_required"))?;
            if capacity.receipt_digest!=current_binding.allocation_receipt_digest || plan.revision!=current_binding.project_revision{return Err(Error::Conflict("project_setup_changed"));}
            fresh(facts_started,now)?;
            administer(p,&sf,&input.space_id,now)?;administer(p,&rf,&input.room_id,now)?;membership(p,&room_facts,&input.room_id,&input.space_id,now)?;
            if !sf.is_space || rf.is_space || rf.linked_space_id.as_deref()!=Some(input.space_id.as_str()) || room_facts.encrypted || !room_facts.service_can_invite{return Err(Error::Conflict("project_matrix_not_ready"));}
            let project=Self::register_project_transaction(db,p,&input.space_id,now).await?;
            if !project.active {return Err(Error::Conflict("project_disabled"));}
            let occupied=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE project_id=$1 OR room_id=$2 OR room_id IN (SELECT room_id FROM hagency_agent_v1.rooms WHERE project_id=$1)) AS matched").bind::<Text,_>(&project.id).bind::<Text,_>(&input.room_id).get_result::<Flag>(db).await?.matched;
            if occupied{return Err(Error::Conflict("managed_project_has_bindings"));}
            let room=Self::register_room_transaction(db,p,&project.id,&input.room_id,now).await?;
            if !room.active {return Err(Error::Conflict("room_disabled"));}
            sql_query("UPDATE hagency_agent_v1.management_project_setups SET state='ready',revision=revision+1,space_id=$2,room_id=$3,domain_project_id=$4,verified_at_ms=$5 WHERE id=$1")
                .bind::<Text,_>(id).bind::<Text,_>(&input.space_id).bind::<Text,_>(&input.room_id).bind::<Text,_>(&project.id).bind::<BigInt,_>(now).execute(db).await?;
            changed(db,&plan,p,policy,now).await?;let result=view(db,&plan.id,p,policy,now).await?;Self::audit(db,p,"project.setup.confirm",id,now).await?;receipt(db,p,&input.command_id,"project.setup.confirm",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn recover_management_setup_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<Value> {
        enabled(policy)?;
        key(id)?;
        let (device, _) = device(p)?;
        if !matches!(
            input.operation.as_str(),
            "project.admin.assign"
                | "project.setup.begin"
                | "project.setup.confirm"
                | "project.setup.cancel"
                | "project.setup.rebase"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(id).load::<Receipt>(db).await?;
            if rows.len()>1{return Err(Error::Conflict("management_command_conflict"));}
            let Some(row)=rows.as_slice().first()else{
                // This serialized absence check is terminal evidence for a
                // retired assignment. Older unknown journals need not retry it.
                if input.operation=="project.admin.assign" {return Err(Error::Conflict("project_administrator_assignment_retired"));}
                return Ok(json!({"recorded":false}));
            };
            if row.operation!=input.operation{return Err(Error::Conflict("management_command_conflict"));}
            let saved:ProjectReadiness=decode(&row.result)?;
            Ok(json!({"recorded":true,"readiness":view(db,&saved.project_id,p,policy,now).await?}))
        }).await
    }
}
