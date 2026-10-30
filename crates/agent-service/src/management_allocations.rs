//! Central custody of native-ledger reservations. No central write can assert
//! that capacity was reserved without the enrolled executor's exact receipt.
#[path = "management_allocation_ui.rs"]
mod ui;
use super::*;
use crate::management_allocation_types::*;
use crate::secret_token;

#[derive(QueryableByName)]
struct GrantRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    contribution_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    receipt: String,
    #[diesel(sql_type=Text)]
    receipt_digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=BigInt)]
    observed_at_ms: i64,
    #[diesel(sql_type=Text)]
    chain_root_id: String,
    #[diesel(sql_type=BigInt)]
    period_start_ms: i64,
}
#[derive(QueryableByName)]
struct AllocationRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    project_id: String,
    #[diesel(sql_type=BigInt)]
    project_revision: i64,
    #[diesel(sql_type=Text)]
    grant_id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    reserve_command_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    release_command_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    result: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    balance: Option<String>,
}
#[derive(QueryableByName)]
struct PlanRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    contribution_id: String,
    #[diesel(sql_type=Text)]
    manager_user_id: String,
    #[diesel(sql_type=Text)]
    manager_mxid: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=BigInt)]
    requested_tokens: i64,
    #[diesel(sql_type=Text)]
    name: String,
    #[diesel(sql_type=Text)]
    purpose: String,
}
#[derive(QueryableByName)]
struct WorkRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    allocation_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    operation: String,
    #[diesel(sql_type=Text)]
    command: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    receipt: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    receipt_digest: Option<String>,
}
#[derive(QueryableByName)]
struct RevisionRow {
    #[diesel(sql_type=BigInt)]
    revision: i64,
}
#[derive(QueryableByName)]
struct PeriodRow {
    #[diesel(sql_type=Text)]
    key: String,
    #[diesel(sql_type=BigInt)]
    start_ms: i64,
    #[diesel(sql_type=BigInt)]
    end_ms: i64,
}
fn enabled(policy: &ManagementPolicy) -> Result<()> {
    if policy.project_reservations() {
        Ok(())
    } else {
        Err(Error::Unavailable("project_reservations_unavailable"))
    }
}
async fn grant(db: &mut AsyncPgConnection, id: &str) -> Result<GrantRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn allocation(db: &mut AsyncPgConnection, id: &str) -> Result<AllocationRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_allocations WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn plan(db: &mut AsyncPgConnection, id: &str) -> Result<PlanRow> {
    sql_query("SELECT m.id,m.contribution_id,m.manager_user_id,u.mxid AS manager_mxid,m.state,m.revision,m.requested_tokens,m.name,m.purpose FROM hagency_agent_v1.management_projects m JOIN hagency_agent_v1.users u ON u.id=m.manager_user_id WHERE m.id=$1")
        .bind::<Text,_>(id).get_result(db).await.map_err(hidden)
}
async fn work(db: &mut AsyncPgConnection, id: &str) -> Result<WorkRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_executor_commands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn grant_view(row: &GrantRow, now: i64) -> Result<ResourceGrantView> {
    Ok(ResourceGrantView {
        id: row.id.clone(),
        receipt: decode(&row.receipt)?,
        receipt_digest: row.receipt_digest.clone(),
        state: if row.state == "active" && row.expires_at_ms <= now {
            "expired".into()
        } else {
            row.state.clone()
        },
        revision: row.revision,
        observed_at_ms: row.observed_at_ms,
    })
}
async fn grant_live(db: &mut AsyncPgConnection, g: &GrantRow, now: i64) -> Result<bool> {
    let receipt: NativeGrantReceipt = decode(&g.receipt)?;
    let (owner, c) = contribution(db, &receipt.contribution_id).await?;
    // A scheduled renewal authorizes nothing before its UTC month starts.
    if g.state != "active"
        || g.expires_at_ms <= now
        || g.period_start_ms > now
        || receipt.period_start_ms > now
        || receipt.period_end_ms <= now
        || c.state != "admitted"
        || owner != g.owner_user_id
        || c.executor_device_id != g.executor_device_id
        || c.executor_generation != g.executor_generation
        || c.resource_id != receipt.resource_id
        || c.resource_revision != receipt.resource_revision
        || c.coordinator_mxid != receipt.owner_mxid
        || receipt
            .admission_request_id
            .as_ref()
            .is_some_and(|id| id != &c.request_id)
    {
        return Ok(false);
    }
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id WHERE d.id=$1 AND d.user_id=$2 AND d.generation=$3 AND NOT d.revoked AND NOT s.revoked AND u.active AND s.valid_until_ms>$4) AS matched")
        .bind::<Text,_>(&g.executor_device_id).bind::<Text,_>(&g.owner_user_id).bind::<BigInt,_>(g.executor_generation).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
async fn allocation_live(
    db: &mut AsyncPgConnection,
    a: &AllocationRow,
    g: &GrantRow,
    now: i64,
) -> Result<bool> {
    let p = plan(db, &a.project_id).await?;
    let r: NativeGrantReceipt = decode(&g.receipt)?;
    Ok(p.state == "approved"
        && p.revision == a.project_revision
        && a.expires_at_ms > now
        && r.eligible_managers.contains(&p.manager_mxid)
        && grant_live(db, g, now).await?)
}

