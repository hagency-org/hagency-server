//! Recurring approvals are durable executor commands. Central state describes
//! native authorization; it never fabricates a reservation or resets usage.
use super::*;
use crate::{
    management_agent_executor_types::*,
    management_pool_types::*,
    recurring_quota::{Calendar, Period, RenewalMode},
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Terms {
    pub calendar: Calendar,
    pub renewal_mode: RenewalMode,
}
pub(super) async fn terms(db: &mut AsyncPgConnection, request: &str) -> Result<Option<Terms>> {
    sql_query("SELECT json_build_object('calendar',calendar::jsonb,'renewalMode',renewal_mode)::text AS payload FROM hagency_agent_v1.management_agent_recurring_terms WHERE request_id=$1")
        .bind::<Text,_>(request).get_result::<AgentPerson>(db).await.optional()?.map(|r|decode(&r.payload)).transpose()
}
fn period(t: &Terms, now: i64) -> Result<Period> {
    t.calendar
        .period_at(now as u64)
        .map_err(|_| Error::Invalid("invalid_agent_period"))
}
pub(super) async fn authorized(
    db: &mut AsyncPgConnection,
    c: &ManagedAgentReserveCommand,
    now: i64,
) -> Result<bool> {
    let Some(g) = c.pool_grant.as_ref().filter(|g| g.calendar.is_some()) else {
        return Ok(true);
    };
    let t = terms(db, &c.request_id)
        .await?
        .ok_or(Error::Conflict("agent_period_terms_missing"))?;
    let period = period(&t, now)?;
    if period.key == g.period_key {
        return Ok(now < c.expires_at_ms);
    }
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands WHERE intent_id=$1 AND operation='agent.period.renew' AND period_key=$2 AND receipt::jsonb->>'state'='authorized' AND (command_payload::jsonb->>'expiresAtMs')::bigint>$3) AS matched")
        .bind::<Text,_>(&c.command_id).bind::<Text,_>(&period.key).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
/// Work is pinned at issuance, so late observations of an older work item never
/// consume a newer period. Unknown native holds remain enforced by the executor.
pub(super) async fn known(
    db: &mut AsyncPgConnection,
    request: &str,
    period: &Period,
) -> Result<i64> {
    Ok(sql_query("SELECT LEAST(COALESCE(SUM(w.known_consumed_tokens),0),9007199254740991)::bigint AS now FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes x ON x.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id WHERE a.request_id=$1 AND w.created_at_ms>=$2 AND w.created_at_ms<$3")
        .bind::<Text,_>(request).bind::<BigInt,_>(period.start_ms as i64).bind::<BigInt,_>(period.end_ms as i64).get_result::<Clock>(db).await?.now)
}
pub(super) async fn current_known(
    db: &mut AsyncPgConnection,
    request: &str,
    now: i64,
) -> Result<Option<i64>> {
    let Some(t) = terms(db, request).await? else {
        return Ok(None);
    };
    Ok(Some(known(db, request, &period(&t, now)?).await?))
}
pub(super) async fn projection(
    db: &mut AsyncPgConnection,
    r: &Row,
    p: &Principal,
    now: i64,
) -> Result<(
    Option<Calendar>,
    Option<RenewalMode>,
    Option<PoolAgentPeriod>,
    bool,
)> {
    let Some(t) = terms(db, &r.id).await? else {
        return Ok((None, None, None, false));
    };
    let period = period(&t, now)?;
    let original=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1 AND state<>'fenced'").bind::<Text,_>(&r.id).get_result::<AgentPerson>(db).await.optional()?.map(|r|decode::<ManagedAgentReserveCommand>(&r.payload)).transpose()?;
    let has_reservation = original.is_some() && pool::reserved(db, &r.id).await?;
    let live = match &original {
        Some(c) => has_reservation && authorized(db, c, now).await?,
        None => false,
    };
    let pending=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.period.renew' AND w.period_key=$2 AND w.receipt IS NULL) AS matched").bind::<Text,_>(&r.id).bind::<Text,_>(&period.key).get_result::<Flag>(db).await?.matched;
    let attempt = latest_attempt(db, original.as_ref().map(|c| c.command_id.as_str()), &period.key).await?;
    let refused = attempt.as_ref().is_some_and(|a| a.state.as_deref() == Some("refused"));
    let already_authorized = attempt.as_ref().is_some_and(|a| a.state.as_deref() == Some("authorized"));
    let (has_capacity, limit_exceeded) = match pool::effective_grant(db, r, now).await {
        Ok(g) => (g.receipt.request_limit() >= r.requested_tokens
            && g.receipt.calendar.as_ref() == Some(&t.calendar)
            && g.receipt.period_key == period.key,
            g.receipt.request_limit() < r.requested_tokens),
        Err(Error::Conflict(_) | Error::NotFound(_)) => (false, false),
        Err(e) => return Err(e),
    };
    let consumed = known(db, &r.id, &period).await?;
    let unsettled=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes x ON x.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id WHERE a.request_id=$1 AND w.created_at_ms>=$2 AND w.created_at_ms<$3 AND NOT w.usage_complete AND w.state<>'cancelled') AS matched").bind::<Text,_>(&r.id).bind::<BigInt,_>(period.start_ms as i64).bind::<BigInt,_>(period.end_ms as i64).get_result::<Flag>(db).await?.matched;
    let new_period = original
        .as_ref()
        .and_then(|c| c.pool_grant.as_ref())
        .is_some_and(|g| period.start_ms >= g.period_end_ms as u64);
    let open = r.state == "approved_waiting_executor" && original.is_some();
    let state = if r.state == "rejected" {
        "rejected"
    } else if r.state == "pending_approval" && r.expires_at_ms <= now {
        "expired"
    } else if !open && r.state != "pending_approval" {
        "revoked"
    } else if live && consumed >= r.requested_tokens {
        "exhausted"
    } else if live {
        "available"
    } else if limit_exceeded {
        "request_limit_exceeded"
    } else if !has_capacity {
        "awaiting_capacity"
    } else if already_authorized {
        "ended"
    } else if !pending && refused {
        "renewal_refused"
    } else if has_reservation && new_period && !pending && t.renewal_mode == RenewalMode::Owner {
        "awaiting_owner"
    } else {
        "awaiting_executor"
    };
    let can_renew = open
        && has_reservation
        && new_period
        && !pending
        && !live
        && !already_authorized
        && has_capacity
        && p.user_id == r.administrator_user_id
        && (t.renewal_mode == RenewalMode::Owner || refused);
    Ok((
        Some(t.calendar),
        Some(t.renewal_mode),
        Some(PoolAgentPeriod {
            key: period.key,
            start_ms: period.start_ms,
            end_ms: period.end_ms,
            tokens_per_period: r.requested_tokens,
            consumed_tokens: consumed,
            remaining_tokens: if unsettled {
                None
            } else {
                Some(if live {
                    (r.requested_tokens - consumed).max(0)
                } else {
                    0
                })
            },
            state: state.into(),
            next_reset_ms: period.end_ms,
        }),
        can_renew,
    ))
}
#[derive(QueryableByName)]
struct PeriodAttempt {
    #[diesel(sql_type = Nullable<Text>)]
    state: Option<String>,
}
async fn latest_attempt(db: &mut AsyncPgConnection, intent: Option<&str>, period: &str) -> Result<Option<PeriodAttempt>> {
    Ok(sql_query("SELECT receipt::jsonb->>'state' AS state FROM hagency_agent_v1.management_agent_executor_commands WHERE intent_id=$1 AND operation='agent.period.renew' AND period_key=$2 ORDER BY (receipt::jsonb->>'state' IS DISTINCT FROM 'refused') DESC,created_at_ms DESC,id DESC LIMIT 1")
        .bind::<Nullable<Text>,_>(intent).bind::<Text,_>(period).get_result(db).await.optional()?)
}
pub(super) async fn enqueue(
    db: &mut AsyncPgConnection,
    r: &Row,
    now: i64,
    owner_action: bool,
    server: &str,
) -> Result<bool> {
    let t = terms(db, &r.id)
        .await?
        .ok_or(Error::Conflict("agent_not_recurring"))?;
    if !owner_action && t.renewal_mode != RenewalMode::Automatic {
        return Ok(false);
    }
    let period = period(&t, now)?;
    let original=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1 AND state<>'fenced'").bind::<Text,_>(&r.id).get_result::<AgentPerson>(db).await.optional()?.ok_or(Error::Conflict("agent_allocation_fenced"))?;
    let c: ManagedAgentReserveCommand = decode(&original.payload)?;
    if c.renewal_mode != Some(t.renewal_mode)
        || c.pool_grant.as_ref().and_then(|g| g.calendar.as_ref()) != Some(&t.calendar)
    {
        return Err(Error::Conflict("agent_period_terms_changed"));
    }
    if r.state != "approved_waiting_executor" || r.revision != c.request_revision {
        return Err(Error::Conflict("pool_agent_request_closed"));
    }
    if period.start_ms < (c.pool_grant.as_ref().unwrap().period_end_ms as u64) {
        return Err(Error::Conflict("agent_period_already_authorized"));
    }
    let attempt = latest_attempt(db, Some(&c.command_id), &period.key).await?;
    // A missing receipt is an unknown native outcome. Only a durable refusal
    // permits a fresh command; authorized periods never receive another credit.
    if attempt.as_ref().is_some_and(|a| a.state.as_deref() != Some("refused")) {
        return Ok(false);
    }
    let g = pool::effective_grant(db, r, now).await?;
    // Automatic mode waits for changed authority after a definitive refusal.
    // An explicit owner action may retry without changing the approved terms.
    if !owner_action && attempt.is_some() && sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands WHERE intent_id=$1 AND operation='agent.period.renew' AND period_key=$2 AND receipt::jsonb->>'state'='refused' AND command_payload::jsonb->>'currentGrantDigest'=$3) AS matched")
        .bind::<Text,_>(&c.command_id).bind::<Text,_>(&period.key).bind::<Text,_>(&g.receipt_digest).get_result::<Flag>(db).await?.matched {
        return Ok(false);
    }
    if g.receipt.request_limit() < r.requested_tokens {
        return Err(Error::Conflict("pool_agent_request_limit_exceeded"));
    }
    if g.receipt.calendar.as_ref() != Some(&t.calendar)
        || g.receipt.period_key != period.key
    {
        return Err(Error::Conflict("pool_capacity_unavailable"));
    }
    let a=sql_query("SELECT json_build_object('id',id,'generation',generation,'child',agent_reservation_id)::text AS payload FROM hagency_agent_v1.management_shared_assignments WHERE request_id=$1 AND state='pending_native'").bind::<Text,_>(&r.id).get_result::<AgentPerson>(db).await.optional()?.ok_or(Error::Conflict("agent_assignment_unavailable"))?;
    let a: Value = decode(&a.payload)?;
    let command = ManagedAgentPeriodRenewCommand {
        version: 1,
        capability: "hagency.agent_periods.v1".into(),
        operation: "agent.period.renew".into(),
        command_id: format!("map_{}", entity_id()?),
        reserve_command_digest: allocation_digest(&c)?,
        agent_reservation_id: a["child"].as_str().unwrap().into(),
        assignment_id: a["id"].as_str().unwrap().into(),
        assignment_generation: a["generation"].as_i64().unwrap(),
        current_grant_digest: g.receipt_digest,
        decision_actor_mxid: g.receipt.owner_mxid.clone(),
        expires_at_ms: g
            .receipt
            .period_end_ms
            .min(g.receipt.delegation_expires_at_ms),
        current_grant: g.receipt,
        period,
        renewal_mode: t.renewal_mode,
        issued_at_ms: now,
        reserve_command: c,
    };
    command.validate(server)?;
    sql_query("INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms,command_payload,period_key) VALUES($1,$2,'agent.period.renew',$3,$4,$5)").bind::<Text,_>(&command.command_id).bind::<Text,_>(&command.reserve_command.command_id).bind::<BigInt,_>(now).bind::<Text,_>(wire(&command)?).bind::<Text,_>(&command.period.key).execute(db).await?;
    Ok(true)
}
pub(super) async fn automatic(
    db: &mut AsyncPgConnection,
    p: &Principal,
    now: i64,
    server: &str,
) -> Result<()> {
    #[derive(QueryableByName)]
    struct Candidate {
        #[diesel(sql_type = Text)] id: String,
        #[diesel(sql_type = Text)] request_id: String,
    }
    let cursor = sql_query("SELECT after_intent_id AS payload FROM hagency_agent_v1.management_agent_renewal_cursors WHERE executor_device_id=$1")
        .bind::<Nullable<Text>,_>(p.device_id.as_deref()).get_result::<AgentPerson>(db).await.optional()?.map(|r| r.payload).unwrap_or_default();
    // Rotate a bounded page even when every candidate in it is unavailable.
    // This prevents early live/invalid intents from starving later Agents.
    let query = "SELECT i.id,i.request_id FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.management_agent_recurring_terms t ON t.request_id=i.request_id JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=i.request_id WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND i.state<>'fenced' AND a.state='pending_native' AND t.renewal_mode='automatic' AND i.id>$3 ORDER BY i.id LIMIT 1000";
    let mut ids = sql_query(query).bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(p.device_id.as_deref()).bind::<Text,_>(&cursor).load::<Candidate>(db).await?;
    if ids.is_empty() && !cursor.is_empty() {
        ids = sql_query(query).bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(p.device_id.as_deref()).bind::<Text,_>("").load::<Candidate>(db).await?;
    }
    for id in &ids {
        // Each Agent gets a savepoint: malformed/oversized durable input cannot
        // poison the executor transaction or suppress unrelated delivery.
        let result = db.transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            let r = row(db, &id.request_id).await?;
            enqueue(db, &r, now, false, server).await
        }).await;
        match result {
            Ok(_) | Err(Error::Conflict(_) | Error::NotFound(_)) => {}
            Err(Error::Invalid(reason) | Error::Unavailable(reason)) => tracing::warn!(request_id=%id.request_id, reason, "automatic Agent renewal rejected"),
            Err(e) => return Err(e),
        }
    }
    if let Some(last) = ids.last() {
        sql_query("INSERT INTO hagency_agent_v1.management_agent_renewal_cursors(executor_device_id,after_intent_id) VALUES($1,$2) ON CONFLICT(executor_device_id) DO UPDATE SET after_intent_id=EXCLUDED.after_intent_id")
            .bind::<Nullable<Text>,_>(p.device_id.as_deref()).bind::<Text,_>(&last.id).execute(db).await?;
    }
    Ok(())
}
impl DomainStore {
    pub async fn renew_pool_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RenewPoolAgent,
    ) -> Result<PoolAgentRequest> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision < 1 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let r = row(db, id).await?;
                if r.pool_contribution_id.is_none() || r.administrator_user_id != p.user_id {
                    return Err(Error::Unauthorized("pool_resource_owner_required"));
                }
                if let Some(saved) = replay::<PoolAgentRequest>(
                    db,
                    p,
                    &input.command_id,
                    "pool_agent.renew",
                    &payload,
                )
                .await?
                {
                    return pool::view_pool(db, &saved.id, p, now).await;
                }
                if r.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                let t = terms(db, id)
                    .await?
                    .ok_or(Error::Conflict("agent_not_recurring"))?;
                if period(&t, now)?.key != input.expected_period_key {
                    return Err(Error::Conflict("agent_period_changed"));
                }
                enqueue(db, &r, now, true, &self.server).await?;
                let result = pool::view_pool(db, id, p, now).await?;
                Self::audit(db, p, "pool_agent.renew", id, now).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "pool_agent.renew",
                    &payload,
                    &result,
                )
                .await?;
                Ok(result)
            })
            .await
    }
}

