//! A pool owner approves an applicant-owned Agent directly against one grant.
//! Creation never inserts a project, project setup or project allocation.
use super::*;
use crate::management_allocation_types::ResourceGrantView;
use crate::management_pool_types::*;

fn enabled_pool(policy: &ManagementPolicy) -> Result<()> {
    if policy.agent_admission() && policy.project_reservations() {
        Ok(())
    } else {
        Err(Error::Unavailable("pool_agents_unavailable"))
    }
}

#[derive(QueryableByName)]
struct PoolPeople {
    #[diesel(sql_type=Text)]
    manager: String,
    #[diesel(sql_type=Text)]
    owner: String,
    #[diesel(sql_type=Bool)]
    active: bool,
}

async fn people(db: &mut AsyncPgConnection, r: &Row) -> Result<PoolPeople> {
    sql_query("SELECT m.mxid AS manager,o.mxid AS owner,(m.active AND o.active) AS active FROM hagency_agent_v1.users m CROSS JOIN hagency_agent_v1.users o WHERE m.id=$1 AND o.id=$2")
        .bind::<Text,_>(&r.manager_user_id).bind::<Text,_>(&r.administrator_user_id).get_result(db).await.map_err(hidden)
}

async fn pool_row(db: &mut AsyncPgConnection, id: &str, p: &Principal) -> Result<Row> {
    let r = row(db, id).await?;
    if r.pool_contribution_id.is_none()
        || (r.manager_user_id != p.user_id && r.administrator_user_id != p.user_id)
    {
        return Err(Error::NotFound("pool_agent_request_not_found"));
    }
    Ok(r)
}

pub(super) async fn current_grant(
    db: &mut AsyncPgConnection,
    r: &Row,
    now: i64,
) -> Result<ResourceGrantView> {
    let pool = r
        .pool_contribution_id
        .as_deref()
        .ok_or(Error::Conflict("pool_agent_required"))?;
    let grant = allocations::live_grant_for_pool(db, pool, now)
        .await?
        .ok_or(Error::Conflict("pool_capacity_unavailable"))?;
    let persons = people(db, r).await?;
    if !persons.active
        || r.pool_grant_id.as_deref() != Some(&grant.id)
        || grant.receipt.owner_mxid != persons.owner
        || !grant.receipt.eligible_managers.contains(&persons.manager)
        || grant.receipt.period_key != r.period_key
        || grant.receipt.grant_revision != r.authority_revision
    {
        return Err(Error::Conflict("pool_agent_authority_changed"));
    }
    Ok(grant)
}