pub(super) struct SetupCapacity {
    pub id: String,
    pub revision: i64,
    pub receipt_digest: String,
    pub grant: NativeGrantReceipt,
    pub grant_digest: String,
    pub requested_tokens: i64,
}
/// Capacity readiness reuses the exact allocation authority checks; no setup
/// caller can turn an approved intent or a stale receipt into a live reservation.
pub(super) async fn setup_capacity(
    db: &mut AsyncPgConnection,
    project_id: &str,
    pinned: Option<&str>,
    now: i64,
) -> Result<Option<SetupCapacity>> {
    let rows = sql_query("SELECT * FROM hagency_agent_v1.management_allocations WHERE project_id=$1 AND ($2::text IS NULL OR id=$2) AND state='reserved' ORDER BY id")
        .bind::<Text,_>(project_id).bind::<Nullable<Text>,_>(pinned).load::<AllocationRow>(db).await?;
    for a in rows {
        let g = grant(db, &a.grant_id).await?;
        if !allocation_live(db, &a, &g, now).await? || a.release_command_id.is_some() {
            continue;
        }
        let plan = plan(db, project_id).await?;
        let (owner, c) = contribution(db, &plan.contribution_id).await?;
        match executor_live(db, &owner, &c, now).await {
            Ok(()) => {}
            Err(Error::Conflict("contribution_executor_changed")) => continue,
            Err(error) => return Err(error),
        }
        let Some(command) = a.reserve_command_id.as_deref() else {
            continue;
        };
        let w = work(db, command).await?;
        let Some(raw) = w.receipt.as_deref() else {
            continue;
        };
        let receipt: NativeAllocationReceipt = decode(raw)?;
        if receipt.state != "reserved" || receipt.retained_tokens <= 0 {
            continue;
        }
        let Some(receipt_digest) = w.receipt_digest else {
            continue;
        };
        return Ok(Some(SetupCapacity {
            id: a.id,
            revision: a.revision,
            receipt_digest,
            grant: decode(&g.receipt)?,
            grant_digest: g.receipt_digest,
            requested_tokens: receipt.reserved_tokens,
        }));
    }
    Ok(None)
}
async fn blocked(
    db: &mut AsyncPgConnection,
    a: &AllocationRow,
    g: &GrantRow,
    now: i64,
) -> Result<Option<String>> {
    let p = plan(db, &a.project_id).await?;
    let r: NativeGrantReceipt = decode(&g.receipt)?;
    let reason = if p.state != "approved" {
        Some("project_unavailable")
    } else if p.revision != a.project_revision {
        Some("project_revision_changed")
    } else if a.expires_at_ms <= now {
        Some("allocation_expired")
    } else if g.state != "active" || g.expires_at_ms <= now {
        Some("owner_grant_unavailable")
    } else if !r.eligible_managers.contains(&p.manager_mxid) {
        Some("manager_not_eligible")
    } else if r.owner_mxid == p.manager_mxid && !r.allow_self_approval {
        Some("self_approval_forbidden")
    } else if !grant_live(db, g, now).await? {
        Some("executor_unavailable")
    } else {
        None
    };
    Ok(reason.map(str::to_owned))
}
pub(super) async fn actionable(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<bool> {
    if !policy.project_reservations() {
        return Ok(false);
    }
    let a = allocation(db, id).await?;
    let g = grant(db, &a.grant_id).await?;
    let r: NativeGrantReceipt = decode(&g.receipt)?;
    let manager = plan(db, &a.project_id).await?;
    Ok(a.state == "pending_coordinator"
        && p.mxid == r.owner_mxid
        && (r.allow_self_approval || p.mxid != manager.manager_mxid)
        && allocation_live(db, &a, &g, now).await?)
}
async fn view(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ProjectAllocation> {
    let a = allocation(db, id).await?;
    let g = grant(db, &a.grant_id).await?;
    let r: NativeGrantReceipt = decode(&g.receipt)?;
    let manager = plan(db, &a.project_id).await?;
    if p.user_id != manager.manager_user_id
        && p.user_id != g.owner_user_id
        && p.mxid != r.owner_mxid
        && p.mxid != policy.administrator()
    {
        return Err(Error::NotFound("allocation_not_found"));
    }
    let state = if a.state == "pending_coordinator" && a.expires_at_ms <= now {
        "expired".into()
    } else if a.state == "approved_waiting_executor" {
        let w = work(
            db,
            a.reserve_command_id
                .as_deref()
                .ok_or(Error::Unavailable("invalid_allocation_record"))?,
        )
        .await?;
        if (w.state == "issued" && w.lease_until_ms <= now) || a.expires_at_ms <= now {
            "outcome_unknown".into()
        } else {
            a.state.clone()
        }
    } else {
        a.state.clone()
    };
    let known = super::usage::known(db, super::usage::Scope::Allocation(id)).await?;
    let balance = a
        .balance
        .as_deref()
        .map(decode::<NativeAllocationReceipt>)
        .transpose()?;
    Ok(ProjectAllocation {
        id: a.id.clone(),
        project_id: a.project_id.clone(),
        project_revision: a.project_revision,
        grant_id: a.grant_id.clone(),
        requested_tokens: manager.requested_tokens,
        state,
        revision: a.revision,
        request_id: a.request_id.clone(),
        requester_mxid: manager.manager_mxid,
        coordinator_mxid: r.owner_mxid,
        period_key: r.period_key,
        expires_at_ms: a.expires_at_ms,
        can_decide: actionable(db, id, p, policy, now).await?,
        can_release: (p.user_id == manager.manager_user_id
            || p.user_id == g.owner_user_id
            || p.mxid == policy.administrator())
            && !matches!(a.state.as_str(), "released" | "rejected"),
        blocking_reason: if a.state == "pending_coordinator" {
            blocked(db, &a, &g, now).await?
        } else {
            None
        },
        result: a.result.as_deref().map(decode).transpose()?,
        observed_usage: (known > 0).then(|| AllocationObservedUsage {
            known_consumed_tokens: known,
            retained_tokens_floor: balance
                .as_ref()
                .map_or(known, |b| b.retained_tokens.max(known)),
            requires_reconciliation: balance.as_ref().is_none_or(|b| b.retained_tokens < known),
        }),
        balance,
    })
}

/// The pool's effective grant at `now`: the one active receipt of the
/// contribution whose window covers `now` (a v2 root or a v3 successor).
pub(super) async fn live_grant_for_pool(
    db: &mut AsyncPgConnection,
    contribution: &str,
    now: i64,
) -> Result<Option<ResourceGrantView>> {
    let rows = sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND state='active' AND period_start_ms<=$2 AND expires_at_ms>$2 ORDER BY id DESC LIMIT 2")
        .bind::<Text,_>(contribution).bind::<BigInt,_>(now).load::<GrantRow>(db).await?;
    let mut current = None;
    for row in rows {
        if grant_live(db, &row, now).await? {
            let view = grant_view(&row, now)?;
            if !matches!(view.receipt.version, 2 | 3) { continue; }
            if current.is_some() { return Err(Error::Conflict("resource_grant_conflict")); }
            current = Some(view);
        }
    }
    Ok(current)
}
/// `ancestor` is `grant` itself or one of its predecessors.
pub(super) async fn chain_contains(
    db: &mut AsyncPgConnection,
    ancestor: &str,
    grant: &str,
) -> Result<bool> {
    Ok(sql_query("WITH RECURSIVE up(id,predecessor,depth) AS (SELECT id,predecessor_grant_id,0 FROM hagency_agent_v1.management_resource_grants WHERE id=$2 UNION ALL SELECT g.id,g.predecessor_grant_id,up.depth+1 FROM hagency_agent_v1.management_resource_grants g JOIN up ON g.id=up.predecessor WHERE up.depth<4096) SELECT EXISTS(SELECT 1 FROM up WHERE id=$1) AS matched")
        .bind::<Text,_>(ancestor).bind::<Text,_>(grant).get_result::<Flag>(db).await?.matched)
}

pub(super) async fn seed_for_project(
    db: &mut AsyncPgConnection,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    if !policy.project_reservations() {
        return Ok(());
    }
    let p = plan(db, id).await?;
    if p.state != "approved" {
        return Ok(());
    }
    let grants=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND state='active' AND expires_at_ms>$2 ORDER BY id LIMIT 2")
        .bind::<Text,_>(&p.contribution_id).bind::<BigInt,_>(now).load::<GrantRow>(db).await?;
    for g in grants {
        let r: NativeGrantReceipt = decode(&g.receipt)?;
        // Successor grants carry pool Agents only, never project allocations.
        if r.succession.is_some()
            || !r.eligible_managers.contains(&p.manager_mxid)
            || !grant_live(db, &g, now).await?
        {
            continue;
        }
        if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_allocations WHERE project_id=$1 AND grant_id=$2) AS matched").bind::<Text,_>(id).bind::<Text,_>(&g.id).get_result::<Flag>(db).await?.matched {continue;}
        let allocation_id = format!("mal_{}", entity_id()?);
        let request_id = format!("req_{}", entity_id()?);
        let expires = (now + TTL).min(g.expires_at_ms).min(r.period_end_ms);
        sql_query("INSERT INTO hagency_agent_v1.management_allocations(id,project_id,project_revision,grant_id,request_id,state,expires_at_ms) VALUES($1,$2,$3,$4,$5,'pending_coordinator',$6)")
            .bind::<Text,_>(&allocation_id).bind::<Text,_>(id).bind::<BigInt,_>(p.revision).bind::<Text,_>(&g.id).bind::<Text,_>(&request_id).bind::<BigInt,_>(expires).execute(db).await?;
        sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,project_id,allocation_id,requester_user_id,approver_mxid,state,created_at_ms,expires_at_ms) VALUES($1,'project_allocation',$2,$3,$4,$5,$6,'pending',$7,$8)")
            .bind::<Text,_>(&request_id).bind::<Text,_>(&p.contribution_id).bind::<Text,_>(id).bind::<Text,_>(&allocation_id).bind::<Text,_>(&p.manager_user_id).bind::<Text,_>(&r.owner_mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(expires).execute(db).await?;
        // Outbox contains only action references, never the native authority receipt.
        let notification = ManagementRequest {
            id: request_id,
            kind: "project_allocation".into(),
            contribution_id: p.contribution_id.clone(),
            project_id: Some(id.into()),
            allocation_id: Some(allocation_id),
            project_name: None,
            resource_label: None,
            requester_mxid: p.manager_mxid.clone(),
            approver_mxid: r.owner_mxid.clone(),
            state: "pending".into(),
            revision: 1,
            created_at_ms: now,
            expires_at_ms: expires,
            decided_at_ms: None,
            can_decide: false,
        };
        notify(db, &notification, [r.owner_mxid], now).await?;
    }
    Ok(())
}
async fn insert_command(
    db: &mut AsyncPgConnection,
    a: &AllocationRow,
    g: &GrantRow,
    actor: &str,
    operation: &str,
    now: i64,
) -> Result<String> {
    let p = plan(db, &a.project_id).await?;
    let receipt: NativeGrantReceipt = decode(&g.receipt)?;
    let id = format!("mex_{}", entity_id()?);
    let project_definition_digest = allocation_digest(
        &serde_json::json!({"projectId":p.id,"projectRevision":a.project_revision,"managerMxid":p.manager_mxid,"name":p.name,"purpose":p.purpose,"requestedTokens":p.requested_tokens}),
    )?;
    let command = AllocationCommand {
        version: 1,
        command_id: id.clone(),
        operation: operation.into(),
        allocation_id: a.id.clone(),
        project_id: a.project_id.clone(),
        project_revision: a.project_revision,
        request_id: a.request_id.clone(),
        request_revision: sql_query(
            "SELECT revision FROM hagency_agent_v1.management_requests WHERE id=$1",
        )
        .bind::<Text, _>(&a.request_id)
        .get_result::<RevisionRow>(db)
        .await?
        .revision
            + 1,
        decision_actor: actor.into(),
        manager_mxid: p.manager_mxid,
        project_definition_digest,
        grant_receipt: receipt,
        requested_tokens: p.requested_tokens,
        reserve_command_id: a.reserve_command_id.clone(),
        issued_at_ms: now,
        expires_at_ms: if operation == "project.reserve" {
            a.expires_at_ms
        } else {
            MAX_EXACT
        },
    };
    let digest = allocation_digest(&command)?;
    sql_query("INSERT INTO hagency_agent_v1.management_executor_commands(id,allocation_id,owner_user_id,executor_device_id,executor_generation,operation,command,digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind::<Text,_>(&id).bind::<Text,_>(&a.id).bind::<Text,_>(&g.owner_user_id).bind::<Text,_>(&g.executor_device_id).bind::<BigInt,_>(g.executor_generation).bind::<Text,_>(operation).bind::<Text,_>(wire(&command)?).bind::<Text,_>(&digest).execute(db).await?;
    Ok(id)
}
async fn release(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    let a = allocation(db, id).await?;
    if matches!(a.state.as_str(), "released" | "rejected")
        || a.release_command_id.is_some() && a.state != "release_blocked"
    {
        return Ok(());
    }
    let g = grant(db, &a.grant_id).await?;
    let release_id = if a.reserve_command_id.is_some() {
        Some(insert_command(db, &a, &g, &p.mxid, "project.release", now).await?)
    } else {
        None
    };
    sql_query("UPDATE hagency_agent_v1.management_allocations SET state=$2,release_command_id=$3,revision=revision+1 WHERE id=$1")
        .bind::<Text,_>(id).bind::<Text,_>(if release_id.is_some(){"releasing"}else{"released"}).bind::<Nullable<Text>,_>(release_id).execute(db).await?;
    sql_query("UPDATE hagency_agent_v1.management_requests SET state='revoked',revision=revision+1,decided_at_ms=$2 WHERE id=$1").bind::<Text,_>(&a.request_id).bind::<BigInt,_>(now).execute(db).await?;
    sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'").bind::<Text,_>(&a.request_id).execute(db).await?;
    let r = request(db, &a.request_id, p, policy, now).await?;
    let manager = plan(db, &a.project_id).await?;
    let receipt: NativeGrantReceipt = decode(&g.receipt)?;
    notify(
        db,
        &r,
        [
            manager.manager_mxid,
            receipt.owner_mxid.clone(),
            receipt.owner_mxid,
        ],
        now,
    )
    .await?;
    Ok(())
}
pub(super) async fn release_for_project(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    let rows=sql_query("SELECT * FROM hagency_agent_v1.management_allocations WHERE project_id=$1 AND state NOT IN ('released','rejected') ORDER BY id")
        .bind::<Text,_>(id).load::<AllocationRow>(db).await?;
    for a in rows {
        release(db, p, policy, &a.id, now).await?;
    }
    Ok(())
}
#[derive(QueryableByName)]
struct ChainLink {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=BigInt)]
    period_end_ms: i64,
}
/// End of the contiguous run of active grants of `effective`'s chain: a
/// renewal that starts exactly when the current grant's month ends extends it.
pub(super) async fn chain_until(db: &mut AsyncPgConnection, effective: &str) -> Result<i64> {
    const LINK: &str = "SELECT id,expires_at_ms,(receipt::jsonb->>'periodEndMs')::bigint AS period_end_ms FROM hagency_agent_v1.management_resource_grants";
    let mut current = sql_query(format!("{LINK} WHERE id=$1")).bind::<Text,_>(effective).get_result::<ChainLink>(db).await.map_err(hidden)?;
    let mut until = current.expires_at_ms;
    for _ in 0..24 {
        if current.expires_at_ms != current.period_end_ms {
            break;
        }
        let next = sql_query(format!("{LINK} WHERE predecessor_grant_id=$1 AND state='active' AND succession='renewal' AND period_start_ms=$2"))
            .bind::<Text,_>(&current.id).bind::<BigInt,_>(current.period_end_ms).get_result::<ChainLink>(db).await.optional()?;
        let Some(next) = next else { break };
        until = next.expires_at_ms;
        current = next;
    }
    Ok(until)
}
/// The active renewal of `grant` that has not started yet, if any.
pub(super) async fn scheduled_renewal(
    db: &mut AsyncPgConnection,
    grant: &str,
    now: i64,
) -> Result<Option<ResourceGrantView>> {
    let row = sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE predecessor_grant_id=$1 AND state='active' AND succession='renewal' AND period_start_ms>$2")
        .bind::<Text,_>(grant).bind::<BigInt,_>(now).get_result::<GrantRow>(db).await.optional()?;
    row.map(|row| grant_view(&row, now)).transpose()
}
pub(super) async fn retire_grant(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1 WHERE id=$1 AND state='active'").bind::<Text,_>(id).execute(db).await?;
    let rows = sql_query(
        "SELECT * FROM hagency_agent_v1.management_allocations WHERE grant_id=$1 ORDER BY id",
    )
    .bind::<Text, _>(id)
    .load::<AllocationRow>(db)
    .await?;
    for a in rows {
        release(db, p, policy, &a.id, now).await?;
    }
    Ok(())
}
/// Withdraw `id` and every still-active grant that succeeds it. Returns the
/// successors withdrawn with it.
pub(super) async fn retire_chain_from(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<Vec<String>> {
    let successors = active_descendants(db, id).await?;
    retire_grant(db, p, policy, id, now).await?;
    for successor in &successors {
        retire_grant(db, p, policy, successor, now).await?;
    }
    Ok(successors)
}
/// Every still-active grant that succeeds `id` in its chain. A successor
/// depends on its predecessor's receipt, so withdrawing the predecessor also
/// withdraws them (in particular a scheduled renewal).
pub(super) async fn active_descendants(db: &mut AsyncPgConnection, id: &str) -> Result<Vec<String>> {
    #[derive(QueryableByName)]
    struct Id {
        #[diesel(sql_type=Text)]
        id: String,
    }
    Ok(sql_query("WITH RECURSIVE down(id,state,depth) AS (SELECT id,state,0 FROM hagency_agent_v1.management_resource_grants WHERE predecessor_grant_id=$1 UNION ALL SELECT g.id,g.state,down.depth+1 FROM hagency_agent_v1.management_resource_grants g JOIN down ON g.predecessor_grant_id=down.id WHERE down.depth<4096) SELECT id FROM down WHERE state='active' ORDER BY id")
        .bind::<Text,_>(id).load::<Id>(db).await?.into_iter().map(|row| row.id).collect())
}
pub(super) async fn revoke_for_contribution(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    sql_query("UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1 WHERE contribution_id=$1 AND state='active'").bind::<Text,_>(id).execute(db).await?;
    let rows=sql_query("SELECT a.* FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_projects m ON m.id=a.project_id WHERE m.contribution_id=$1 AND a.state NOT IN ('released','rejected') ORDER BY a.id")
        .bind::<Text,_>(id).load::<AllocationRow>(db).await?;
    for a in rows {
        release(db, p, policy, &a.id, now).await?;
    }
    Ok(())
}

impl DomainStore {
    pub async fn management_allocation_supported(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
    ) -> Result<bool> {
        if !policy.project_reservations() {
            return Ok(false);
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            // Keep recovery/history discoverable after revocation. This is protocol
            // support, not permission to make a new reservation.
            if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id JOIN hagency_agent_v1.management_projects m ON m.id=a.project_id WHERE g.owner_user_id=$1 OR m.manager_user_id=$1 OR g.receipt::jsonb->>'ownerMxid'=$2 OR $2=$3) AS matched").bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).get_result::<Flag>(db).await?.matched{return Ok(true);}
            let grants=sql_query("SELECT g.* FROM hagency_agent_v1.management_resource_grants g WHERE g.state='active' AND g.expires_at_ms>$1 AND (g.owner_user_id=$2 OR g.receipt::jsonb->>'ownerMxid'=$3 OR (g.receipt::jsonb->'eligibleManagers') ? $3 OR $3=$4) ORDER BY g.id")
                .bind::<BigInt,_>(now).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).load::<GrantRow>(db).await?;
            for grant in grants {if grant_live(db,&grant,now).await? {return Ok(true);}}
            Ok(false)
        }).await
    }
    pub async fn management_resource_grant(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ResourceGrantView> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let g = grant(db, id).await?;
                let r: NativeGrantReceipt = decode(&g.receipt)?;
                if p.user_id != g.owner_user_id
                    && p.mxid != r.owner_mxid
                    && p.mxid != policy.administrator()
                {
                    return Err(Error::NotFound("resource_grant_not_found"));
                }
                grant_view(&g, now)
            })
            .await
    }
    pub async fn recover_management_allocation_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<serde_json::Value> {
        enabled(policy)?;
        key(id)?;
        let (executor, _) = device(p)?;
        if !matches!(
            input.operation.as_str(),
            "grant.publish"
                | "grant.succeed"
                | "grant.revoke"
                | "allocation.decide"
                | "allocation.release"
        ) {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(id).load::<Receipt>(db).await?;
            if rows.len()>1{return Err(Error::Conflict("management_command_conflict"));}
            let Some(row)=rows.as_slice().first()else{return Ok(serde_json::json!({"recorded":false}))};
            if row.operation!=input.operation{return Err(Error::Conflict("management_command_conflict"));}
            if input.operation.starts_with("grant.") {
                let saved:ResourceGrantView=decode(&row.result)?;let g=grant(db,&saved.id).await?;
                if g.owner_user_id!=p.user_id{return Err(Error::NotFound("resource_grant_not_found"));}
                Ok(serde_json::json!({"recorded":true,"grant":grant_view(&g,now)?}))
            } else {let saved:ProjectAllocation=decode(&row.result)?;Ok(serde_json::json!({"recorded":true,"allocation":view(db,&saved.id,p,policy,now).await?}))}
        }).await
    }
    pub async fn publish_management_grant(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: PublishGrantInput,
    ) -> Result<ResourceGrantView> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        input.receipt.validate(&self.server)?;
        // A successor names its predecessor and is published only through it.
        if input.receipt.succession.is_some() {
            return Err(Error::Invalid("invalid_grant_succession"));
        }
        let (executor, generation) = device(p)?;
        if input.expected_contribution_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let receipt_digest = allocation_digest(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let (owner,c)=contribution(db,id).await?;
            if owner!=p.user_id || executor!=c.executor_device_id || generation!=c.executor_generation{return Err(Error::Unauthorized("contribution_executor_required"));}
            if let Some(saved)=replay::<ResourceGrantView>(db,p,&input.command_id,"grant.publish",&payload_digest).await? {return grant_view(&grant(db,&saved.id).await?,now);}
            let r=&input.receipt;
            if c.state!="admitted" {return Err(Error::Conflict("contribution_not_admitted"));}
            if c.revision!=input.expected_contribution_revision{return Err(Error::Conflict("management_revision_changed"));}
            if r.contribution_id!=id || r.owner_mxid!=p.mxid || r.executor_device_id!=executor || r.executor_generation!=generation || r.resource_id!=c.resource_id || r.resource_revision!=c.resource_revision || r.coordinator_mxid!=p.mxid || c.coordinator_mxid!=p.mxid{return Err(Error::Invalid("resource_grant_binding_mismatch"));}
            if r.admission_request_id.as_ref().is_some_and(|request| request!=&c.request_id) {return Err(Error::Invalid("resource_grant_binding_mismatch"));}
            executor_live(db,&owner,&c,now).await?;
            let period=sql_query("SELECT to_char(to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC','YYYY-MM') AS key,(extract(epoch FROM (date_trunc('month',to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))*1000)::bigint AS start_ms,(extract(epoch FROM ((date_trunc('month',to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC'))*1000)::bigint AS end_ms").bind::<BigInt,_>(now).get_result::<PeriodRow>(db).await?;
            if r.period_key!=period.key || r.period_start_ms!=period.start_ms || r.period_end_ms!=period.end_ms || r.delegation_expires_at_ms<=now || r.delegation_expires_at_ms>now+366*86400000 {return Err(Error::Invalid("invalid_grant_period"));}
            let existing=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&r.grant_id).get_result::<GrantRow>(db).await.optional()?;
            if let Some(existing)=existing {
                if existing.owner_user_id!=p.user_id || existing.receipt_digest!=receipt_digest {return Err(Error::Conflict("resource_grant_conflict"));}
                let result=grant_view(&existing,now)?;receipt(db,p,&input.command_id,"grant.publish",&payload_digest,&result).await?;return Ok(result);
            }
            let duplicate=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND period_key=$2 AND state='active') AS matched").bind::<Text,_>(id).bind::<Text,_>(&r.period_key).get_result::<Flag>(db).await?.matched;
            if duplicate{return Err(Error::Conflict("resource_grant_already_exists"));}
            // authorize() holds the domain advisory transaction lock across
            // this check and insertion. One grant per admission per UTC month:
            // revocation cannot permit a second hold in the same month, while a
            // month that has ended (the period check above) can be granted again.
            // A renewal cancelled before any pool request or intent was bound to
            // it never authorized anything, so it does not use up its month.
            if let Some(request)=&r.admission_request_id
                && sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants g WHERE g.contribution_id=$1 AND g.receipt::jsonb->>'admissionRequestId'=$2 AND g.receipt::jsonb->>'periodKey'=$3 AND NOT (g.state='revoked' AND g.succession IS NOT DISTINCT FROM 'renewal' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests q WHERE q.pool_grant_id=g.id) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents i WHERE i.payload::jsonb->'poolGrant'->>'grantId'=g.id))) AS matched").bind::<Text,_>(id).bind::<Text,_>(request).bind::<Text,_>(&r.period_key).get_result::<Flag>(db).await?.matched
            {return Err(Error::Conflict("resource_grant_already_exists"));}
            super::agents::executor::validate_external_ledger_receipt(db,&p.user_id,executor,r.ledger_revision,r.ledger_revision).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms,chain_root_id,period_start_ms) VALUES($1,$2,$3,$4,$5,$6,$7,'active',$8,$9,$10,$1,$11)")
                .bind::<Text,_>(&r.grant_id).bind::<Text,_>(id).bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(generation).bind::<Text,_>(wire(r)?).bind::<Text,_>(&receipt_digest).bind::<Text,_>(&r.period_key).bind::<BigInt,_>(r.delegation_expires_at_ms.min(r.period_end_ms)).bind::<BigInt,_>(now).bind::<BigInt,_>(r.period_start_ms).execute(db).await?;
            let rows=sql_query("SELECT m.id,m.contribution_id,m.manager_user_id,u.mxid AS manager_mxid,m.state,m.revision,m.requested_tokens,m.name,m.purpose FROM hagency_agent_v1.management_projects m JOIN hagency_agent_v1.users u ON u.id=m.manager_user_id WHERE m.contribution_id=$1 AND m.state='approved' ORDER BY m.id").bind::<Text,_>(id).load::<PlanRow>(db).await?;
            for m in rows {seed_for_project(db,policy,&m.id,now).await?;}
            let result=grant_view(&grant(db,&r.grant_id).await?,now)?;
            Self::audit(db,p,"grant.publish",&r.grant_id,now).await?;receipt(db,p,&input.command_id,"grant.publish",&payload_digest,&result).await?;Ok(result)
        }).await
    }

    /// Publish an owner-confirmed v3 successor of the pool's effective grant.
    /// An amendment supersedes its predecessor in this same transaction, so
    /// exactly one grant of the chain is effective before and after (N1, N2).
    /// A renewal is scheduled for the next UTC month during the last days of
    /// the current one. Native custody already holds the successor's capacity.
    pub async fn publish_management_grant_successor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SucceedGrantInput,
    ) -> Result<ResourceGrantView> {
        enabled(policy)?;
        if !policy.pool_grant_succession() {
            return Err(Error::Unavailable("pool_grant_succession_unavailable"));
        }
        key(id)?;
        key(&input.command_id)?;
        input.receipt.validate(&self.server)?;
        let succession = input
            .receipt
            .succession
            .clone()
            .filter(|s| input.receipt.version == 3 && s.predecessor_grant_id == id)
            .ok_or(Error::Invalid("invalid_grant_succession"))?;
        let (executor, generation) = device(p)?;
        if input.expected_contribution_revision <= 0 || input.expected_predecessor_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let receipt_digest = allocation_digest(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let pred=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(id).get_result::<GrantRow>(db).await.optional()?
                .filter(|g| g.owner_user_id==p.user_id).ok_or(Error::NotFound("resource_grant_not_found"))?;
            let pr:NativeGrantReceipt=decode(&pred.receipt)?;
            let (owner,c)=contribution(db,&pred.contribution_id).await?;
            if owner!=p.user_id || executor!=c.executor_device_id || generation!=c.executor_generation{return Err(Error::Unauthorized("contribution_executor_required"));}
            if let Some(saved)=replay::<ResourceGrantView>(db,p,&input.command_id,"grant.succeed",&payload_digest).await? {return grant_view(&grant(db,&saved.id).await?,now);}
            let r=&input.receipt;
            if c.state!="admitted" {return Err(Error::Conflict("contribution_not_admitted"));}
            if c.revision!=input.expected_contribution_revision || pred.revision!=input.expected_predecessor_revision {return Err(Error::Conflict("management_revision_changed"));}
            // Same pool, executor, owner, admission and policy as the predecessor.
            if r.contribution_id!=pred.contribution_id || r.contribution_id!=c.id || r.capability!=pr.capability
                || r.owner_mxid!=p.mxid || r.owner_mxid!=pr.owner_mxid
                || r.executor_device_id!=executor || r.executor_device_id!=pr.executor_device_id
                || r.executor_generation!=generation || r.executor_generation!=pr.executor_generation
                || r.resource_id!=c.resource_id || r.resource_id!=pr.resource_id
                || r.resource_revision!=c.resource_revision || r.resource_revision!=pr.resource_revision
                || r.coordinator_mxid!=p.mxid || c.coordinator_mxid!=p.mxid || r.coordinator_mxid!=pr.coordinator_mxid
                || r.enrollment_id!=pr.enrollment_id || r.allow_self_approval!=pr.allow_self_approval
                || r.admission_request_id.is_none() || r.admission_request_id!=pr.admission_request_id
                || r.admission_request_id.as_deref()!=Some(c.request_id.as_str())
            {return Err(Error::Invalid("resource_grant_binding_mismatch"));}
            // Managers may be added, never removed: removal would silently stop
            // that manager's carried Agents.
            if !pr.eligible_managers.iter().all(|m| r.eligible_managers.contains(m))
                || r.grant_revision!=pr.grant_revision+1
                || succession.predecessor_receipt_digest!=pred.receipt_digest
                || r.delegation_expires_at_ms<=now.max(r.period_start_ms)
                || r.delegation_expires_at_ms>now+366*86400000
                || r.ledger_revision<=pr.ledger_revision
            {return Err(Error::Invalid("invalid_grant_succession"));}
            executor_live(db,&owner,&c,now).await?;
            let existing=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&r.grant_id).get_result::<GrantRow>(db).await.optional()?;
            if let Some(existing)=existing {
                if existing.owner_user_id!=p.user_id || existing.receipt_digest!=receipt_digest {return Err(Error::Conflict("resource_grant_conflict"));}
                let result=grant_view(&existing,now)?;receipt(db,p,&input.command_id,"grant.succeed",&payload_digest,&result).await?;return Ok(result);
            }
            if pred.state!="active" || !grant_live(db,&pred,now).await? {return Err(Error::Conflict("grant_predecessor_not_effective"));}
            // One live successor per grant. While a renewal is scheduled the
            // owner cancels (revokes) it before amending this month (Q7).
            if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE predecessor_grant_id=$1 AND state<>'revoked') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched
            {return Err(Error::Conflict("grant_successor_exists"));}
            if succession.renewal() {
                let next=sql_query("SELECT to_char(to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC','YYYY-MM') AS key,(extract(epoch FROM (date_trunc('month',to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))*1000)::bigint AS start_ms,(extract(epoch FROM ((date_trunc('month',to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC'))*1000)::bigint AS end_ms").bind::<BigInt,_>(pr.period_end_ms).get_result::<PeriodRow>(db).await?;
                if r.period_start_ms!=pr.period_end_ms || r.period_key!=next.key || r.period_start_ms!=next.start_ms || r.period_end_ms!=next.end_ms {return Err(Error::Invalid("invalid_grant_period"));}
                if now<pr.period_end_ms-RENEWAL_WINDOW_MS || now>=pr.period_end_ms {return Err(Error::Conflict("grant_succession_window_closed"));}
            } else if r.period_key!=pr.period_key || r.period_start_ms!=pr.period_start_ms || r.period_end_ms!=pr.period_end_ms {
                return Err(Error::Invalid("invalid_grant_period"));
            }
            if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND period_key=$2 AND state='active' AND id<>$3) AS matched").bind::<Text,_>(&pred.contribution_id).bind::<Text,_>(&r.period_key).bind::<Text,_>(id).get_result::<Flag>(db).await?.matched
            {return Err(Error::Conflict("resource_grant_already_exists"));}
            // The native ledger is the authority for the floor; the server
            // still refuses to publish below what it knows the chain holds.
            let reserved=super::agents::chain_reserved_total(db,&pred.chain_root_id,&self.server).await?;
            if r.allocated_tokens<reserved {return Err(Error::Conflict("grant_successor_below_reserved"));}
            if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3) AS matched").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(r.ledger_revision).get_result::<Flag>(db).await?.matched
            {return Err(Error::Conflict("agent_executor_ledger_revision_changed"));}
            super::agents::executor::validate_external_ledger_receipt(db,&p.user_id,executor,r.ledger_revision,r.ledger_revision).await?;
            if !succession.renewal() {
                // Supersede first: the (contribution, month) active index admits one.
                let changed=sql_query("UPDATE hagency_agent_v1.management_resource_grants SET state='superseded',revision=revision+1 WHERE id=$1 AND state='active'").bind::<Text,_>(id).execute(db).await?;
                if changed!=1 {return Err(Error::Conflict("grant_predecessor_not_effective"));}
            }
            sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms,chain_root_id,predecessor_grant_id,succession,period_start_ms) VALUES($1,$2,$3,$4,$5,$6,$7,'active',$8,$9,$10,$11,$12,$13,$14)")
                .bind::<Text,_>(&r.grant_id).bind::<Text,_>(&pred.contribution_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(generation)
                .bind::<Text,_>(wire(r)?).bind::<Text,_>(&receipt_digest).bind::<Text,_>(&r.period_key).bind::<BigInt,_>(r.delegation_expires_at_ms.min(r.period_end_ms)).bind::<BigInt,_>(now)
                .bind::<Text,_>(&pred.chain_root_id).bind::<Text,_>(id).bind::<Text,_>(&succession.kind).bind::<BigInt,_>(r.period_start_ms).execute(db).await?;
            let result=grant_view(&grant(db,&r.grant_id).await?,now)?;
            Self::audit(db,p,"grant.succeed",&r.grant_id,now).await?;receipt(db,p,&input.command_id,"grant.succeed",&payload_digest,&result).await?;Ok(result)
        }).await
    }

    pub async fn management_project_allocation(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<Option<ProjectAllocation>> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            // Validate project visibility even when no grant/allocation exists.
            super::projects::project(db,id,p,policy,now).await?;
            let a=sql_query("SELECT * FROM hagency_agent_v1.management_allocations WHERE project_id=$1 ORDER BY id DESC LIMIT 1").bind::<Text,_>(id).get_result::<AllocationRow>(db).await.optional()?;
            match a {Some(a)=>Ok(Some(view(db,&a.id,p,policy,now).await?)),None=>Ok(None)}
        }).await
    }

    pub async fn decide_management_allocation(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        request_id: &str,
        input: DecisionInput,
    ) -> Result<ProjectAllocation> {
        enabled(policy)?;
        key(request_id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(request_id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if let Some(saved)=replay::<ProjectAllocation>(db,p,&input.command_id,"allocation.decide",&payload_digest).await? {return view(db,&saved.id,p,policy,now).await;}
            let a=sql_query("SELECT * FROM hagency_agent_v1.management_allocations WHERE request_id=$1").bind::<Text,_>(request_id).get_result::<AllocationRow>(db).await.map_err(hidden)?;
            let g=grant(db,&a.grant_id).await?;let r:NativeGrantReceipt=decode(&g.receipt)?;
            if p.user_id!=g.owner_user_id{return Err(Error::Unauthorized("allocation_coordinator_required"));}
            let initial_request=request(db,request_id,p,policy,now).await?;
            if initial_request.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
            if !actionable(db,&a.id,p,policy,now).await?{return Err(Error::Conflict("management_action_closed"));}
            let approving=matches!(input.decision,Decision::Approve);
            let m=plan(db,&a.project_id).await?;
            if approving && m.requested_tokens>r.allocated_tokens{return Err(Error::Conflict("allocation_exceeds_grant"));}
            let command=if approving {Some(insert_command(db,&a,&g,&p.mxid,"project.reserve",now).await?)}else{None};
            sql_query("UPDATE hagency_agent_v1.management_allocations SET state=$2,reserve_command_id=$3,revision=revision+1 WHERE id=$1").bind::<Text,_>(&a.id).bind::<Text,_>(if approving{"approved_waiting_executor"}else{"rejected"}).bind::<Nullable<Text>,_>(command).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_requests SET state=$2,revision=revision+1,decided_at_ms=$3 WHERE id=$1").bind::<Text,_>(request_id).bind::<Text,_>(if approving{"approved"}else{"rejected"}).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'").bind::<Text,_>(request_id).execute(db).await?;
            let current=request(db,request_id,p,policy,now).await?;notify(db,&current,[m.manager_mxid,r.owner_mxid],now).await?;
            let result=view(db,&a.id,p,policy,now).await?;Self::audit(db,p,"allocation.decide",request_id,now).await?;receipt(db,p,&input.command_id,"allocation.decide",&payload_digest,&result).await?;Ok(result)
        }).await
    }

    pub async fn release_management_allocation(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<ProjectAllocation> {
        enabled(policy)?;
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
                let a = allocation(db, id).await?;
                let g = grant(db, &a.grant_id).await?;
                let m = plan(db, &a.project_id).await?;
                if p.user_id != g.owner_user_id
                    && p.user_id != m.manager_user_id
                    && p.mxid != policy.administrator()
                {
                    return Err(Error::Unauthorized("allocation_release_forbidden"));
                }
                if let Some(saved) = replay::<ProjectAllocation>(
                    db,
                    p,
                    &input.command_id,
                    "allocation.release",
                    &payload_digest,
                )
                .await?
                {
                    return view(db, &saved.id, p, policy, now).await;
                }
                if a.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                release(db, p, policy, id, now).await?;
                let result = view(db, id, p, policy, now).await?;
                Self::audit(db, p, "allocation.release", id, now).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "allocation.release",
                    &payload_digest,
                    &result,
                )
                .await?;
                Ok(result)
            })
            .await
    }

    pub async fn revoke_management_grant(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<ResourceGrantView> {
        enabled(policy)?;
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
                let g = grant(db, id).await?;
                if p.user_id != g.owner_user_id {
                    return Err(Error::Unauthorized("contribution_owner_required"));
                }
                if let Some(saved) = replay::<ResourceGrantView>(
                    db,
                    p,
                    &input.command_id,
                    "grant.revoke",
                    &payload_digest,
                )
                .await?
                {
                    return grant_view(&grant(db, &saved.id).await?, now);
                }
                if g.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                for successor in retire_chain_from(db, p, policy, id, now).await? {
                    super::parents::enqueue_close(db, p, policy, &successor, "owner_withdrawal", now).await?;
                }
                // A superseded grant's capacity moved to its successor; it is
                // withdrawn through that chain and never gets its own close.
                if grant(db, id).await?.state != "superseded" {
                    super::parents::enqueue_close(db, p, policy, id, "owner_withdrawal", now).await?;
                }
                let result = grant_view(&grant(db, id).await?, now)?;
                Self::audit(db, p, "grant.revoke", id, now).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "grant.revoke",
                    &payload_digest,
                    &result,
                )
                .await?;
                Ok(result)
            })
            .await
    }
}