pub(super) async fn recovery_grant(
    db: &mut AsyncPgConnection,
    r: &Row,
    p: &Principal,
    now: i64,
) -> Result<Option<crate::management_allocation_types::ResourceGrantView>> {
    if r.administrator_user_id != p.user_id {
        return Ok(None);
    }
    let Some(pool) = &r.pool_contribution_id else {
        return Ok(None);
    };
    let Some(g) = allocations::live_grant_for_pool(db, pool, now).await? else {
        return Ok(None);
    };
    if g.receipt.calendar.is_none() || g.receipt.request_limit() < r.requested_tokens {
        return Ok(None);
    }
    let funded=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id WHERE i.payload::jsonb->>'proposedAgentId'=$1 AND ((w.operation='agent.reserve' AND w.receipt::jsonb->>'state'='reserved' AND i.payload::jsonb->'poolGrant'->'calendar' IS NOT NULL AND i.payload::jsonb->'poolGrant'->'calendar'<>'null'::jsonb AND (i.payload::jsonb->'poolGrant'->>'periodStartMs')::bigint<$3 AND (i.payload::jsonb->'poolGrant'->>'periodEndMs')::bigint>$2) OR (w.operation='agent.period.renew' AND w.receipt::jsonb->>'state'='authorized' AND (w.command_payload::jsonb->'period'->>'startMs')::bigint<$3 AND (w.command_payload::jsonb->'period'->>'endMs')::bigint>$2))) AS matched").bind::<Text,_>(&r.proposed_agent_id).bind::<BigInt,_>(g.receipt.period_start_ms).bind::<BigInt,_>(g.receipt.period_end_ms).get_result::<Flag>(db).await?.matched;
    if funded {
        return Ok(None);
    }
    let valid=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.fence' JOIN hagency_agent_v1.agents a ON a.id=$2 JOIN hagency_agent_v1.users u ON u.id=$3 WHERE i.request_id=$1 AND i.state='fenced' AND w.receipt::jsonb->>'state' IN ('fenced','absent_fenced') AND a.state='suspended' AND a.owner_user_id=$3 AND u.active AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_reallocations x WHERE x.previous_request_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments x WHERE x.agent_id=$2 AND x.state='pending_native') AND i.payload::jsonb->>'resourceId'=$4 AND i.payload::jsonb->>'resourceRevision'=$5 AND $6::jsonb ? u.mxid) AS matched")
        .bind::<Text,_>(&r.id).bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&r.manager_user_id).bind::<Text,_>(&g.receipt.resource_id).bind::<Text,_>(&g.receipt.resource_revision).bind::<Text,_>(wire(&g.receipt.eligible_managers)?).get_result::<Flag>(db).await?.matched;
    Ok(valid.then_some(g))
}
impl DomainStore {
    /// A new owner approval creates a new native allocation generation while
    /// all evidence of the old irreversible fence remains untouched.
    pub async fn reallocate_pool_agent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ReallocatePoolAgent,
    ) -> Result<PoolAgentRequest> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        let payload = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let old=row(db,id).await?;
            if old.administrator_user_id!=p.user_id {return Err(Error::Unauthorized("pool_resource_owner_required"))}
            if let Some(saved)=replay::<PoolAgentRequest>(db,p,&input.command_id,"pool_agent.reallocate",&payload).await? {return pool::view_pool(db,&saved.id,p,now).await}
            if old.revision!=input.expected_revision {return Err(Error::Conflict("management_revision_changed"))}
            let grant=recovery_grant(db,&old,p,now).await?.ok_or(Error::Conflict("agent_reallocation_unavailable"))?;
            let g=&grant.receipt;
            if g.grant_revision!=input.expected_grant_revision {return Err(Error::Conflict("management_revision_changed"))}
            let previous=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&old.id).get_result::<AgentPerson>(db).await?;
            let previous:ManagedAgentReserveCommand=decode(&previous.payload)?;
            let generation=previous.allocation_generation.unwrap_or(1).checked_add(1).filter(|n|*n<=crate::management_allocation_types::MAX_EXACT).ok_or(Error::Conflict("agent_generation_overflow"))?;
            let id=format!("par_{}",entity_id()?);
            let expires=g.period_end_ms.min(g.delegation_expires_at_ms);
            sql_query("INSERT INTO hagency_agent_v1.management_agent_requests(id,setup_binding_revision,allocation_receipt_digest,manager_user_id,room_id,proposed_agent_id,definition,definition_digest,requested_tokens,period_key,administrator_user_id,authority_revision,state,created_at_ms,expires_at_ms,pool_contribution_id,pool_grant_id) VALUES($1,0,'',$2,$3,$4,$5,$6,$7,$8,$9,$10,'pending_approval',$11,$12,$13,$14)")
                .bind::<Text,_>(&id).bind::<Text,_>(&old.manager_user_id).bind::<Text,_>(&old.room_id).bind::<Text,_>(&old.proposed_agent_id).bind::<Text,_>(&old.definition).bind::<Text,_>(&old.definition_digest).bind::<BigInt,_>(old.requested_tokens).bind::<Text,_>(&g.period_key).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(g.grant_revision).bind::<BigInt,_>(now).bind::<BigInt,_>(expires).bind::<Text,_>(&g.contribution_id).bind::<Text,_>(&g.grant_id).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_agent_recurring_terms(request_id,calendar,renewal_mode) VALUES($1,$2,$3)").bind::<Text,_>(&id).bind::<Text,_>(wire(g.calendar.as_ref().unwrap())?).bind::<Text,_>(if input.renewal_mode==RenewalMode::Automatic {"automatic"}else{"owner"}).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_agent_reallocations(request_id,previous_request_id,allocation_generation) VALUES($1,$2,$3)").bind::<Text,_>(&id).bind::<Text,_>(&old.id).bind::<BigInt,_>(generation).execute(db).await?;
            let verdict=allocation_digest(&json!({"version":1,"requestId":id,"requestRevision":1,"resourceOwnerMxid":p.mxid,"commandId":input.command_id,"decision":"approve","previousRequestId":old.id,"allocationGeneration":generation}))?;
            let command=ManagedAgentReserveCommand {version:2,renewal_mode:Some(input.renewal_mode),allocation_generation:Some(generation),pool_grant:Some(g.clone()),capability:"hagency.pool_agents.v1".into(),operation:"agent.reserve".into(),command_id:format!("mae_{}",entity_id()?),request_id:id.clone(),request_revision:2,proposed_agent_id:old.proposed_agent_id.clone(),manager_mxid:previous.manager_mxid,project_id:String::new(),project_revision:0,setup_id:String::new(),setup_binding_revision:0,domain_project_id:String::new(),room_id:old.room_id.clone(),authority_revision:g.grant_revision,decision_actor_mxid:p.mxid.clone(),decision_digest:verdict.clone(),allocation_id:id.clone(),allocation_receipt_digest:String::new(),grant_receipt_digest:grant.receipt_digest,enrollment_id:g.enrollment_id.clone(),executor_device_id:g.executor_device_id.clone(),executor_generation:g.executor_generation,resource_id:g.resource_id.clone(),resource_revision:g.resource_revision.clone(),definition:decode(&old.definition)?,definition_digest:old.definition_digest,requested_tokens:old.requested_tokens,period_key:g.period_key.clone(),issued_at_ms:now,expires_at_ms:expires};
            command.validate(&self.server)?;
            sql_query("INSERT INTO hagency_agent_v1.management_agent_intents(id,request_id,executor_owner_user_id,executor_device_id,executor_generation,payload,digest) VALUES($1,$2,$3,$4,$5,$6,$7)").bind::<Text,_>(&command.command_id).bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&g.executor_device_id).bind::<BigInt,_>(g.executor_generation).bind::<Text,_>(wire(&command)?).bind::<Text,_>(allocation_digest(&command)?).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_agent_decisions(request_id,request_revision,administrator_user_id,authority_revision,command_id,digest,decision,decided_at_ms) VALUES($1,1,$2,$3,$4,$5,'approve',$6)").bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(g.grant_revision).bind::<Text,_>(&input.command_id).bind::<Text,_>(&verdict).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_agent_requests SET state='approved_waiting_executor',revision=2 WHERE id=$1").bind::<Text,_>(&id).execute(db).await?;
            let result=pool::view_pool(db,&id,p,now).await?;
            Self::audit(db,p,"pool_agent.reallocate",&id,now).await?;receipt(db,p,&input.command_id,"pool_agent.reallocate",&payload,&result).await?;
            Ok(result)
        }).await
    }
}

#[cfg(test)]
impl DomainStore {
    pub(crate) async fn reset_management_test_clock(&self) -> Result<()> {
        let mut db=self.db.lock().await?;
        sql_query("SELECT set_config('hagency.test_clock_ms','',false)").execute(&mut *db).await?;
        Ok(())
    }
    pub(crate) async fn set_management_test_clock(&self, now: i64) -> Result<()> {
        let mut db = self.db.lock().await?;
        sql_query("SELECT set_config('hagency.test_clock_ms',$1,false)")
            .bind::<Text, _>(now.to_string())
            .execute(&mut *db)
            .await?;
        Ok(())
    }
}

pub(super) async fn assignment_period(db:&mut AsyncPgConnection,assignment:&str,now:i64)->Result<Option<Period>> {
    let calendar=sql_query("SELECT t.calendar AS payload FROM hagency_agent_v1.management_agent_recurring_terms t JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=t.request_id WHERE a.id=$1").bind::<Text,_>(assignment).get_result::<AgentPerson>(db).await.optional()?;
    calendar.map(|r|decode::<Calendar>(&r.payload)?.period_at(now as u64).map_err(|_|Error::Conflict("agent_period_changed"))).transpose()
}
