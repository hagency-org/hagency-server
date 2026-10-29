//! Shared project intent. Approval never reserves executor capacity or creates
//! an executable Matrix project; those require a separate allocation protocol.
use super::*;

// The native authenticated transport accepts at most 128 KiB. Count actual
// escaped JSON, leaving room for the protocol envelope and HTTP projection.
const MAX_PROJECT_PAGE_BYTES: usize = 96 * 1024;

const PROJECTS: &str = "SELECT m.id,m.manager_user_id,m.contribution_id,m.request_id,json_build_object('id',m.id,'managerMxid',u.mxid,'contributionId',m.contribution_id,'resourceLabel',c.label,'coordinatorMxid',c.coordinator_mxid,'name',m.name,'purpose',m.purpose,'requestedTokens',m.requested_tokens,'state',m.state,'revision',m.revision,'createdAtMs',m.created_at_ms,'updatedAtMs',m.updated_at_ms)::text AS payload FROM hagency_agent_v1.management_projects m JOIN hagency_agent_v1.users u ON u.id=m.manager_user_id JOIN hagency_agent_v1.management_contributions c ON c.id=m.contribution_id";

#[derive(QueryableByName)]
struct ProjectRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    contribution_id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
}

async fn project_row(db: &mut AsyncPgConnection, id: &str) -> Result<ProjectRow> {
    sql_query(format!("{PROJECTS} WHERE m.id=$1"))
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}

pub(super) async fn project(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ProjectPlan> {
    let row = project_row(db, id).await?;
    let (owner, c) = contribution(db, &row.contribution_id).await?;
    if row.manager_user_id != p.user_id
        && !visible(p, policy, &owner, &c)
        && !super::setup::is_administrator(db, Some(id), p).await?
    {
        return Err(Error::NotFound("project_request_not_found"));
    }
    let r = request(db, &row.request_id, p, policy, now).await?;
    let mut payload: serde_json::Value = decode(&row.payload)?;
    if r.state == "expired" {
        payload["state"] = "expired".into();
    }
    payload["allocationStatus"] = if c.state != "admitted" {
        "resource_unavailable"
    } else {
        match payload["state"].as_str() {
            Some("pending_approval") => "awaiting_approval",
            Some("approved") => "awaiting_allocation",
            _ => "none",
        }
    }
    .into();
    payload["canRevoke"] = ((row.manager_user_id == p.user_id || p.mxid == policy.administrator())
        && matches!(
            payload["state"].as_str(),
            Some("pending_approval" | "approved")
        ))
    .into();
    payload["request"] =
        serde_json::to_value(r).map_err(|_| Error::Unavailable("encoding_failed"))?;
    serde_json::from_value(payload).map_err(|_| Error::Unavailable("encoding_failed"))
}

async fn close_request(
    db: &mut AsyncPgConnection,
    project_id: &str,
    request_id: &str,
    state: &str,
    now: i64,
) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_projects SET state=$2,revision=revision+1,updated_at_ms=$3 WHERE id=$1")
        .bind::<Text,_>(project_id).bind::<Text,_>(state).bind::<BigInt,_>(now).execute(db).await?;
    sql_query("UPDATE hagency_agent_v1.management_requests SET state=$2,revision=revision+1,decided_at_ms=$3 WHERE id=$1")
        .bind::<Text,_>(request_id).bind::<Text,_>(state).bind::<BigInt,_>(now).execute(db).await?;
    sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'")
        .bind::<Text,_>(request_id).execute(db).await?;
    Ok(())
}

/// A resource withdrawal closes its outstanding plans in the same transaction.
/// No downstream worker can observe an actionable request for withdrawn capacity.
pub(super) async fn revoke_contribution_projects(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    contribution_id: &str,
    now: i64,
) -> Result<()> {
    let rows = sql_query(format!("{PROJECTS} WHERE m.contribution_id=$1 AND m.state IN ('pending_approval','approved') ORDER BY m.id"))
        .bind::<Text,_>(contribution_id).load::<ProjectRow>(db).await?;
    for row in rows {
        close_request(db, &row.id, &row.request_id, "revoked", now).await?;
        super::allocations::release_for_project(db, p, policy, &row.id, now).await?;
        let result = project(db, &row.id, p, policy, now).await?;
        notify(
            db,
            &result.request,
            [
                result.manager_mxid,
                result.coordinator_mxid,
                policy.administrator().to_owned(),
            ],
            now,
        )
        .await?;
    }
    Ok(())
}

