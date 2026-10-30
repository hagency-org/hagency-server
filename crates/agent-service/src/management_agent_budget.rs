//! Phase A: human decisions and exact native delta evidence. No increased-budget model authority.
use super::*;
use crate::management_agent_budget_types::*;
use crate::management_allocation_types::{ExecutorPollInput, ExecutorRecoverInput, MAX_EXACT};
#[path = "management_agent_budget_executor.rs"]
mod delivery;
#[derive(QueryableByName)]
struct Base {
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=BigInt)]
    assignment_generation: i64,
    #[diesel(sql_type=Text)]
    assignment_state: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    device_id: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
    #[diesel(sql_type=Text)]
    command_payload: String,
    #[diesel(sql_type=Text)]
    command_digest: String,
    #[diesel(sql_type=Text)]
    receipt_payload: String,
    #[diesel(sql_type=Text)]
    receipt_digest: String,
}
#[derive(QueryableByName)]
struct Top {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=BigInt)]
    expected_budget_revision: i64,
    #[diesel(sql_type=Text)]
    expected_budget_digest: String,
    #[diesel(sql_type=BigInt)]
    expected_ceiling_tokens: i64,
    #[diesel(sql_type=BigInt)]
    add_tokens: i64,
    #[diesel(sql_type=Text)]
    purpose: String,
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
struct Key {
    #[diesel(sql_type=Text)]
    id: String,
}
struct Head {
    revision: i64,
    digest: String,
    ceiling: i64,
    original: i64,
    known: i64,
    floor: i64,
    ledger: i64,
}
fn enabled(p: &ManagementPolicy) -> Result<()> {
    if !p.agent_budget_deltas() {
        return Err(Error::Unavailable("agent_budget_issuance_unavailable"));
    }
    Ok(())
}
async fn base(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<(
    Base,
    ManagedAgentReserveCommand,
    ManagedAgentReservationReceipt,
)> {
    let b=sql_query("SELECT a.id AS assignment_id,a.generation AS assignment_generation,a.state AS assignment_state,a.executor_owner_user_id AS owner_user_id,a.executor_device_id AS device_id,a.executor_generation AS generation,i.payload AS command_payload,i.digest AS command_digest,w.receipt AS receipt_payload,a.reserve_receipt_digest AS receipt_digest FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.reserve' WHERE a.request_id=$1 AND w.receipt IS NOT NULL").bind::<Text,_>(id).get_result::<Base>(db).await.map_err(hidden)?;
    let c: ManagedAgentReserveCommand = decode(&b.command_payload)?;
    let r: ManagedAgentReservationReceipt = decode(&b.receipt_payload)?;
    c.validate(server)?;
    r.validate_for(&c)?;
    if c.request_id != id
        || r.state != "reserved"
        || b.command_digest != allocation_digest(&c)?
        || b.receipt_digest != allocation_digest(&r)?
    {
        return Err(Error::Conflict("agent_budget_original_changed"));
    }
    Ok((b, c, r))
}
async fn top(db: &mut AsyncPgConnection, id: &str) -> Result<Top> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_topups WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
#[derive(QueryableByName)]
struct BudgetAuthority {
    #[diesel(sql_type=Text)] administrator_user_id: String,
    #[diesel(sql_type=Text)] administrator_mxid: String,
    #[diesel(sql_type=BigInt)] authority_revision: i64,
    #[diesel(sql_type=Bool)] active: bool,
}
struct BudgetContext {
    roles: BudgetAuthority,
    grant: crate::management_allocation_types::NativeGrantReceipt,
    limit_tokens: i64,
}
async fn budget_authority(db: &mut AsyncPgConnection, r: &Row) -> Result<BudgetAuthority> {
    if r.pool_contribution_id.is_some() {
        return sql_query("SELECT o.id AS administrator_user_id,o.mxid AS administrator_mxid,$3::bigint AS authority_revision,(o.active AND m.active) AS active FROM hagency_agent_v1.users o CROSS JOIN hagency_agent_v1.users m WHERE o.id=$1 AND m.id=$2")
            .bind::<Text,_>(&r.administrator_user_id).bind::<Text,_>(&r.manager_user_id)
            .bind::<BigInt,_>(r.authority_revision).get_result(db).await.map_err(hidden);
    }
    let a=roles(db,&r.plan_id).await?;
    Ok(BudgetAuthority {administrator_user_id:a.administrator_user_id,administrator_mxid:a.administrator_mxid,authority_revision:a.authority_revision,active:a.active})
}
async fn visible(db: &mut AsyncPgConnection, p: &Principal, request: &str) -> Result<Row> {
    let r = row(db, request).await?;
    let a = budget_authority(db, &r).await?;
    if p.user_id != r.manager_user_id && p.user_id != a.administrator_user_id {
        return Err(Error::NotFound("agent_topup_not_found"));
    }
    Ok(r)
}
async fn fresh_base(
    db: &mut AsyncPgConnection,
    r: &Row,
    b: &Base,
    c: &ManagedAgentReserveCommand,
    now: i64,
) -> Result<BudgetContext> {
    let cx=if r.pool_contribution_id.is_some() {
        // Top-ups follow the pool's chain: the effective grant sets the limit.
        let g=pool::effective_grant(db,r,now).await?;
        if !pool::command_live(db,r,c,now).await? || r.administrator_user_id!=b.owner_user_id {
            return Err(Error::Conflict("agent_budget_authority_changed"));
        }
        BudgetContext {roles:budget_authority(db,r).await?,limit_tokens:g.receipt.allocated_tokens,grant:g.receipt}
    } else {
        let cx=context(db,&r.plan_id,now).await?;
        if !cx.matches(r) || cx.capacity.grant_digest!=c.grant_receipt_digest
            || cx.owner!=b.owner_user_id || cx.roles.manager_mxid!=c.manager_mxid {
            return Err(Error::Conflict("agent_budget_authority_changed"));
        }
        BudgetContext {roles:budget_authority(db,r).await?,limit_tokens:cx.capacity.requested_tokens,grant:cx.capacity.grant}
    };
    let live=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id WHERE d.id=$1 AND d.generation=$2 AND d.user_id=$3 AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>$4 AND u.active) AS matched").bind::<Text,_>(&b.device_id).bind::<BigInt,_>(b.generation).bind::<Text,_>(&b.owner_user_id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched;
    if !live || !cx.roles.active
        || b.assignment_state == "fenced"
        || r.state != "approved_waiting_executor"
        || now >= super::executor::reserve_lifetime(db, c, now).await?
        || cx.grant.executor_device_id != b.device_id
        || cx.grant.executor_generation != b.generation
        || cx.grant.resource_revision != c.resource_revision
    {
        return Err(Error::Conflict("agent_budget_authority_changed"));
    }
    Ok(cx)
}
async fn head(
    db: &mut AsyncPgConnection,
    id: &str,
    b: &Base,
    original: &ManagedAgentReservationReceipt,
    server: &str,
) -> Result<Head> {
    let mut h = Head {
        revision: 1,
        digest: allocation_digest(
            &json!({"version":1,"originalReserveReceiptDigest":b.receipt_digest,"budgetRevision":1,"effectiveCeilingTokens":original.allocated_tokens}),
        )?,
        ceiling: original.allocated_tokens,
        original: original.allocated_tokens,
        known: 0,
        floor: 0,
        ledger: original.ledger_revision,
    };
    #[derive(QueryableByName)]
    struct Pair {
        #[diesel(sql_type=Text)]
        command: String,
        #[diesel(sql_type=Text)]
        receipt: String,
    }
    let rows=sql_query("SELECT c.payload AS command,r.payload AS receipt FROM hagency_agent_v1.management_agent_budget_commands c JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.request_id=$1 AND r.state='applied' ORDER BY r.budget_revision LIMIT 10001").bind::<Text,_>(id).load::<Pair>(db).await?;
    if rows.len() > 10000 {
        return Err(Error::Unavailable("agent_budget_history_capacity"));
    }
    for p in rows {
        let c: AgentBudgetCommand = decode(&p.command)?;
        c.validate(server)?;
        let r: AgentBudgetReceipt = decode(&p.receipt)?;
        r.validate_for(&c)?;
        if c.assignment_id != b.assignment_id
            || c.reserve_receipt_digest != b.receipt_digest
            || c.expected_budget_revision != h.revision
            || c.expected_budget_digest != h.digest
            || c.expected_ceiling_tokens != h.ceiling
            || r.ledger_revision <= h.ledger
        {
            return Err(Error::Conflict("agent_budget_chain_changed"));
        }
        h.revision = r.budget_revision;
        h.digest = r.budget_digest;
        h.ceiling = r.effective_ceiling_tokens;
        h.ledger = r.ledger_revision;
    }
    h.known = super::super::usage::known(db, super::super::usage::Scope::Agent(id)).await?;
    let retained=sql_query("SELECT LEAST(coalesce(sum(greatest(w.known_consumed_tokens,coalesce(w.retained_tokens,w.known_consumed_tokens))),0),9007199254740992)::bigint AS now FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1").bind::<Text,_>(&b.assignment_id).get_result::<Clock>(db).await?.now;
    if retained > MAX_EXACT {
        return Err(Error::Unavailable("shared_usage_overflow"));
    }
    let pending_ceiling=sql_query("SELECT coalesce(max((c.payload::jsonb->>'expectedCeilingTokens')::bigint+(c.payload::jsonb->>'addTokens')::bigint),0)::bigint AS now FROM hagency_agent_v1.management_agent_budget_commands c LEFT JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.request_id=$1 AND r.command_id IS NULL").bind::<Text,_>(id).get_result::<Clock>(db).await?.now;
    h.floor = h.ceiling.max(h.known).max(retained).max(pending_ceiling);
    if let Some(settled)=super::settlement::retained_for_head(db,id,h.revision,&h.digest,h.ceiling,h.known,server).await? { h.known=h.known.max(settled.known); h.floor=if settled.complete { settled.retained.max(h.known) } else {h.floor.max(settled.retained).max(h.known)}; }
    Ok(h)
}
async fn pending(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_topups t LEFT JOIN hagency_agent_v1.management_agent_budget_commands c ON c.topup_id=t.id LEFT JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE t.request_id=$1 AND (t.state='pending' AND t.expires_at_ms>(extract(epoch from clock_timestamp())*1000)::bigint OR t.state IN('approved','cancellation_requested') AND r.command_id IS NULL)) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
/// What the server knows the pool Agents of one capacity chain hold: each
/// live reserved Agent's budget floor, and the requested amount of every
/// approved reserve whose native outcome is not yet known. A successor grant
/// may not be smaller (the native ledger independently checks its own floor).
pub(in crate::domain::management) async fn chain_reserved_total(
    db: &mut AsyncPgConnection,
    root: &str,
    server: &str,
) -> Result<i64> {
    #[derive(QueryableByName)]
    struct Live {
        #[diesel(sql_type=Text)]
        id: String,
        #[diesel(sql_type=BigInt)]
        requested_tokens: i64,
        #[diesel(sql_type=Bool)]
        assigned: bool,
        #[diesel(sql_type=Text)]
        reserve_state: String,
    }
    let rows=sql_query("SELECT q.id,q.requested_tokens,EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a WHERE a.request_id=q.id) AS assigned,coalesce(w.receipt::jsonb->>'state','unknown') AS reserve_state FROM hagency_agent_v1.management_agent_requests q JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=q.id JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.reserve' WHERE q.pool_grant_id IN (SELECT id FROM hagency_agent_v1.management_resource_grants WHERE chain_root_id=$1) AND i.state<>'fenced' ORDER BY q.id LIMIT 1001")
        .bind::<Text,_>(root).load::<Live>(db).await?;
    if rows.len() > 1000 {
        return Err(Error::Unavailable("agent_budget_history_capacity"));
    }
    let mut held: i64 = 0;
    for row in rows {
        let value = if row.assigned {
            let (b, _, receipt) = base(db, &row.id, server).await?;
            head(db, &row.id, &b, &receipt, server).await?.floor
        } else if row.reserve_state != "refused" {
            row.requested_tokens
        } else {
            0
        };
        held = held
            .checked_add(value)
            .filter(|n| *n <= MAX_EXACT)
            .ok_or(Error::Unavailable("shared_usage_overflow"))?;
    }
    Ok(held)
}
/// A missing top-up result may already have increased the native hold. Cleanup
/// reads must retain that possibility even after admission/budget issuance stops.
/// A complete native fence remains the separate authority for a lower balance.
pub(super) async fn cleanup_commitment(
    db: &mut AsyncPgConnection,
    request: &str,
    server: &str,
) -> Result<Option<i64>> {
    let has_delta = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands WHERE request_id=$1) AS matched")
        .bind::<Text, _>(request)
        .get_result::<Flag>(db)
        .await?
        .matched;
    if !has_delta {
        return Ok(None);
    }
    let (b, _, original) = base(db, request, server).await?;
    Ok(Some(head(db, request, &b, &original, server).await?.floor))
}
async fn fits_parent(
    db: &mut AsyncPgConnection,
    r: &Row,
    cx: &BudgetContext,
    own: &Head,
    add: i64,
    server: &str,
) -> Result<()> {
    // A pool Agent's parent is its whole capacity chain (renewals and
    // amendments carry Agents), bounded by the effective grant's amount.
    let rows=sql_query("SELECT q.id FROM hagency_agent_v1.management_agent_requests q JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=q.id WHERE (($2::text IS NULL AND q.pool_grant_id IS NULL AND q.allocation_id=$1) OR q.pool_grant_id IN (SELECT id FROM hagency_agent_v1.management_resource_grants WHERE chain_root_id=(SELECT chain_root_id FROM hagency_agent_v1.management_resource_grants WHERE id=$2))) ORDER BY q.id LIMIT 1001").bind::<Text,_>(&r.allocation_id).bind::<Nullable<Text>,_>(&r.pool_grant_id).load::<Key>(db).await?;
    if rows.len() > 1000 {
        return Err(Error::Unavailable("agent_budget_history_capacity"));
    }
    // An approved reserve with no acknowledged child may already have applied.
    // Keep that original commitment until an exact refusal/absence receipt exists.
    let unresolved=sql_query("SELECT LEAST(coalesce(sum(q.requested_tokens),0),9007199254740992)::bigint AS now FROM hagency_agent_v1.management_agent_requests q JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=q.id JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.reserve' WHERE (($2::text IS NULL AND q.pool_grant_id IS NULL AND q.allocation_id=$1) OR q.pool_grant_id IN (SELECT id FROM hagency_agent_v1.management_resource_grants WHERE chain_root_id=(SELECT chain_root_id FROM hagency_agent_v1.management_resource_grants WHERE id=$2))) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a WHERE a.request_id=q.id) AND coalesce(w.receipt::jsonb->>'state','unknown')<>'refused' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands f WHERE f.intent_id=i.id AND f.operation='agent.fence' AND f.receipt::jsonb->>'state'='absent_fenced')").bind::<Text,_>(&r.allocation_id).bind::<Nullable<Text>,_>(&r.pool_grant_id).get_result::<Clock>(db).await?.now;
    if unresolved > MAX_EXACT {
        return Err(Error::Unavailable("shared_usage_overflow"));
    }
    let mut held = unresolved;
    for k in rows {
        let value = if k.id == r.id {
            own.ceiling
                .checked_add(add)
                .filter(|n| *n <= MAX_EXACT)
                .ok_or(Error::Invalid("invalid_agent_budget"))?
                .max(own.floor)
        } else {
            let (b, _, receipt) = base(db, &k.id, server).await?;
            head(db, &k.id, &b, &receipt, server).await?.floor
        };
        held = held
            .checked_add(value)
            .filter(|n| *n <= MAX_EXACT)
            .ok_or(Error::Unavailable("shared_usage_overflow"))?;
    }
    if held > cx.limit_tokens {
        return Err(Error::Conflict(if r.pool_grant_id.is_some(){"agent_budget_exceeds_pool"}else{"agent_budget_exceeds_project"}));
    }
    Ok(())
}
async fn view(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<AgentTopUp> {
    let t = top(db, id).await?;
    let r = visible(db, p, &t.request_id).await?;
    let a = budget_authority(db, &r).await?;
    #[derive(QueryableByName)]
    struct Native {
        #[diesel(sql_type=Text)]
        id: String,
        #[diesel(sql_type=Nullable<Text>)]
        state: Option<String>,
    }
    let n=sql_query("SELECT c.id,r.state FROM hagency_agent_v1.management_agent_budget_commands c LEFT JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.topup_id=$1").bind::<Text,_>(id).get_result::<Native>(db).await.optional()?;
    let native_state = n.as_ref().and_then(|n| n.state.clone()).unwrap_or_else(|| {
        if n.is_some() {
            "unknown".into()
        } else {
            "not_delivered".into()
        }
    });
    let input: RequestAgentTopUp = decode(&t.payload)?;
    Ok(AgentTopUp {
        owner_funded: input.owner_funded,
        id: t.id,
        authority_revision: a.authority_revision,
        request_id: t.request_id,
        assignment_id: t.assignment_id,
        add_tokens: t.add_tokens,
        purpose: t.purpose,
        expected_budget_revision: t.expected_budget_revision,
        expected_budget_digest: t.expected_budget_digest,
        expected_ceiling_tokens: t.expected_ceiling_tokens,
        revision: t.revision,
        state: if t.state == "pending" && now >= t.expires_at_ms {
            "expired".into()
        } else {
            t.state.clone()
        },
        native_state,
        executor_command_id: n.as_ref().map(|n| n.id.clone()),
        created_at_ms: t.created_at_ms,
        expires_at_ms: t.expires_at_ms,
        can_decide: policy.agent_budget_deltas()
            && a.active
            && p.user_id == a.administrator_user_id
            && t.state == "pending"
            && now < t.expires_at_ms,
        can_cancel: p.user_id == t.manager_user_id
            && matches!(t.state.as_str(), "pending" | "approved")
            && n.as_ref().is_none_or(|n| n.state.is_none()),
    })
}
async fn reply(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    ack: AgentTopUpAck,
    now: i64,
) -> Result<AgentTopUpReply> {
    let top_up = view(db, &ack.top_up_id, p, policy, now).await?;
    Ok(AgentTopUpReply {
        receipt: ack,
        top_up,
    })
}
impl DomainStore {
    pub async fn agent_budget(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<AgentBudget> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let r = visible(db, p, id).await?;
                let (b, c, receipt) = base(db, id, &self.server).await?;
                let h = head(db, id, &b, &receipt, &self.server).await?;
                let owner_funding = r.pool_grant_id.is_some() && p.user_id == r.administrator_user_id;
                let reason = if !policy.agent_budget_deltas() {
                    Some("agent_budget_issuance_unavailable")
                } else if r.manager_user_id != p.user_id && !owner_funding {
                    Some("project_manager_required")
                } else if pending(db, id).await? {
                    Some("agent_topup_pending")
                } else if h.ceiling >= MAX_EXACT {
                    Some("agent_budget_limit")
                } else {
                    match fresh_base(db, &r, &b, &c, now).await {
                        Ok(cx) => match if owner_funding { Ok(()) } else { fits_parent(db, &r, &cx, &h, 1, &self.server).await } {
                            Ok(()) => None,
                            Err(Error::Conflict(code)) => Some(code),
                            Err(e) => return Err(e),
                        },
                        Err(Error::Conflict(_) | Error::NotFound(_)) => {
                            Some("agent_budget_authority_changed")
                        }
                        Err(e) => return Err(e),
                    }
                };
                let can_owner_fund = owner_funding && reason.is_none();
                Ok(AgentBudget {
                    can_owner_fund,
                    can_owner_fund_direct: can_owner_fund,
                    request_id: id.into(),
                    request_revision: r.revision,
                    assignment_generation: b.assignment_generation,
                    assignment_id: b.assignment_id,
                    agent_reservation_id: receipt.agent_reservation_id.unwrap(),
                    original_ceiling_tokens: h.original,
                    applied_delta_tokens: h.ceiling - h.original,
                    effective_ceiling_tokens: h.ceiling,
                    budget_revision: h.revision,
                    budget_digest: h.digest,
                    known_consumed_tokens: h.known,
                    conservative_commitment_tokens: h.floor,
                    native_retained_tokens: None,
                    native_remaining_tokens: None,
                    usage_complete: None,
                    execution_supported: false,
                    can_request: r.manager_user_id == p.user_id && reason.is_none(),
                    blocking_reason: reason.map(str::to_owned),
                })
            })
            .await
    }
    pub async fn request_agent_topup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RequestAgentTopUp,
    ) -> Result<AgentTopUpReply> {
        key(id)?;
        input.validate()?;
        device(p)?;
        let d = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;if let Some(a)=replay(db,p,&input.command_id,"agent.topup.request",&d).await?{return reply(db,p,policy,a,now).await}enabled(policy)?;let r=visible(db,p,id).await?;if input.owner_funded {
 if r.pool_grant_id.is_none() || p.user_id!=r.administrator_user_id {return Err(Error::Unauthorized("pool_resource_owner_required"))}
} else if r.manager_user_id!=p.user_id{return Err(Error::Unauthorized("project_manager_required"))}let(b,c,receipt)=base(db,id,&self.server).await?;let cx=fresh_base(db,&r,&b,&c,now).await?;let h=head(db,id,&b,&receipt,&self.server).await?;if r.revision!=input.expected_request_revision||b.assignment_generation!=input.expected_assignment_generation||h.revision!=input.expected_budget_revision||h.digest!=input.expected_budget_digest{return Err(Error::Conflict("management_revision_changed"))}
if pending(db,id).await?{return Err(Error::Conflict("agent_topup_pending"))}if !input.owner_funded {fits_parent(db,&r,&cx,&h,input.add_tokens,&self.server).await?;}
if h.ceiling.checked_add(input.add_tokens).is_none_or(|v|v>MAX_EXACT){return Err(Error::Invalid("invalid_agent_budget"))}
 let full=sql_query("SELECT count(*)>=10000 AS matched FROM hagency_agent_v1.management_agent_topups WHERE manager_user_id=$1").bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?.matched;if full{return Err(Error::Conflict("agent_topup_capacity"))}
 let target=format!("mat_{}",entity_id()?);let expires=(now+TTL).min(cx.grant.period_end_ms).min(cx.grant.delegation_expires_at_ms).min(super::executor::reserve_lifetime(db,&c,now).await?);
 sql_query("INSERT INTO hagency_agent_v1.management_agent_topups(id,request_id,assignment_id,manager_user_id,payload,digest,expected_budget_revision,expected_budget_digest,expected_ceiling_tokens,add_tokens,purpose,state,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'pending',$12,$13)").bind::<Text,_>(&target).bind::<Text,_>(id).bind::<Text,_>(&b.assignment_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(wire(&input)?).bind::<Text,_>(&d).bind::<BigInt,_>(h.revision).bind::<Text,_>(&h.digest).bind::<BigInt,_>(h.ceiling).bind::<BigInt,_>(input.add_tokens).bind::<Text,_>(&input.purpose).bind::<BigInt,_>(now).bind::<BigInt,_>(expires).execute(db).await?;
 let a=AgentTopUpAck{command_id:input.command_id.clone(),operation:"agent.topup.request".into(),top_up_id:target,applied_revision:1};super::receipt(db,p,&input.command_id,"agent.topup.request",&d,&a).await?;Self::audit(db,p,"agent.topup.request",&a.top_up_id,now).await?;
 // The owner already confirmed funding and approval together. Persist both receipts
 // atomically; a lost reply replays this original request, never another increase.
 if input.approve_immediately {
     let authority=budget_authority(db,&r).await?;
     let decision=DecideAgentTopUp {command_id:format!("mad_{}",entity_id()?),expected_revision:1,expected_authority_revision:authority.authority_revision,decision:Decision::Approve};
     self.decide_agent_topup_in_transaction(db,p,policy,&a.top_up_id,&decision,now).await?;
 }
 reply(db,p,policy,a,now).await}).await
    }
    pub async fn decide_agent_topup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: DecideAgentTopUp,
    ) -> Result<AgentTopUpReply> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 || input.expected_authority_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let d = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, crate::api::now_ms()).await?;
            let now = clock(db).await?;
            if let Some(a) = replay(db, p, &input.command_id, "agent.topup.decide", &d).await? {
                return reply(db, p, policy, a, now).await;
            }
            self.decide_agent_topup_in_transaction(db, p, policy, id, &input, now).await
        }).await
    }
    async fn decide_agent_topup_in_transaction(
        &self, db: &mut AsyncPgConnection, p: &Principal, policy: &ManagementPolicy,
        id: &str, input: &DecideAgentTopUp, now: i64,
    ) -> Result<AgentTopUpReply> {
        let d = digest(id, input)?;
        enabled(policy)?;let t=top(db,id).await?;let r=visible(db,p,&t.request_id).await?;let roles=budget_authority(db,&r).await?;if !roles.active||p.user_id!=roles.administrator_user_id{return Err(Error::Unauthorized(if r.pool_grant_id.is_some(){"pool_resource_owner_required"}else{"assigned_project_administrator_required"}))}
if t.revision!=input.expected_revision||roles.authority_revision!=input.expected_authority_revision{return Err(Error::Conflict("management_revision_changed"))}
if t.state!="pending"||now>=t.expires_at_ms{return Err(Error::Conflict("agent_topup_closed"))}
 let request_input:RequestAgentTopUp=decode(&t.payload)?;
 let approved=matches!(input.decision,Decision::Approve);if approved{let(b,c,receipt)=base(db,&r.id,&self.server).await?;let cx=fresh_base(db,&r,&b,&c,now).await?;let h=head(db,&r.id,&b,&receipt,&self.server).await?;if h.revision!=t.expected_budget_revision||h.digest!=t.expected_budget_digest||h.ceiling!=t.expected_ceiling_tokens{return Err(Error::Conflict("agent_budget_changed"))}if request_input.owner_funded {
 if r.pool_grant_id.is_none() || t.manager_user_id!=r.administrator_user_id {return Err(Error::Unauthorized("pool_resource_owner_required"))}
 } else {fits_parent(db,&r,&cx,&h,t.add_tokens,&self.server).await?;}
 let cmd=AgentBudgetCommand{owner_funded:request_input.owner_funded,version:if request_input.owner_funded{2}else{1},capability:BUDGET_CAPABILITY.into(),operation:"agent.budget.increase".into(),command_id:format!("mab_{}",entity_id()?),top_up_id:t.id.clone(),top_up_request_digest:t.digest.clone(),approval_command_id:input.command_id.clone(),approval_digest:d.clone(),approving_administrator_mxid:p.mxid.clone(),authority_revision:roles.authority_revision,assignment_id:b.assignment_id.clone(),assignment_generation:b.assignment_generation,reserve_command:c,reserve_command_digest:b.command_digest,reserve_receipt:receipt,reserve_receipt_digest:b.receipt_digest,expected_budget_revision:h.revision,expected_budget_digest:h.digest,expected_ceiling_tokens:h.ceiling,add_tokens:t.add_tokens,issued_at_ms:now,expires_at_ms:t.expires_at_ms};cmd.validate(&self.server)?;
 sql_query("INSERT INTO hagency_agent_v1.management_agent_budget_commands VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind::<Text,_>(&cmd.command_id).bind::<Text,_>(id).bind::<Text,_>(&r.id).bind::<Text,_>(&b.owner_user_id).bind::<Text,_>(&b.device_id).bind::<BigInt,_>(b.generation).bind::<Text,_>(wire(&cmd)?).bind::<Text,_>(allocation_digest(&cmd)?).bind::<BigInt,_>(now).execute(db).await?;}
 sql_query("UPDATE hagency_agent_v1.management_agent_topups SET state=$2,revision=revision+1 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(if approved{"approved"}else{"declined"}).execute(db).await?;let a=AgentTopUpAck{command_id:input.command_id.clone(),operation:"agent.topup.decide".into(),top_up_id:id.into(),applied_revision:t.revision+1};super::receipt(db,p,&input.command_id,"agent.topup.decide",&d,&a).await?;Self::audit(db,p,"agent.topup.decide",id,now).await?;reply(db,p,policy,a,now).await
    }
    pub async fn cancel_agent_topup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<AgentTopUpReply> {
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let d = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;if let Some(a)=replay(db,p,&input.command_id,"agent.topup.cancel",&d).await?{return reply(db,p,policy,a,now).await}let t=top(db,id).await?;if p.user_id!=t.manager_user_id{return Err(Error::Unauthorized("project_manager_required"))}
if t.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"))}
if !matches!(t.state.as_str(),"pending"|"approved"){return Err(Error::Conflict("agent_topup_closed"))}
 if t.state=="approved"{let saved=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results r JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=r.command_id WHERE c.topup_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;if saved{return Err(Error::Conflict("agent_topup_closed"))}let cmd=sql_query("SELECT id FROM hagency_agent_v1.management_agent_budget_commands WHERE topup_id=$1").bind::<Text,_>(id).get_result::<Key>(db).await?;let body=json!({"topUpId":id,"commandId":cmd.id,"humanCommandId":input.command_id,"expectedRevision":input.expected_revision});sql_query("INSERT INTO hagency_agent_v1.management_agent_budget_cancellations VALUES($1,$2,$3,$4)").bind::<Text,_>(&cmd.id).bind::<Text,_>(&input.command_id).bind::<Text,_>(wire(&body)?).bind::<Text,_>(allocation_digest(&body)?).execute(db).await?;}
 sql_query("UPDATE hagency_agent_v1.management_agent_topups SET state=$2,revision=revision+1 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(if t.state=="approved"{"cancellation_requested"}else{"cancelled"}).execute(db).await?;let a=AgentTopUpAck{command_id:input.command_id.clone(),operation:"agent.topup.cancel".into(),top_up_id:id.into(),applied_revision:t.revision+1};super::receipt(db,p,&input.command_id,"agent.topup.cancel",&d,&a).await?;Self::audit(db,p,"agent.topup.cancel",id,now).await?;reply(db,p,policy,a,now).await}).await
    }
    pub async fn agent_topup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<AgentTopUp> {
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
    pub async fn recover_agent_topup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<Value> {
        key(id)?;
        let (device, _) = device(p)?;
        if !matches!(
            input.operation.as_str(),
            "agent.topup.request" | "agent.topup.decide" | "agent.topup.cancel"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(id).load::<Receipt>(db).await?;if rows.len()>1{return Err(Error::Conflict("management_command_conflict"))}let Some(r)=rows.as_slice().first()else{return Ok(json!({"recorded":false,"result":null}))};if r.operation!=input.operation{return Err(Error::Conflict("management_command_conflict"))}let a:AgentTopUpAck=decode(&r.result)?;let now=clock(db).await?;Ok(json!({"recorded":true,"result":reply(db,p,policy,a,now).await?}))}).await
    }
}

impl DomainStore {
    pub async fn query_agent_topups(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: AgentTopUpQuery,
    ) -> Result<AgentTopUpPage> {
        key(id)?;
        if let Some(a) = &input.after {
            key(a)?
        }
        let limit = input.limit.unwrap_or(16);
        if !(1..=16).contains(&limit) {
            return Err(Error::Invalid("invalid_management_arguments"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;visible(db,p,id).await?;let now=clock(db).await?;let ids=sql_query("SELECT id FROM hagency_agent_v1.management_agent_topups WHERE request_id=$1 AND id>$2 ORDER BY id LIMIT $3").bind::<Text,_>(id).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(i64::from(limit)+1).load::<Key>(db).await?;let mut more=ids.len()>usize::from(limit);let mut bytes=0usize;let mut items=Vec::new();for row in ids.into_iter().take(usize::from(limit)){let item=view(db,&row.id,p,policy,now).await?;let size=serde_json::to_vec(&item).map_err(|_|Error::Unavailable("encoding_failed"))?.len();if bytes+size>96*1024{more=true;break}bytes+=size;items.push(item)}let next_after=if more{items.last().map(|t|t.id.clone())}else{None};Ok(AgentTopUpPage{items,next_after})}).await
    }
}

/// Internal verified applied head only. An approval or unresolved command never
/// supplies a larger execution budget. Original chain bytes are revalidated.
pub(super) async fn execution_head(
    db: &mut AsyncPgConnection,
    request: &str,
    server: &str,
) -> Result<(
    crate::management_shared_budget_types::SharedExecutionBudget,
    i64,
    i64,
)> {
    let (b, _, original) = base(db, request, server).await?;
    let h = head(db, request, &b, &original, server).await?;
    #[derive(QueryableByName)]
    struct Applied {
        #[diesel(sql_type=Text)]
        command_id: String,
        #[diesel(sql_type=Text)]
        command_digest: String,
        #[diesel(sql_type=Text)]
        receipt_digest: String,
    }
    let last = sql_query("SELECT c.id AS command_id,c.digest AS command_digest,r.digest AS receipt_digest FROM hagency_agent_v1.management_agent_budget_commands c JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.request_id=$1 AND r.state='applied' ORDER BY r.budget_revision DESC LIMIT 1")
        .bind::<Text,_>(request).get_result::<Applied>(db).await.optional()?
        .ok_or(Error::Conflict("shared_applied_budget_required"))?;
    let pin = crate::management_shared_budget_types::SharedExecutionBudget {
        version: 1,
        original_reserve_receipt_digest: b.receipt_digest,
        budget_revision: h.revision,
        budget_digest: h.digest,
        effective_ceiling_tokens: h.ceiling,
        applied_command_id: last.command_id,
        applied_command_digest: last.command_digest,
        applied_receipt_digest: last.receipt_digest,
    };
    pin.validate()?;
    Ok((pin, h.ledger, h.known))
}

#[derive(QueryableByName)]
struct SettlementSaved { #[diesel(sql_type=Text)] payload:String }
pub(super) struct SettlementBudget {
    pub revision:i64, pub digest:String, pub ceiling:i64, pub pending:bool,
}
/// Read only the original/applied chain. This does not require live issuance or
/// an unexpired grant, and never treats an approved-but-unacknowledged delta as usable.
pub(super) async fn settlement_head(db:&mut AsyncPgConnection,id:&str,server:&str)->Result<Option<SettlementBudget>> {
    let any=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands WHERE request_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
    if !any {
        let r=sql_query("SELECT w.receipt AS payload FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.reserve' AND w.receipt IS NOT NULL").bind::<Text,_>(id).get_result::<SettlementSaved>(db).await.optional()?;
        let Some(r)=r else{return Ok(None)};let r:ManagedAgentReservationReceipt=decode(&r.payload)?;
        if r.state!="reserved"{return Ok(None)}
        return Ok(Some(SettlementBudget{revision:1,digest:allocation_digest(&json!({"version":1,"originalReserveReceiptDigest":allocation_digest(&r)?,"budgetRevision":1,"effectiveCeilingTokens":r.allocated_tokens}))?,ceiling:r.allocated_tokens,pending:false}));
    }
    let (b,_,original)=base(db,id,server).await?;let h=head(db,id,&b,&original,server).await?;
    let missing=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands c WHERE c.request_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results r WHERE r.command_id=c.id)) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
    Ok(Some(SettlementBudget{revision:h.revision,digest:h.digest,ceiling:h.ceiling,pending:missing}))
}
