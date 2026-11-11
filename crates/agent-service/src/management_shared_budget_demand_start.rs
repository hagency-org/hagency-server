use super::*;
async fn fresh(
    db: &mut AsyncPgConnection,
    p: &Principal,
    d: &Demand,
    q: &Queued,
    input: &SharedBudgetDemandStartInput,
    server: &str,
    now: i64,
) -> Result<(Runtime, SharedRuntimeCommand)> {
    let o = offer(d, q)?;
    input.validate_for(&o)?;
    authorize(db, p, q, server, now, true).await?;
    if !matches!(q.state.as_str(), "pending" | "blocked")
        || q.expires_at_ms <= now
        || oldest(db, &q.assignment_id, q.control_revision)
            .await?
            .is_none_or(|v| v.id != q.id)
        || binding(db, &d.id).await?.is_some()
        || super::super::binding(db, &d.id).await?.is_some()
        || super::super::started(db, &d.id).await?.is_some()
        || input.original_block != original_block(db, &d.id).await?
        || input.start.execution_budget != latest_pin(db, &d.id).await?
    {
        return Err(Error::Conflict("shared_demand_not_current"));
    }
    if status(db, d).await?.state != "offered" {
        return Err(Error::Conflict("shared_demand_blocked"));
    }
    if budget_attempts::prior_start(db, &input.start, &allocation_digest(&input.start)?, p)
        .await?
        .is_some()
    {
        return Err(Error::Conflict("shared_demand_start_conflict"));
    }
    budget_attempts::ready(db, p, &input.start, server, now).await
}
async fn saved(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    d: &Demand,
    input: &SharedBudgetDemandStartInput,
    server: &str,
) -> Result<Option<SharedBudgetAttemptDelivery>> {
    let q = queued(db, &d.intake_id).await?;
    let o = offer(d, &q)?;
    input.validate_for(&o)?;
    let w = intake::assignment(db, &q.assignment_id).await?;
    owner(&w, p)?;
    let Some(s) = started(db, &d.id).await? else {
        return Ok(None);
    };
    if s.digest != allocation_digest(input)? || s.payload != wire(input)? {
        return Err(Error::Conflict("shared_demand_start_changed"));
    }
    let r = runtime(db, &s.runtime_id).await?;
    Ok(Some(
        budget_attempts::attempt_delivery(db, p, policy, &r, server, true, None).await?,
    ))
}
impl DomainStore {
    pub async fn start_shared_budget_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedBudgetDemandStartInput,
        gateway: &Gateway,
    ) -> Result<SharedBudgetAttemptDelivery> {
        key(id)?;
        let scope = {
            let mut g = self.db.lock().await?;
            (*g).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let d = load(db, id).await?;
                let q = queued(db, &d.intake_id).await?;
                if let Some(v) = saved(db, p, policy, &d, &input, &self.server).await? {
                    return Ok(Err(v));
                }
                enabled_budget(policy)?;
                let now = clock(db).await?;
                let (r, _) = fresh(db, p, &d, &q, &input, &self.server, now).await?;
                Ok(Ok(membership(db, &r).await?))
            })
            .await?
        };
        let m = match scope {
            Err(v) => return Ok(v),
            Ok(m) => m,
        };
        let observed = gateway
            .shared_delivery(
                &m.room_id,
                &m.space_id,
                &m.manager_mxid,
                &m.manager_mxid,
                &m.puppet_mxid,
            )
            .await?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,0).await?;let d=load(db,id).await?;let q=queued(db,&d.intake_id).await?;if let Some(v)=saved(db,p,policy,&d,&input,&self.server).await?{return Ok(v)}enabled_budget(policy)?;let now=clock(db).await?;let prepared=fresh(db,p,&d,&q,&input,&self.server,now).await?;facts(&membership(db,&prepared.0).await?,&observed)?;
 let delivery=budget_attempts::commit_start(db,p,policy,&input.start,prepared,&self.server,now).await?;
 sql_query("INSERT INTO hagency_agent_v1.management_shared_budget_demand_starts(demand_id,start_intent_id,runtime_id,payload,digest,budget_revision) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(id).bind::<Text,_>(&input.start.start_intent_id).bind::<Text,_>(&delivery.command.command_id).bind::<Text,_>(wire(&input)?).bind::<Text,_>(allocation_digest(&input)?).bind::<BigInt,_>(input.start.execution_budget.budget_revision).execute(db).await?;Ok(delivery)}).await
    }
    pub async fn block_shared_budget_demand(
        &self,
        p: &Principal,
        id: &str,
        input: SharedBudgetDemandBlockInput,
    ) -> Result<SharedBudgetDemandStatus> {
        key(id)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, 0).await?;
            let d = load(db, id).await?;
            let q = queued(db, &d.intake_id).await?;
            let o = offer(&d, &q)?;
            let now = clock(db).await?;
            authorize(db,p,&q,&self.server,now,false).await?;
            let prior=sql_query("SELECT payload,digest FROM hagency_agent_v1.management_shared_budget_demand_blocks WHERE demand_id=$1 AND budget_revision=$2")
                .bind::<Text,_>(id).bind::<BigInt,_>(input.execution_budget.budget_revision).get_result::<Pin>(db).await.optional()?;
            if let Some(prior)=prior {
                if prior.payload!=wire(&input)? || prior.digest!=hash(&prior.payload) {return Err(Error::Conflict("shared_demand_block_changed"));}
                return status(db,&d).await;
            }
            let w = authorize(db, p, &q, &self.server, now, true).await?;
            if d.digest != input.demand_digest
                || o.control_revision != input.expected_control_revision
                || started(db, id).await?.is_some()
                || binding(db, id).await?.is_some()
            {
                return Err(Error::Conflict("shared_demand_changed"));
            }
            if !matches!(q.state.as_str(),"pending"|"blocked") {
                return Err(Error::Conflict("shared_demand_not_current"));
            }
            let (pin,_,known)=super::super::super::super::super::budget::execution_head(db,&w.request_id,&self.server).await?;
            if input.execution_budget!=pin || input.execution_budget!=latest_pin(db,id).await?
                || input.original_block!=original_block(db,id).await? {return Err(Error::Conflict("shared_budget_head_changed"));}
            match (input.reason.as_str(), &input.budget) {
                ("budget_blocked", Some(b)) => {
                    b.validate()?;
                    if b.allocated_tokens != pin.effective_ceiling_tokens
                        || b.remaining_tokens != 0
                        || b.known_consumed_tokens < known
                    {
                        return Err(Error::Conflict("shared_budget_observation_changed"));
                    }
                }
                ("usage_unrepresentable", None) => (),
                _ => return Err(Error::Invalid("invalid_shared_demand_block")),
            }
            block(db, &d, &pin, &input.reason, &wire(&input)?).await?;
            status(db, &d).await
        })
        .await
    }
}