pub(super) async fn command_live(
    db: &mut AsyncPgConnection,
    r: &Row,
    c: &ManagedAgentReserveCommand,
    now: i64,
) -> Result<bool> {
    let Some(original) = &c.pool_grant else {
        return Ok(false);
    };
    let current = match current_grant(db, r, now).await {
        Ok(g) => g,
        Err(Error::Conflict(_) | Error::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    let persons = people(db, r).await?;
    Ok(c.version == 2
        && c.request_id == r.id
        && c.proposed_agent_id == r.proposed_agent_id
        && c.room_id == r.room_id
        && c.definition_digest == r.definition_digest
        && c.requested_tokens == r.requested_tokens
        && c.manager_mxid == persons.manager
        && c.decision_actor_mxid == persons.owner
        && c.grant_receipt_digest == current.receipt_digest
        && allocation_digest(original)? == current.receipt_digest)
}

async fn view_pool(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    now: i64,
) -> Result<PoolAgentRequest> {
    let r = pool_row(db, id, p).await?;
    let persons = people(db, &r).await?;
    let pool_id = r.pool_contribution_id.as_deref().unwrap();
    let (_, contribution) = contribution(db, pool_id).await?;
    let mut blocked = match current_grant(db, &r, now).await {
        Ok(_) => None,
        Err(Error::Conflict(reason) | Error::NotFound(reason)) => Some(reason.to_owned()),
        Err(e) => return Err(e),
    };
    let expired = r.state == "pending_approval" && r.expires_at_ms <= now;
    if expired {
        blocked = Some("pool_agent_request_expired".into());
    }
    let open = matches!(
        r.state.as_str(),
        "pending_approval" | "approved_waiting_executor"
    );
    let progress=sql_query("SELECT json_build_object('assignment',a.id,'membership',m.state,'runtime',CASE WHEN x.permanent_fence THEN 'cleanup_pending' ELSE x.state END)::text AS payload FROM hagency_agent_v1.management_shared_assignments a LEFT JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id LEFT JOIN LATERAL (SELECT state,permanent_fence FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=a.id ORDER BY created_at_ms DESC,id DESC LIMIT 1) x ON true WHERE a.request_id=$1").bind::<Text,_>(&r.id).get_result::<AgentPerson>(db).await.optional()?.map(|r|decode::<Value>(&r.payload)).transpose()?.unwrap_or(Value::Null);
    let reservation=sql_query("SELECT w.receipt AS payload FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.reserve' AND w.receipt IS NOT NULL").bind::<Text,_>(&r.id).get_result::<AgentPerson>(db).await.optional()?.map(|r|decode::<Value>(&r.payload)).transpose()?.unwrap_or(Value::Null);
    Ok(PoolAgentRequest {
        id: r.id,
        pool_id: pool_id.into(),
        pool_label: contribution.label,
        resource_owner_mxid: persons.owner,
        manager_mxid: persons.manager,
        agent_id: r.proposed_agent_id,
        room_id: r.room_id,
        assignment_id: progress["assignment"].as_str().map(str::to_owned),
        membership_state: progress["membership"].as_str().map(str::to_owned),
        runtime_state: progress["runtime"].as_str().map(str::to_owned),
        reservation_state: reservation["state"].as_str().map(str::to_owned),
        reservation_reason: reservation["reason"].as_str().map(str::to_owned),
        definition: decode(&r.definition)?,
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
        can_decide: r.state == "pending_approval"
            && !expired
            && p.user_id == r.administrator_user_id,
        can_cancel: open && p.user_id == r.manager_user_id,
        can_stop_supply: open && p.user_id == r.administrator_user_id,
        blocking_reason: blocked,
    })
}

async fn notice_pool(db: &mut AsyncPgConnection, r: &PoolAgentRequest, now: i64) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_agent_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'")
        .bind::<Text,_>(&r.id).execute(db).await?;
    for recipient in BTreeSet::from([&r.manager_mxid, &r.resource_owner_mxid]) {
        let id = format!("mano_{}", entity_id()?);
        sql_query("INSERT INTO hagency_agent_v1.management_agent_outbox(id,request_id,request_revision,recipient_mxid,transaction_id,created_at_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(request_id,request_revision,recipient_mxid) DO NOTHING")
            .bind::<Text,_>(&id).bind::<Text,_>(&r.id).bind::<BigInt,_>(r.revision)
            .bind::<Text,_>(recipient).bind::<Text,_>(format!("hagency-{id}")).bind::<BigInt,_>(now).execute(db).await?;
    }
    Ok(())
}

impl DomainStore {
    pub async fn recover_pool_agent_command(
        &self,
        p: &Principal,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<Value> {
        key(id)?;
        let (device, _) = device(p)?;
        if !matches!(
            input.operation.as_str(),
            "pool_agent.request" | "pool_agent.decide" | "pool_agent.stop"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(id).load::<Receipt>(db).await?;
            if rows.len()>1 {return Err(Error::Conflict("management_command_conflict"));}
            let Some(r)=rows.as_slice().first() else {return Ok(json!({"recorded":false,"request":null}));};
            if r.operation!=input.operation {return Err(Error::Conflict("management_command_conflict"));}
            let saved:PoolAgentRequest=decode(&r.result)?;
            Ok(json!({"recorded":true,"request":view_pool(db,&saved.id,p,now).await?}))
        }).await
    }
    pub async fn resource_pools(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        query: CatalogQuery,
    ) -> Result<ManagementPage<ResourcePool>> {
        enabled_pool(policy)?;
        if let Some(after) = &query.after {
            key(after)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            let rows=sql_query(format!("{CONTRIBUTIONS} WHERE c.state='admitted' AND c.id>$1 AND ($2::text IS NULL OR c.capabilities::jsonb ? $2) ORDER BY c.id LIMIT 17"))
                .bind::<Text,_>(query.after.as_deref().unwrap_or(""))
                .bind::<Nullable<Text>,_>(query.capability.as_deref()).load::<ContributionRow>(db).await?;
            let more=rows.len()>16;
            let mut items=Vec::new();
            for row in rows.into_iter().take(16) {
                let c: Contribution=decode(&row.payload)?;
                let grant=allocations::live_grant_for_pool(db,&c.id,now).await?;
                let eligible=grant.as_ref().is_some_and(|g|g.receipt.eligible_managers.contains(&p.mxid));
                let visible=eligible || row.owner_user_id==p.user_id;
                items.push(ResourcePool {id:c.id,label:c.label,revision:c.revision,owner_mxid:c.owner_mxid,
                    runtime:c.runtime,capabilities:c.capabilities,
                    grant_revision:grant.as_ref().filter(|_|visible).map(|g|g.receipt.grant_revision),
                    published_tokens:grant.as_ref().filter(|_|visible).map(|g|g.receipt.allocated_tokens),
                    native_available_tokens:None,can_request:eligible,
                    blocking_reason:if eligible {None} else {Some(if grant.is_some() {"pool_not_shared_with_account"}else{"pool_capacity_unavailable"}.into())}});
            }
            let next_after=if more {items.last().map(|p|p.id.clone())}else{None};
            Ok(ManagementPage {protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }

    pub async fn request_pool_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        pool: &str,
        input: RequestPoolAgent,
    ) -> Result<PoolAgentRequest> {
        enabled_pool(policy)?;
        key(pool)?;
        input.validate()?;
        device(p)?;
        let payload = digest(pool, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?; let now=clock(db).await?;
            if let Some(saved)=replay::<PoolAgentRequest>(db,p,&input.command_id,"pool_agent.request",&payload).await? {
                return view_pool(db,&saved.id,p,now).await;
            }
            let (owner,c)=contribution(db,pool).await?;
            if c.state!="admitted" {return Err(Error::Conflict("pool_capacity_unavailable"));}
            if c.revision!=input.expected_pool_revision {return Err(Error::Conflict("management_revision_changed"));}
            let grant=allocations::live_grant_for_pool(db,pool,now).await?.ok_or(Error::Conflict("pool_capacity_unavailable"))?;
            let g=&grant.receipt;
            if !g.eligible_managers.contains(&p.mxid) {return Err(Error::Unauthorized("pool_not_shared_with_account"));}
            if g.grant_revision!=input.expected_grant_revision {return Err(Error::Conflict("management_revision_changed"));}
            if input.requested_tokens>g.allocated_tokens || !c.capabilities.contains(&input.definition.capability) {
                return Err(Error::Conflict("pool_agent_resource_mismatch"));
            }
            let full=sql_query("SELECT count(*)>=1000 AS matched FROM hagency_agent_v1.management_agent_requests WHERE manager_user_id=$1 AND pool_contribution_id IS NOT NULL AND state IN ('pending_approval','approved_waiting_executor')")
                .bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;
            if full {return Err(Error::Conflict("pool_agent_request_capacity"));}
            let id=format!("par_{}",entity_id()?); let proposed=format!("agt_{}",entity_id()?);
            let expires=(now+TTL).min(g.period_end_ms).min(g.delegation_expires_at_ms);
            sql_query("INSERT INTO hagency_agent_v1.management_agent_requests(id,setup_binding_revision,allocation_receipt_digest,manager_user_id,room_id,proposed_agent_id,definition,definition_digest,requested_tokens,period_key,administrator_user_id,authority_revision,state,created_at_ms,expires_at_ms,pool_contribution_id,pool_grant_id) VALUES($1,0,'',$2,$14,$3,$4,$5,$6,$7,$8,$9,'pending_approval',$10,$11,$12,$13)")
                .bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&proposed)
                .bind::<Text,_>(wire(&input.definition)?).bind::<Text,_>(allocation_digest(&input.definition)?)
                .bind::<BigInt,_>(input.requested_tokens).bind::<Text,_>(&g.period_key).bind::<Text,_>(&owner)
                .bind::<BigInt,_>(g.grant_revision).bind::<BigInt,_>(now).bind::<BigInt,_>(expires)
                .bind::<Text,_>(pool).bind::<Text,_>(&grant.id).bind::<Text,_>(&input.room_id).execute(db).await?;
            let result=view_pool(db,&id,p,now).await?; notice_pool(db,&result,now).await?;
            Self::audit(db,p,"pool_agent.request",&id,now).await?;
            receipt(db,p,&input.command_id,"pool_agent.request",&payload,&result).await?;
            Ok(result)
        }).await
    }

    pub async fn pool_agent_request(&self, p: &Principal, id: &str) -> Result<PoolAgentRequest> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                view_pool(db, id, p, now).await
            })
            .await
    }

    pub async fn pool_agent_requests(
        &self,
        p: &Principal,
        input: PoolAgentQuery,
    ) -> Result<ManagementPage<PoolAgentRequest>> {
        for id in [&input.after, &input.pool_id].into_iter().flatten() {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT * FROM hagency_agent_v1.management_agent_request_read WHERE pool_contribution_id IS NOT NULL AND (manager_user_id=$1 OR administrator_user_id=$1) AND (NOT $2 OR administrator_user_id=$1 AND state='pending_approval' AND expires_at_ms>$3) AND id>$4 AND ($5::text IS NULL OR pool_contribution_id=$5) ORDER BY id LIMIT 17")
                .bind::<Text,_>(&p.user_id).bind::<Bool,_>(input.needs_my_action).bind::<BigInt,_>(now)
                .bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<Nullable<Text>,_>(input.pool_id.as_deref()).load::<Row>(db).await?;
            let more=rows.len()>16;let mut items=Vec::new();
            for r in rows.into_iter().take(16) {items.push(view_pool(db,&r.id,p,now).await?);}
            let next_after=if more {items.last().map(|r|r.id.clone())}else{None};
            Ok(ManagementPage {protocol:PROTOCOL,observed_at_ms:now,items,next_after})
        }).await
    }

    pub async fn decide_pool_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: DecidePoolAgent,
    ) -> Result<PoolAgentRequest> {
        enabled_pool(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision < 1 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            sql_query("SELECT id AS payload FROM hagency_agent_v1.management_agent_requests WHERE id=$1 FOR UPDATE").bind::<Text,_>(id).get_result::<AgentPerson>(db).await.map_err(hidden)?;
            let r=pool_row(db,id,p).await?;
            if r.administrator_user_id!=p.user_id {return Err(Error::Unauthorized("pool_resource_owner_required"));}
            if let Some(saved)=replay::<PoolAgentRequest>(db,p,&input.command_id,"pool_agent.decide",&payload).await? {
                return view_pool(db,&saved.id,p,now).await;
            }
            if r.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"));}
            if r.state!="pending_approval" || r.expires_at_ms<=now {return Err(Error::Conflict("pool_agent_request_closed"));}
            let approve=matches!(input.decision,Decision::Approve);
            let verdict=if approve {"approve"}else{"reject"};
            let verdict_digest=allocation_digest(&json!({"version":1,"requestId":r.id,"requestRevision":r.revision,"resourceOwnerMxid":p.mxid,"commandId":input.command_id,"decision":verdict}))?;
            if approve {
                let grant=current_grant(db,&r,now).await?;let persons=people(db,&r).await?;let g=grant.receipt;
                let command=ManagedAgentReserveCommand {version:2,pool_grant:Some(g.clone()),capability:"hagency.pool_agents.v1".into(),operation:"agent.reserve".into(),command_id:format!("mae_{}",entity_id()?),request_id:r.id.clone(),request_revision:r.revision+1,proposed_agent_id:r.proposed_agent_id.clone(),manager_mxid:persons.manager,project_id:String::new(),project_revision:0,setup_id:String::new(),setup_binding_revision:0,domain_project_id:String::new(),room_id:r.room_id.clone(),authority_revision:g.grant_revision,decision_actor_mxid:p.mxid.clone(),decision_digest:verdict_digest.clone(),allocation_id:r.id.clone(),allocation_receipt_digest:String::new(),grant_receipt_digest:grant.receipt_digest,enrollment_id:g.enrollment_id.clone(),executor_device_id:g.executor_device_id.clone(),executor_generation:g.executor_generation,resource_id:g.resource_id.clone(),resource_revision:g.resource_revision.clone(),definition:decode(&r.definition)?,definition_digest:r.definition_digest.clone(),requested_tokens:r.requested_tokens,period_key:r.period_key.clone(),issued_at_ms:now,expires_at_ms:r.expires_at_ms};
                command.validate(&self.server)?;
                sql_query("INSERT INTO hagency_agent_v1.management_agent_intents(id,request_id,executor_owner_user_id,executor_device_id,executor_generation,payload,digest) VALUES($1,$2,$3,$4,$5,$6,$7)")
                    .bind::<Text,_>(&command.command_id).bind::<Text,_>(&r.id).bind::<Text,_>(&p.user_id)
                    .bind::<Text,_>(&g.executor_device_id).bind::<BigInt,_>(g.executor_generation)
                    .bind::<Text,_>(wire(&command)?).bind::<Text,_>(allocation_digest(&command)?).execute(db).await?;
            }
            sql_query("INSERT INTO hagency_agent_v1.management_agent_decisions(request_id,request_revision,administrator_user_id,authority_revision,command_id,digest,decision,decided_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind::<Text,_>(&r.id).bind::<BigInt,_>(r.revision).bind::<Text,_>(&p.user_id)
                .bind::<BigInt,_>(r.authority_revision).bind::<Text,_>(&input.command_id).bind::<Text,_>(&verdict_digest)
                .bind::<Text,_>(verdict).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state=$2,revision=revision+1 WHERE id=$1")
                .bind::<Text,_>(&r.id).bind::<Text,_>(if approve {"approved_waiting_executor"}else{"rejected"}).execute(db).await?;
            let result=view_pool(db,&r.id,p,now).await?;notice_pool(db,&result,now).await?;
            Self::audit(db,p,"pool_agent.decide",&r.id,now).await?;
            receipt(db,p,&input.command_id,"pool_agent.decide",&payload,&result).await?;Ok(result)
        }).await
    }

    pub async fn stop_pool_agent(
        &self,
        p: &Principal,
        id: &str,
        input: RevokeInput,
    ) -> Result<PoolAgentRequest> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        let payload = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            sql_query("SELECT id AS payload FROM hagency_agent_v1.management_agent_requests WHERE id=$1 FOR UPDATE").bind::<Text,_>(id).get_result::<AgentPerson>(db).await.map_err(hidden)?;
            let r=pool_row(db,id,p).await?;
            if let Some(saved)=replay::<PoolAgentRequest>(db,p,&input.command_id,"pool_agent.stop",&payload).await? {
                return view_pool(db,&saved.id,p,now).await;
            }
            if r.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"));}
            if !matches!(r.state.as_str(),"pending_approval"|"approved_waiting_executor") {return Err(Error::Conflict("pool_agent_request_closed"));}
            sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state=$2,revision=revision+1 WHERE id=$1")
                .bind::<Text,_>(id).bind::<Text,_>(if p.user_id==r.manager_user_id {"cancelled"}else{"revoked"}).execute(db).await?;
            let result=view_pool(db,id,p,now).await?;notice_pool(db,&result,now).await?;
            Self::audit(db,p,"pool_agent.stop",id,now).await?;
            receipt(db,p,&input.command_id,"pool_agent.stop",&payload,&result).await?;Ok(result)
        }).await
    }
}
