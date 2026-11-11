use super::*;
async fn fresh(
    db: &mut AsyncPgConnection,
    p: &Principal,
    d: &Demand,
    q: &Queued,
    input: &SharedDemandStartInput,
    server: &str,
    now: i64,
) -> Result<(Runtime, SharedRuntimeCommand)> {
    let o = offer(d, q)?;
    input.validate_for(&o)?;
    let original = authorize(db, p, q, server, now, true).await?;
    if has_applied_budget(db, &original.request_id).await? {
        return Err(Error::Conflict("shared_budget_execution_required"));
    }
    if q.state != "pending"
        || q.expires_at_ms <= now
        || oldest(db, &q.assignment_id, q.control_revision)
            .await?
            .is_none_or(|v| v.id != q.id)
        || binding(db, &d.id).await?.is_some()
    {
        return Err(Error::Conflict("shared_demand_not_current"));
    }
    if status(db, d).await?.state != "offered" {
        return Err(Error::Conflict("shared_demand_blocked"));
    }
    if attempts::prior_start(db, &input.start, &allocation_digest(&input.start)?, p)
        .await?
        .is_some()
    {
        return Err(Error::Conflict("shared_demand_start_conflict"));
    }
    attempts::ready(db, p, &input.start, server, now).await
}
async fn saved(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    d: &Demand,
    input: &SharedDemandStartInput,
    server: &str,
) -> Result<Option<SharedAttemptDelivery>> {
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
        attempts::attempt_delivery(db, p, policy, &r, server, true, None).await?,
    ))
}
impl DomainStore {
    pub async fn start_shared_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedDemandStartInput,
        gateway: &Gateway,
    ) -> Result<SharedAttemptDelivery> {
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
                enabled_demand(policy)?;
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
        (*g).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,0).await?;let d=load(db,id).await?;let q=queued(db,&d.intake_id).await?;if let Some(v)=saved(db,p,policy,&d,&input,&self.server).await?{return Ok(v)}enabled_demand(policy)?;let now=clock(db).await?;let prepared=fresh(db,p,&d,&q,&input,&self.server,now).await?;facts(&membership(db,&prepared.0).await?,&observed)?;
 let delivery=attempts::commit_start(db,p,policy,&input.start,prepared,&self.server,now).await?;
 sql_query("INSERT INTO hagency_agent_v1.management_shared_demand_starts VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(id).bind::<Text,_>(&input.start.start_intent_id).bind::<Text,_>(&delivery.command.command_id).bind::<Text,_>(wire(&input)?).bind::<Text,_>(allocation_digest(&input)?).execute(db).await?;Ok(delivery)}).await
    }
    pub async fn block_shared_demand(
        &self,
        p: &Principal,
        id: &str,
        input: SharedDemandBlockInput,
    ) -> Result<SharedDemandStatus> {
        key(id)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, 0).await?;
            let d = load(db, id).await?;
            let q = queued(db, &d.intake_id).await?;
            let o = offer(&d, &q)?;
            let now = clock(db).await?;
            let w = authorize(db, p, &q, &self.server, now, true).await?;
            if d.digest != input.demand_digest
                || o.control_revision != input.expected_control_revision
                || started(db, id).await?.is_some()
                || binding(db, id).await?.is_some()
            {
                return Err(Error::Conflict("shared_demand_changed"));
            }
            if q.state != "pending" && status(db, &d).await?.state != input.reason {
                return Err(Error::Conflict("shared_demand_not_current"));
            }
            let c: ManagedAgentReserveCommand = decode(&w.payload)?;
            match (input.reason.as_str(), &input.budget) {
                ("budget_blocked", Some(b)) => {
                    b.validate()?;
                    if b.allocated_tokens != c.requested_tokens
                        || b.remaining_tokens != 0
                        || b.known_consumed_tokens < known(db, &q.assignment_id).await?
                    {
                        return Err(Error::Conflict("shared_budget_observation_changed"));
                    }
                }
                ("usage_unrepresentable", None) => (),
                _ => return Err(Error::Invalid("invalid_shared_demand_block")),
            }
            block(db, &d, &q, &input.reason, &wire(&input)?).await?;
            status(db, &d).await
        })
        .await
    }
}