async fn delivery(
    db: &mut AsyncPgConnection,
    p: &Principal,
    w: &WorkRow,
    now: i64,
    historical: bool,
) -> Result<ExecutorDelivery> {
    let (executor, generation) = device(p)?;
    if w.owner_user_id != p.user_id || w.executor_device_id != executor {
        return Err(Error::NotFound("executor_command_not_found"));
    }
    let a = allocation(db, &w.allocation_id).await?;
    let g = grant(db, &a.grant_id).await?;
    let allowed = !historical
        && w.receipt.is_none()
        && (w.operation == "project.release"
            || generation == w.executor_generation
                && matches!(
                    a.state.as_str(),
                    "approved_waiting_executor" | "outcome_unknown"
                )
                && allocation_live(db, &a, &g, now).await?);
    let token = secret_token();
    let until = now + 30000;
    sql_query("UPDATE hagency_agent_v1.management_executor_commands SET state=CASE WHEN state='pending' THEN 'issued' ELSE state END,lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(&w.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).execute(db).await?;
    Ok(ExecutorDelivery {
        command: decode(&w.command)?,
        command_digest: w.digest.clone(),
        token,
        lease_until_ms: until,
        execution_allowed: allowed,
        recorded: w.receipt.as_deref().map(decode).transpose()?,
    })
}
impl DomainStore {
    pub async fn poll_management_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<ExecutorDelivery>> {
        enabled(policy)?;
        let (executor, _) = device(p)?;
        if let Some(after) = &input.after {
            key(after)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let w=sql_query("SELECT * FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND state<>'recorded' AND lease_until_ms<=$3 ORDER BY (id<=$4),id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(now).bind::<Text,_>(input.after.as_deref().unwrap_or("")).get_result::<WorkRow>(db).await.optional()?;
            match w {Some(w)=>Ok(Some(delivery(db,p,&w,now,false).await?)),None=>Ok(None)}
        }).await
    }
    pub async fn recover_management_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ExecutorRecoverInput,
    ) -> Result<ExecutorDelivery> {
        enabled(policy)?;
        key(id)?;
        device(p)?;
        if !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_command_digest"));
        }
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let w = work(db, id).await?;
                if w.owner_user_id != p.user_id
                    || Some(w.executor_device_id.as_str()) != p.device_id.as_deref()
                {
                    return Err(Error::NotFound("executor_command_not_found"));
                }
                if w.digest != input.command_digest {
                    return Err(Error::Conflict("executor_command_conflict"));
                }
                delivery(db, p, &w, now, true).await
            })
            .await
    }
    pub async fn record_management_executor_result(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: AllocationResultInput,
    ) -> Result<AllocationAcknowledgement> {
        enabled(policy)?;
        key(id)?;
        let (executor, _) = device(p)?;
        if input.token.len() > 512 || !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_executor_result"));
        }
        let result_digest = allocation_digest(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let w=work(db,id).await?;
            if w.owner_user_id!=p.user_id || w.executor_device_id!=executor{return Err(Error::NotFound("executor_command_not_found"));}
            if input.command_digest!=w.digest{return Err(Error::Conflict("executor_command_conflict"));}
            if let Some(previous)=&w.receipt_digest {
                if previous!=&result_digest{return Err(Error::Conflict("executor_result_conflict"));}
                return Ok(AllocationAcknowledgement{command_id:id.into(),command_digest:w.digest,receipt_digest:result_digest,allocation:view(db,&w.allocation_id,p,policy,now).await?});
            }
            if w.lease_until_ms<=now || w.lease_hash.as_deref()!=Some(hash(&input.token).as_str()){return Err(Error::Unauthorized("executor_result_ticket_required"));}
            let command:AllocationCommand=decode(&w.command)?;let r=&input.receipt;
            let valid_state=match(w.operation.as_str(),r.state.as_str()) {("project.reserve","reserved")=>r.reserved_tokens==command.requested_tokens && r.retained_tokens==command.requested_tokens && r.reason.is_none(),("project.release","released")=>r.reserved_tokens==command.requested_tokens && (0..=r.reserved_tokens).contains(&r.retained_tokens) && r.reason.is_none(),(_,"refused")=>r.reserved_tokens==0 && r.retained_tokens==0 && r.reason.as_deref().is_some_and(|s|matches!(s,"insufficient_capacity"|"authority_changed"|"invalid_request"|"project_unavailable"|"resource_unavailable"|"request_conflict"|"unsettled_usage")),_=>false};
            if r.version!=1 || r.command_id!=id || r.allocation_id!=w.allocation_id || r.period_key!=command.grant_receipt.period_key || !(1..=MAX_EXACT).contains(&r.ledger_revision) || r.ledger_revision<=command.grant_receipt.ledger_revision || !valid_state {return Err(Error::Invalid("invalid_executor_result"));}
            let a=allocation(db,&w.allocation_id).await?;let g=grant(db,&a.grant_id).await?;
            if r.state=="released" && r.retained_tokens < super::usage::known(db,super::usage::Scope::Allocation(&a.id)).await? {return Err(Error::Conflict("executor_usage_changed"));}
            super::parents::validate_child_receipt(db,&a.grant_id,r).await?;
            super::agents::executor::validate_external_ledger_receipt(db,&w.owner_user_id,&w.executor_device_id,r.ledger_revision,r.ledger_revision).await?;
            // Release cannot predate a reservation result already observed here.
            // A late historical reserve receipt is still accepted after a newer
            // release, but never replaces that release's projection.
            let previous_balance=a.balance.as_deref().map(decode::<NativeAllocationReceipt>).transpose()?;
            let previous_result=a.result.as_deref().map(decode::<NativeAllocationReceipt>).transpose()?;
            if w.operation=="project.release" && previous_balance.iter().chain(previous_result.iter()).any(|previous| r.ledger_revision <= previous.ledger_revision) {return Err(Error::Conflict("executor_ledger_revision_changed"));}
            if w.operation=="project.reserve" && r.state=="reserved" && previous_balance.as_ref().is_some_and(|previous| previous.state=="released" && r.ledger_revision>=previous.ledger_revision) {return Err(Error::Conflict("executor_ledger_revision_changed"));}
            let advance_balance=r.state!="refused" && previous_balance.as_ref().is_none_or(|previous| r.ledger_revision>previous.ledger_revision);
            sql_query("UPDATE hagency_agent_v1.management_executor_commands SET state='recorded',receipt=$2,receipt_digest=$3 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(wire(r)?).bind::<Text,_>(&result_digest).execute(db).await?;
            let state=if w.operation=="project.release" {
                if a.release_command_id.as_deref()!=Some(id){return Err(Error::Conflict("executor_command_superseded"));}
                if r.state=="released"{"released"}else{"release_blocked"}
            } else if a.release_command_id.is_some() {a.state.as_str()} else if r.state=="refused" {"refused"} else if allocation_live(db,&a,&g,now).await? {"reserved"} else {"releasing"};
            // Late reserve reports never overwrite a later release/settlement.
            if w.operation=="project.release" || a.release_command_id.is_none() {
                sql_query("UPDATE hagency_agent_v1.management_allocations SET state=$2,result=$3,balance=CASE WHEN $4 THEN $3 ELSE balance END,revision=revision+1 WHERE id=$1").bind::<Text,_>(&a.id).bind::<Text,_>(state).bind::<Text,_>(wire(r)?).bind::<Bool,_>(r.state!="refused").execute(db).await?;
            } else if advance_balance {
                // A delayed reserve can establish a known hold while release is
                // pending/refused, but cannot replace a newer released balance.
                sql_query("UPDATE hagency_agent_v1.management_allocations SET balance=$2,revision=revision+1 WHERE id=$1").bind::<Text,_>(&a.id).bind::<Text,_>(wire(r)?).execute(db).await?;
            }
            if state=="releasing" && a.release_command_id.is_none() {release(db,p,policy,&a.id,now).await?;}
            if w.operation=="project.release" || a.release_command_id.is_none() || advance_balance {
                sql_query("UPDATE hagency_agent_v1.management_requests SET revision=revision+1 WHERE id=$1").bind::<Text,_>(&a.request_id).execute(db).await?;
                let current=request(db,&a.request_id,p,policy,now).await?;
                let manager=plan(db,&a.project_id).await?;
                let grant_receipt:NativeGrantReceipt=decode(&g.receipt)?;
                notify(db,&current,[manager.manager_mxid,grant_receipt.owner_mxid.clone(),grant_receipt.owner_mxid],now).await?;
            }
            let projection=view(db,&a.id,p,policy,now).await?;
            Self::audit(db,p,"executor.result",id,now).await?;
            Ok(AllocationAcknowledgement{command_id:id.into(),command_digest:w.digest,receipt_digest:result_digest,allocation:projection})
        }).await
    }
}