impl DomainStore {
    /// Discovery intentionally omits owner/device/resource identifiers and any
    /// token totals: only the native executor ledger can confirm free capacity.
    pub async fn management_catalog(
        &self,
        p: &Principal,
        query: CatalogQuery,
    ) -> Result<ManagementPage<CatalogResource>> {
        if let Some(after) = &query.after {
            key(after)?;
        }
        if let Some(capability) = &query.capability
            && (capability.is_empty()
                || capability.len() > 80
                || !capability
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b)))
        {
            return Err(Error::Invalid("invalid_resource_capability"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let rows=sql_query(format!("{CONTRIBUTIONS} WHERE c.state='admitted' AND c.id>$1 AND ($2::text IS NULL OR c.capabilities::jsonb ? $2) ORDER BY c.id LIMIT $3"))
                .bind::<Text,_>(query.after.as_deref().unwrap_or(""))
                .bind::<Nullable<Text>,_>(query.capability.as_deref()).bind::<BigInt,_>(MAX_PAGE+1).load::<ContributionRow>(db).await?;
            let more=rows.len()>MAX_PAGE as usize;
            let items=rows.into_iter().take(MAX_PAGE as usize).map(|row| {
                let c:Contribution=decode(&row.payload)?;
                Ok(CatalogResource{id:c.id,label:c.label,runtime:c.runtime,capabilities:c.capabilities,coordinator_mxid:c.coordinator_mxid,verification:c.verification})
            }).collect::<Result<Vec<_>>>()?;
            let next_after=if more {items.last().map(|v|v.id.clone())}else{None};
            Ok(ManagementPage{protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }

    pub async fn request_management_project(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ProjectRequestInput,
    ) -> Result<ProjectPlan> {
        input.validate()?;
        device(p)?;
        let payload_digest = digest("", &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            if let Some(saved)=replay::<ProjectPlan>(db,p,&input.command_id,"project.request",&payload_digest).await? {
                return project(db,&saved.id,p,policy,now).await;
            }
            let (_,c)=contribution(db,&input.contribution_id).await?;
            if c.state!="admitted" {return Err(Error::Conflict("contribution_not_admitted"));}
            let full=sql_query("SELECT count(*)>=1000 AS matched FROM hagency_agent_v1.management_projects WHERE manager_user_id=$1")
                .bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
            if full {return Err(Error::Conflict("project_request_capacity"));}
            let full=sql_query("SELECT count(*)>=1000 AS matched FROM hagency_agent_v1.management_projects WHERE contribution_id=$1 AND state IN ('pending_approval','approved')")
                .bind::<Text,_>(&c.id).get_result::<Flag>(db).await?.matched;
            if full {return Err(Error::Conflict("project_resource_capacity"));}
            let id=format!("mpr_{}",entity_id()?);
            let request_id=format!("req_{}",entity_id()?);
            sql_query("INSERT INTO hagency_agent_v1.management_projects(id,manager_user_id,contribution_id,name,purpose,requested_tokens,state,request_id,created_at_ms,updated_at_ms) VALUES($1,$2,$3,$4,$5,$6,'pending_approval',$7,$8,$8)")
                .bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&c.id).bind::<Text,_>(input.name.trim()).bind::<Text,_>(input.purpose.trim()).bind::<BigInt,_>(input.requested_tokens).bind::<Text,_>(&request_id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,project_id,requester_user_id,approver_mxid,state,created_at_ms,expires_at_ms) VALUES($1,'project_capacity',$2,$3,$4,$5,'pending',$6,$7)")
                .bind::<Text,_>(&request_id).bind::<Text,_>(&c.id).bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(policy.administrator()).bind::<BigInt,_>(now).bind::<BigInt,_>(now+TTL).execute(db).await?;
            let result=project(db,&id,p,policy,now).await?;
            notify(db,&result.request,[policy.administrator().to_owned()],now).await?;
            Self::audit(db,p,"project.request",&id,now).await?;
            receipt(db,p,&input.command_id,"project.request",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }

    pub async fn management_project(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ProjectPlan> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                project(db, id, p, policy, now).await
            })
            .await
    }

    pub async fn management_projects(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        query: ManagementQuery,
    ) -> Result<ManagementPage<ProjectPlan>> {
        query.validate()?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let rows=sql_query(format!("{PROJECTS} JOIN hagency_agent_v1.management_requests r ON r.id=m.request_id WHERE (m.manager_user_id=$1 OR c.owner_user_id=$1 OR c.coordinator_mxid=$2 OR $2=$3 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_authorities pa WHERE pa.plan_id=m.id AND pa.administrator_user_id=$1)) AND (NOT $4 OR r.approver_mxid=$2 AND $2=$3 AND r.state='pending' AND r.expires_at_ms>$5 AND c.state='admitted') AND m.id>$6 ORDER BY m.id LIMIT $7"))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).bind::<Bool,_>(query.needs_my_action).bind::<BigInt,_>(now).bind::<Text,_>(query.after.as_deref().unwrap_or("")).bind::<BigInt,_>(MAX_PAGE+1).load::<ProjectRow>(db).await?;
            let mut more=rows.len()>MAX_PAGE as usize;
            let mut items=Vec::new();
            let mut bytes=512;
            for row in rows.into_iter().take(MAX_PAGE as usize) {
                let item=project(db,&row.id,p,policy,now).await?;
                let size=wire(&item)?.len()+1;
                if bytes+size>MAX_PROJECT_PAGE_BYTES {more=true;break;}
                bytes+=size;
                items.push(item);
            }
            let next_after=if more {items.last().map(|v|v.id.clone())}else{None};
            Ok(ManagementPage{protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }

    pub async fn decide_management_project(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: DecisionInput,
    ) -> Result<ProjectPlan> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                if p.mxid != policy.administrator() {
                    return Err(Error::Unauthorized("management_administrator_required"));
                }
                let now = clock(db).await?;
                if let Some(saved) = replay::<ProjectPlan>(
                    db,
                    p,
                    &input.command_id,
                    "project.decide",
                    &payload_digest,
                )
                .await?
                {
                    return project(db, &saved.id, p, policy, now).await;
                }
                let r = request(db, id, p, policy, now).await?;
                if r.kind != "project_capacity" {
                    return Err(Error::Invalid("management_operation_mismatch"));
                }
                if r.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                if !r.can_decide {
                    return Err(Error::Conflict("management_action_closed"));
                }
                let project_id = r
                    .project_id
                    .as_deref()
                    .ok_or(Error::Unavailable("invalid_project_record"))?;
                let state = match input.decision {
                    Decision::Approve => "approved",
                    Decision::Reject => "rejected",
                };
                close_request(db, project_id, id, state, now).await?;
                super::allocations::seed_for_project(db, policy, project_id, now).await?;
                let result = project(db, project_id, p, policy, now).await?;
                notify(
                    db,
                    &result.request,
                    [result.manager_mxid.clone(), result.coordinator_mxid.clone()],
                    now,
                )
                .await?;
                Self::audit(db, p, "project.decide", id, now).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "project.decide",
                    &payload_digest,
                    &result,
                )
                .await?;
                Ok(result)
            })
            .await
    }

    pub async fn revoke_management_project(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<ProjectPlan> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let row = project_row(db, id).await?;
                if row.manager_user_id != p.user_id && p.mxid != policy.administrator() {
                    return Err(Error::Unauthorized("project_manager_required"));
                }
                if let Some(saved) = replay::<ProjectPlan>(
                    db,
                    p,
                    &input.command_id,
                    "project.revoke",
                    &payload_digest,
                )
                .await?
                {
                    return project(db, &saved.id, p, policy, now).await;
                }
                let current = project(db, id, p, policy, now).await?;
                if current.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                if !matches!(current.state.as_str(), "pending_approval" | "approved") {
                    return Err(Error::Conflict("management_action_closed"));
                }
                close_request(db, id, &row.request_id, "revoked", now).await?;
                super::allocations::release_for_project(db, p, policy, id, now).await?;
                let result = project(db, id, p, policy, now).await?;
                notify(
                    db,
                    &result.request,
                    [
                        result.manager_mxid.clone(),
                        result.coordinator_mxid.clone(),
                        policy.administrator().to_owned(),
                    ],
                    now,
                )
                .await?;
                Self::audit(db, p, "project.revoke", id, now).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "project.revoke",
                    &payload_digest,
                    &result,
                )
                .await?;
                Ok(result)
            })
            .await
    }

    pub async fn recover_management_project_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        command: &str,
        input: RecoverCommandInput,
    ) -> Result<Option<ProjectPlan>> {
        key(command)?;
        if !matches!(
            input.operation.as_str(),
            "project.request" | "project.decide" | "project.revoke"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let (executor, _) = device(p)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(command).load::<Receipt>(db).await?;
            if rows.len()>1 {return Err(Error::Conflict("management_command_conflict"));}
            let Some(row)=rows.as_slice().first() else {return Ok(None)};
            if row.operation!=input.operation {return Err(Error::Conflict("management_command_conflict"));}
            let saved:ProjectPlan=decode(&row.result)?;
            let now=clock(db).await?;
            Ok(Some(project(db,&saved.id,p,policy,now).await?))
        }).await
    }
}
