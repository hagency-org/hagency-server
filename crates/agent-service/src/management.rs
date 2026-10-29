//! Shared resource admission. The contributor's native ledger remains the sole
//! reservation authority; an admission never creates capacity or starts a model.
use super::*;
use crate::management_types::*;
use diesel::{OptionalExtension, QueryableByName};
use serde::de::DeserializeOwned;

#[path = "management_agents.rs"]
mod agents;
#[path = "management_allocations.rs"]
mod allocations;
#[path = "management_parents.rs"]
mod parents;
#[path = "management_projects.rs"]
mod projects;
#[path = "management_setup.rs"]
mod setup;
#[path = "management_usage.rs"]
mod usage;

const CONTRIBUTIONS: &str = "SELECT c.owner_user_id,json_build_object('id',c.id,'ownerMxid',u.mxid,'executorDeviceId',c.executor_device_id,'executorGeneration',c.executor_generation,'resourceId',c.resource_id,'resourceRevision',c.resource_revision,'label',c.label,'runtime',c.runtime,'capabilities',c.capabilities::json,'coordinatorMxid',c.coordinator_mxid,'state',c.state,'revision',c.revision,'requestId',c.request_id,'createdAtMs',c.created_at_ms,'updatedAtMs',c.updated_at_ms,'verification',CASE WHEN v.contribution_id IS NULL THEN NULL ELSE json_build_object('status',CASE WHEN c.state='admitted' AND v.contribution_revision=c.revision AND v.executor_generation=c.executor_generation AND v.resource_revision=c.resource_revision AND v.fresh_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint AND EXISTS(SELECT 1 FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE d.id=c.executor_device_id AND d.user_id=c.owner_user_id AND d.generation=c.executor_generation AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint AND u.active) THEN 'verified' ELSE 'stale' END,'verifiedAtMs',v.verified_at_ms,'freshUntilMs',v.fresh_until_ms) END)::text AS payload FROM hagency_agent_v1.management_contributions c JOIN hagency_agent_v1.users u ON u.id=c.owner_user_id LEFT JOIN hagency_agent_v1.management_verifications v ON v.contribution_id=c.id";
const REQUESTS: &str = "SELECT r.requester_user_id,(r.kind='resource_admission' OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_contributions available WHERE available.id=r.contribution_id AND available.state='admitted')) AS resource_available,json_build_object('id',r.id,'kind',r.kind,'contributionId',r.contribution_id,'projectId',r.project_id,'allocationId',r.allocation_id,'projectName',(SELECT m.name FROM hagency_agent_v1.management_projects m WHERE m.id=r.project_id),'resourceLabel',(SELECT c.label FROM hagency_agent_v1.management_contributions c WHERE c.id=r.contribution_id),'requesterMxid',u.mxid,'approverMxid',r.approver_mxid,'state',r.state,'revision',r.revision,'createdAtMs',r.created_at_ms,'expiresAtMs',r.expires_at_ms,'decidedAtMs',r.decided_at_ms,'canDecide',false)::text AS payload FROM hagency_agent_v1.management_requests r JOIN hagency_agent_v1.users u ON u.id=r.requester_user_id";
const TTL: i64 = 7 * 24 * 60 * 60 * 1000;
#[derive(QueryableByName)]
struct ContributionRow {
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
}
#[derive(QueryableByName)]
struct RequestRow {
    #[diesel(sql_type=Bool)]
    resource_available: bool,
    #[diesel(sql_type=Text)]
    requester_user_id: String,
    #[diesel(sql_type=Text)]
    payload: String,
}
#[derive(QueryableByName)]
struct Receipt {
    #[diesel(sql_type=Text)]
    operation: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    result: String,
}
#[derive(QueryableByName)]
struct Clock {
    #[diesel(sql_type=BigInt)]
    now: i64,
}
fn device(p: &Principal) -> Result<(&str, i64)> {
    match (p.device_id.as_deref(), p.device_generation) {
        (Some(id), Some(generation)) if generation > 0 => Ok((id, generation)),
        _ => Err(Error::Unauthorized("device_authorization_required")),
    }
}
async fn clock(db: &mut AsyncPgConnection) -> Result<i64> {
    Ok(
        sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now")
            .get_result::<Clock>(db)
            .await?
            .now,
    )
}
fn wire<T: Serialize>(value: &T) -> Result<String> {
    wire_with_limit(value, MAX_BYTES)
}
fn wire_with_limit<T: Serialize>(value: &T, max_bytes: usize) -> Result<String> {
    let data = encode(value)?;
    if data.len() > max_bytes {
        return Err(Error::Invalid("management_payload_too_large"));
    }
    Ok(data)
}
fn digest<T: Serialize>(target: &str, value: &T) -> Result<String> {
    Ok(hash(&wire(&(target, value))?))
}
async fn replay<T: DeserializeOwned>(
    db: &mut AsyncPgConnection,
    p: &Principal,
    command: &str,
    operation: &str,
    digest: &str,
) -> Result<Option<T>> {
    let (id, _) = device(p)?;
    key(command)?;
    // Authorization generations fence execution, while an idempotency key
    // survives fresh authorization of this same owner and installation.
    let rows = sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2")
        .bind::<Text,_>(&p.user_id).bind::<Text,_>(id).bind::<Text,_>(command).load::<Receipt>(db).await?;
    if rows.len() > 1 {
        return Err(Error::Conflict("management_command_conflict"));
    }
    if let Some(row) = rows.as_slice().first() {
        if row.operation != operation || row.digest != digest {
            return Err(Error::Conflict("management_command_conflict"));
        }
        return Ok(Some(decode(&row.result)?));
    }
    let full = sql_query("SELECT count(*)>=100000 AS matched FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1")
        .bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
    if full {
        return Err(Error::Conflict("management_receipt_capacity"));
    }
    Ok(None)
}
async fn receipt<T: Serialize>(
    db: &mut AsyncPgConnection,
    p: &Principal,
    command: &str,
    operation: &str,
    digest: &str,
    result: &T,
) -> Result<()> {
    let (id, generation) = device(p)?;
    sql_query("INSERT INTO hagency_agent_v1.management_commands(actor_user_id,device_id,device_generation,command_id,operation,digest,result) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind::<Text,_>(&p.user_id).bind::<Text,_>(id).bind::<BigInt,_>(generation).bind::<Text,_>(command).bind::<Text,_>(operation).bind::<Text,_>(digest).bind::<Text,_>(wire(result)?).execute(db).await?;
    Ok(())
}
async fn contribution(db: &mut AsyncPgConnection, id: &str) -> Result<(String, Contribution)> {
    let row = sql_query(format!("{CONTRIBUTIONS} WHERE c.id=$1"))
        .bind::<Text, _>(id)
        .get_result::<ContributionRow>(db)
        .await
        .map_err(hidden)?;
    Ok((row.owner_user_id, decode(&row.payload)?))
}
fn visible(p: &Principal, policy: &ManagementPolicy, owner: &str, c: &Contribution) -> bool {
    p.user_id == owner || p.mxid == c.coordinator_mxid || p.mxid == policy.administrator()
}
fn project_request(
    mut r: ManagementRequest,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> ManagementRequest {
    if r.state == "pending" && r.expires_at_ms <= now {
        r.state = "expired".into();
    }
    r.can_decide =
        r.state == "pending" && r.approver_mxid == p.mxid && p.mxid == policy.administrator();
    r
}
async fn request(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ManagementRequest> {
    let row = sql_query(format!("{REQUESTS} WHERE r.id=$1"))
        .bind::<Text, _>(id)
        .get_result::<RequestRow>(db)
        .await
        .map_err(hidden)?;
    let r: ManagementRequest = decode(&row.payload)?;
    let (owner, c) = contribution(db, &r.contribution_id).await?;
    if !visible(p, policy, &owner, &c)
        && row.requester_user_id != p.user_id
        && !setup::is_administrator(db, r.project_id.as_deref(), p).await?
    {
        return Err(Error::NotFound("management_request_not_found"));
    }
    let mut r = project_request(r, p, policy, now);
    r.can_decide &= row.resource_available;
    if let Some(id) = &r.allocation_id {
        r.can_decide =
            r.state == "pending" && allocations::actionable(db, id, p, policy, now).await?;
    }
    Ok(r)
}
async fn admission(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<Admission> {
    let (owner, c) = contribution(db, id).await?;
    if !visible(p, policy, &owner, &c) {
        return Err(Error::NotFound("contribution_not_found"));
    }
    let r = request(db, &c.request_id, p, policy, now).await?;
    Ok(Admission {
        contribution: c,
        request: r,
    })
}
async fn notify(
    db: &mut AsyncPgConnection,
    r: &ManagementRequest,
    recipients: impl IntoIterator<Item = String>,
    now: i64,
) -> Result<()> {
    for recipient in recipients.into_iter().collect::<BTreeSet<_>>() {
        let id = format!("mno_{}", entity_id()?);
        sql_query("INSERT INTO hagency_agent_v1.management_outbox(id,request_id,request_revision,recipient_mxid,transaction_id,created_at_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(request_id,request_revision,recipient_mxid) DO NOTHING")
            .bind::<Text,_>(&id).bind::<Text,_>(&r.id).bind::<BigInt,_>(r.revision).bind::<Text,_>(recipient).bind::<Text,_>(format!("hagency-{id}")).bind::<BigInt,_>(now).execute(db).await?;
    }
    Ok(())
}
async fn executor_live(
    db: &mut AsyncPgConnection,
    owner: &str,
    c: &Contribution,
    now: i64,
) -> Result<()> {
    let valid=sql_query("SELECT true AS matched FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id AND s.user_id=d.user_id JOIN hagency_agent_v1.users u ON u.id=d.user_id WHERE d.id=$1 AND d.user_id=$2 AND d.generation=$3 AND NOT d.revoked AND NOT s.revoked AND u.active AND s.valid_until_ms>greatest($4,(extract(epoch from clock_timestamp())*1000)::bigint) FOR SHARE OF d,s,u")
        .bind::<Text,_>(&c.executor_device_id).bind::<Text,_>(owner).bind::<BigInt,_>(c.executor_generation).bind::<BigInt,_>(now).get_result::<Flag>(db).await.optional()?;
    if valid.is_none() {
        return Err(Error::Conflict("contribution_executor_changed"));
    }
    // A session might have expired while waiting for the above row locks.
    let valid=sql_query("SELECT s.valid_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint AS matched FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE d.id=$1")
        .bind::<Text,_>(&c.executor_device_id).get_result::<Flag>(db).await?.matched;
    if !valid {
        return Err(Error::Conflict("contribution_executor_changed"));
    }
    Ok(())
}
impl DomainStore {
    /// Management deadlines are minted by PostgreSQL. Use that same clock for
    /// the response observation so finite native leases do not gain or lose
    /// time through process/DB clock skew or different millisecond rounding.
    pub(crate) async fn management_observed_at_ms(&self) -> Result<i64> {
        let mut db = self.db.lock().await?;
        clock(&mut db).await
    }

    /// Read an earlier result from this same owner/installation under fresh
    /// authorization. Credential rotation does not erase command identity and
    /// this endpoint never renews a verification or grants execution authority.
    pub async fn recover_management_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        command: &str,
        input: RecoverCommandInput,
    ) -> Result<Option<Admission>> {
        key(command)?;
        if !matches!(
            input.operation.as_str(),
            "contribution.create"
                | "request.decide"
                | "contribution.revoke"
                | "contribution.verify"
                | "contribution.resume"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let (executor, _) = device(p)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(command).load::<Receipt>(db).await?;
            if rows.len()>1 {return Err(Error::Conflict("management_command_conflict"));}
            let Some(row)=rows.as_slice().first() else {return Ok(None)};
            if row.operation!=input.operation {return Err(Error::Conflict("management_command_conflict"));}
            let saved: Admission=decode(&row.result)?;
            let now=clock(db).await?;
            Ok(Some(admission(db,&saved.contribution.id,p,policy,now).await?))
        }).await
    }
    /// Explicitly reattach the same installation after fresh authorization and
    /// a new native provider/resource check. Admission remains the administrator's
    /// decision; verification and execution grants are never renewed here.
    pub async fn resume_contribution(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: VerifyConnectionInput,
    ) -> Result<Admission> {
        key(id)?;
        key(&input.command_id)?;
        key(&input.resource_id)?;
        let (executor, generation) = device(p)?;
        if input.expected_revision <= 0 || input.resource_revision.len() != 64 {
            return Err(Error::Invalid("invalid_connection_verification"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let (owner,c)=contribution(db,id).await?;
            if owner!=p.user_id || executor!=c.executor_device_id {return Err(Error::Unauthorized("contribution_executor_required"));}
            if let Some(r)=replay(db,p,&input.command_id,"contribution.resume",&payload_digest).await? {return Ok(r);}
            if c.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"));}
            if !matches!(c.state.as_str(),"pending_admission"|"admitted") {return Err(Error::Conflict("management_action_closed"));}
            if c.state=="pending_admission" && request(db,&c.request_id,p,policy,now).await?.state!="pending" {return Err(Error::Conflict("management_action_closed"));}
            if c.resource_id!=input.resource_id || c.resource_revision!=input.resource_revision {return Err(Error::Conflict("contribution_resource_changed"));}
            if generation<=c.executor_generation {return Err(Error::Conflict("contribution_executor_current"));}
            sql_query("UPDATE hagency_agent_v1.management_contributions SET executor_generation=$2,revision=revision+1,updated_at_ms=$3 WHERE id=$1")
                .bind::<Text,_>(id).bind::<BigInt,_>(generation).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_requests SET revision=revision+1 WHERE id=$1")
                .bind::<Text,_>(&c.request_id).execute(db).await?;
            let result=admission(db,id,p,policy,now).await?;
            notify(db,&result.request,[policy.administrator().to_owned(),c.coordinator_mxid],now).await?;
            Self::audit(db,p,"contribution.resume",id,now).await?;
            receipt(db,p,&input.command_id,"contribution.resume",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }
    /// Only the originally enrolled executor may confirm the association after
    /// checking its local resource/provider. This creates no execution grant.
    pub async fn verify_contribution(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: VerifyConnectionInput,
    ) -> Result<Admission> {
        key(id)?;
        key(&input.command_id)?;
        key(&input.resource_id)?;
        let (executor, generation) = device(p)?;
        if input.expected_revision <= 0
            || input.resource_revision.len() != 64
            || !input
                .resource_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("invalid_connection_verification"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let (owner,c)=contribution(db,id).await?;
            if p.user_id!=owner || executor!=c.executor_device_id || generation!=c.executor_generation {
                return Err(Error::Unauthorized("contribution_executor_required"));
            }
            if let Some(r)=replay(db,p,&input.command_id,"contribution.verify",&payload_digest).await? { return Ok(r); }
            if c.revision!=input.expected_revision { return Err(Error::Conflict("management_revision_changed")); }
            if c.state!="admitted" { return Err(Error::Conflict("contribution_not_admitted")); }
            if c.resource_id!=input.resource_id || c.resource_revision!=input.resource_revision {
                return Err(Error::Conflict("contribution_resource_changed"));
            }
            executor_live(db,&owner,&c,now).await?;
            sql_query("UPDATE hagency_agent_v1.management_contributions SET revision=revision+1,updated_at_ms=$2 WHERE id=$1")
                .bind::<Text,_>(id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_verifications(contribution_id,contribution_revision,executor_generation,resource_revision,verified_at_ms,fresh_until_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(contribution_id) DO UPDATE SET contribution_revision=excluded.contribution_revision,executor_generation=excluded.executor_generation,resource_revision=excluded.resource_revision,verified_at_ms=excluded.verified_at_ms,fresh_until_ms=excluded.fresh_until_ms")
                .bind::<Text,_>(id).bind::<BigInt,_>(c.revision+1).bind::<BigInt,_>(generation).bind::<Text,_>(&input.resource_revision).bind::<BigInt,_>(now).bind::<BigInt,_>(now+5*60*1000).execute(db).await?;
            let result=admission(db,id,p,policy,now).await?;
            Self::audit(db,p,"contribution.verify",id,now).await?;
            receipt(db,p,&input.command_id,"contribution.verify",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }
    pub async fn create_contribution(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ContributionInput,
    ) -> Result<Admission> {
        input.validate(&self.server)?;
        let (executor, generation) = device(p)?;
        let payload_digest = digest("", &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            if let Some(r)=replay(db,p,&input.command_id,"contribution.create",&payload_digest).await? { return Ok(r); }
            if input.coordinator_mxid!=p.mxid{return Err(Error::Conflict("resource_owner_is_coordinator"));}
            let full=sql_query("SELECT count(*)>=1000 AS matched FROM hagency_agent_v1.management_contributions WHERE owner_user_id=$1").bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
            if full { return Err(Error::Conflict("contribution_capacity")); }
            let duplicate=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_contributions WHERE owner_user_id=$1 AND executor_device_id=$2 AND resource_id=$3) AS matched")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(&input.resource_id).get_result::<Flag>(db).await?.matched;
            if duplicate { return Err(Error::Conflict("contribution_already_exists")); }
            let id=format!("con_{}",entity_id()?); let request_id=format!("req_{}",entity_id()?);
            let runtime=match input.runtime { Runtime::Codex=>"codex",Runtime::Claude=>"claude",Runtime::Octos=>"octos" };
            sql_query("INSERT INTO hagency_agent_v1.management_contributions(id,owner_user_id,executor_device_id,executor_generation,resource_id,resource_revision,label,runtime,capabilities,coordinator_mxid,state,request_id,created_at_ms,updated_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'pending_admission',$11,$12,$12)")
                .bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(generation).bind::<Text,_>(&input.resource_id).bind::<Text,_>(&input.resource_revision).bind::<Text,_>(&input.label).bind::<Text,_>(runtime).bind::<Text,_>(encode(&input.capabilities)?).bind::<Text,_>(&input.coordinator_mxid).bind::<Text,_>(&request_id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,created_at_ms,expires_at_ms) VALUES($1,'resource_admission',$2,$3,$4,'pending',$5,$6)")
                .bind::<Text,_>(&request_id).bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(policy.administrator()).bind::<BigInt,_>(now).bind::<BigInt,_>(now+TTL).execute(db).await?;
            let result=admission(db,&id,p,policy,now).await?;
            notify(db,&result.request,[policy.administrator().to_owned()],now).await?;
            Self::audit(db,p,"contribution.create",&id,now).await?;
            receipt(db,p,&input.command_id,"contribution.create",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }
    pub async fn contributions(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        query: ManagementQuery,
    ) -> Result<ManagementPage<Contribution>> {
        query.validate()?;
        if query.needs_my_action {
            return Err(Error::Invalid("invalid_contribution_filter"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let rows=sql_query(format!("{CONTRIBUTIONS} WHERE (c.owner_user_id=$1 OR c.coordinator_mxid=$2 OR $2=$3) AND c.id>$4 ORDER BY c.id LIMIT $5"))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).bind::<Text,_>(query.after.as_deref().unwrap_or("")).bind::<BigInt,_>(MAX_PAGE+1).load::<ContributionRow>(db).await?;
            let more=rows.len()>MAX_PAGE as usize;
            let items=rows.into_iter().take(MAX_PAGE as usize).map(|row|decode(&row.payload)).collect::<Result<Vec<Contribution>>>()?;
            let next_after=if more {items.last().map(|v|v.id.clone())}else{None};
            Ok(ManagementPage{protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }
    pub async fn contribution(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<Admission> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                admission(db, id, p, policy, now).await
            })
            .await
    }
    pub async fn management_request(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ManagementRequest> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                request(db, id, p, policy, now).await
            })
            .await
    }
    pub async fn management_inbox(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        query: ManagementQuery,
    ) -> Result<ManagementPage<ManagementRequest>> {
        query.validate()?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let rows=sql_query(format!("{REQUESTS} JOIN hagency_agent_v1.management_contributions c ON c.id=r.contribution_id WHERE (r.requester_user_id=$1 OR c.owner_user_id=$1 OR c.coordinator_mxid=$2 OR r.approver_mxid=$2 AND ($2=$3 OR $9 AND r.kind='project_allocation') OR $9 AND r.kind='project_allocation' AND $2=$3 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_authorities pa WHERE pa.plan_id=r.project_id AND pa.administrator_user_id=$1)) AND (NOT $4 OR r.approver_mxid=$2 AND ($2=$3 OR $9 AND r.kind='project_allocation') AND r.state='pending' AND r.expires_at_ms>$5 AND (r.kind='resource_admission' OR c.state='admitted')) AND (r.kind='resource_admission' OR $8 AND r.kind='project_capacity' OR $9 AND r.kind='project_allocation') AND r.id>$6 ORDER BY r.id LIMIT $7"))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).bind::<Bool,_>(query.needs_my_action).bind::<BigInt,_>(now).bind::<Text,_>(query.after.as_deref().unwrap_or("")).bind::<BigInt,_>(MAX_PAGE+1).bind::<Bool,_>(query.include_projects).bind::<Bool,_>(query.include_allocations && policy.project_reservations()).load::<RequestRow>(db).await?;
            let more=rows.len()>MAX_PAGE as usize;
            let mut items=Vec::new();let mut last_scanned=None;
            for row in rows.into_iter().take(MAX_PAGE as usize) {
                let mut r=project_request(decode(&row.payload)?,p,policy,now);
                r.can_decide &= row.resource_available;
                if let Some(id)=&r.allocation_id {r.can_decide=r.state=="pending" && allocations::actionable(db,id,p,policy,now).await?;}
                last_scanned=Some(r.id.clone());
                if !query.needs_my_action || r.can_decide {items.push(r);}
            }
            let next_after=if more {last_scanned}else{None};
            Ok(ManagementPage{protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }
    pub async fn decide_management_request(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: DecisionInput,
    ) -> Result<Admission> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            if p.mxid!=policy.administrator() {return Err(Error::Unauthorized("management_administrator_required"));}
            let now=clock(db).await?;
            if let Some(r)=replay(db,p,&input.command_id,"request.decide",&payload_digest).await? {return Ok(r);}
            let r=request(db,id,p,policy,now).await?;
            if r.kind!="resource_admission" {return Err(Error::Invalid("management_operation_mismatch"));}
            if r.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"));}
            if !r.can_decide {return Err(Error::Conflict("management_action_closed"));}
            let (owner,c)=contribution(db,&r.contribution_id).await?;
            if c.state!="pending_admission" {return Err(Error::Conflict("management_action_closed"));}
            if matches!(input.decision,Decision::Approve) {executor_live(db,&owner,&c,now).await?;}
            let (state,contribution_state)=match input.decision {Decision::Approve=>("approved","admitted"),Decision::Reject=>("rejected","rejected")};
            sql_query("UPDATE hagency_agent_v1.management_requests SET state=$2,revision=revision+1,decided_at_ms=$3 WHERE id=$1")
                .bind::<Text,_>(id).bind::<Text,_>(state).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_contributions SET state=$2,revision=revision+1,updated_at_ms=$3 WHERE id=$1")
                .bind::<Text,_>(&c.id).bind::<Text,_>(contribution_state).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'").bind::<Text,_>(id).execute(db).await?;
            let result=admission(db,&c.id,p,policy,now).await?;
            notify(db,&result.request,[c.owner_mxid,c.coordinator_mxid],now).await?;
            Self::audit(db,p,"request.decide",id,now).await?;
            receipt(db,p,&input.command_id,"request.decide",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }
    pub async fn revoke_contribution(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<Admission> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let (owner,c)=contribution(db,id).await?;
            if p.user_id!=owner && p.mxid!=policy.administrator() {return Err(Error::Unauthorized("contribution_owner_required"));}
            if let Some(r)=replay(db,p,&input.command_id,"contribution.revoke",&payload_digest).await? {return Ok(r);}
            if c.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"));}
            if c.state=="revoked" {return Err(Error::Conflict("management_action_closed"));}
            sql_query("UPDATE hagency_agent_v1.management_contributions SET state='revoked',revision=revision+1,updated_at_ms=$2 WHERE id=$1")
                .bind::<Text,_>(id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_requests SET state='revoked',revision=revision+1,decided_at_ms=$2 WHERE id=$1")
                .bind::<Text,_>(&c.request_id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'").bind::<Text,_>(&c.request_id).execute(db).await?;
            allocations::revoke_for_contribution(db,p,policy,id,now).await?;
            parents::close_for_contribution(db,p,policy,id,now).await?;
            projects::revoke_contribution_projects(db,p,policy,id,now).await?;
            let result=admission(db,id,p,policy,now).await?;
            notify(db,&result.request,[c.owner_mxid,c.coordinator_mxid,policy.administrator().to_owned()],now).await?;
            Self::audit(db,p,"contribution.revoke",id,now).await?;
            receipt(db,p,&input.command_id,"contribution.revoke",&payload_digest,&result).await?;
            Ok(result)
        }).await
    }
}
