//! Owner ledger epochs. A native owner ledger numbers its own mutations from 1,
//! so a device that replaces its ledger database would reuse numbers the old
//! database already reported. Each ledger a device declares opens its next
//! epoch, and receipt numbers collide only inside one epoch. A device that
//! never declares stays in epoch 0, which is every receipt recorded before
//! declarations existed.
//!
//! A new database also knows nothing of what the earlier one held: its pools,
//! grants, reservations and unfinished commands cannot be answered any more.
//! The declaration that replaces a database therefore ends all of it in the
//! same transaction (see `sweep`). A first declaration only names the database
//! the device already reported from, so it ends nothing unless the device
//! says the database is `fresh`.
use super::*;
use crate::management_agent_executor_types::{ExecutorLedger, ExecutorLedgerInput};

/// What an owner device's delivery or receipt is about.
#[derive(Clone, Copy)]
pub(in crate::domain::management) enum Subject<'a> {
    /// A resource grant, with its project allocations and parent settlement.
    Grant(&'a str),
    /// A shared Agent request, with its intent, assignment, processes and work.
    Request(&'a str),
    /// A grant being published by the current database.
    Publication,
}

/// The subject belonged to a ledger database its device has since replaced.
pub(in crate::domain::management) async fn replaced(
    db: &mut AsyncPgConnection,
    subject: Subject<'_>,
) -> Result<bool> {
    let (query, id) = match subject {
        Subject::Grant(id) => (
            "SELECT hagency_agent_v1.executor_ledger_replaced_grant($1) AS matched",
            id,
        ),
        Subject::Request(id) => (
            "SELECT hagency_agent_v1.executor_ledger_replaced_request($1) AS matched",
            id,
        ),
        Subject::Publication => return Ok(false),
    };
    Ok(sql_query(query)
        .bind::<Text, _>(id)
        .get_result::<Flag>(db)
        .await?
        .matched)
}

/// Nothing of a replaced database is delivered, recovered or recorded again.
pub(in crate::domain::management) async fn refuse_replaced(
    db: &mut AsyncPgConnection,
    subject: Subject<'_>,
) -> Result<()> {
    if replaced(db, subject).await? {
        return Err(Error::Conflict("executor_ledger_replaced"));
    }
    Ok(())
}

#[derive(QueryableByName)]
struct Declared {
    #[diesel(sql_type=Text)]
    ledger_id: String,
    #[diesel(sql_type=BigInt)]
    epoch: i64,
}

/// The epoch this device's new receipts are numbered in. Callers hold the
/// domain transaction lock taken by `authorize`, as a declaration does.
pub(in crate::domain::management) async fn current_epoch(
    db: &mut AsyncPgConnection,
    owner: &str,
    device: &str,
) -> Result<i64> {
    Ok(sql_query("SELECT coalesce(max(epoch),0)::bigint AS now FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .get_result::<Clock>(db)
        .await?
        .now)
}

#[derive(QueryableByName)]
struct Swept {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Text)]
    request_id: String,
}
#[derive(QueryableByName)]
struct Key {
    #[diesel(sql_type=Text)]
    id: String,
}

/// Contributions this declaration replaced. `$1` owner, `$2` device, `$3` epoch.
const REPLACED: &str = "(SELECT id FROM hagency_agent_v1.management_contributions WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_replaced_epoch=$3)";

async fn count(db: &mut AsyncPgConnection, query: &str, owner: &str, device: &str) -> Result<i64> {
    Ok(sql_query(query)
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .get_result::<Clock>(db)
        .await?
        .now)
}

/// Ends everything the device's earlier ledger database left behind. The
/// device can no longer fence, release, close or settle any of it, so each
/// object takes the end an owner withdrawal gives it where that needs no
/// native receipt, and otherwise closes with what the server had recorded:
/// usage the device never reported is not added. No receipt is invented and no
/// recorded row is changed. Commands of the earlier database stay as they
/// were written; the mark on their contribution keeps them from delivery and
/// from ever taking a receipt.
///
/// Only objects under this owner device's contributions are touched. Other
/// devices of the owner, other owners, the applicants' Agent identities and
/// the owner's personal Agents (which have no contribution) are not.
async fn sweep(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    device: &str,
    epoch: i64,
    now: i64,
) -> Result<serde_json::Value> {
    let owner = p.user_id.as_str();
    // What the fences below will end, counted while it is still open.
    let intents = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_agent_intents WHERE executor_owner_user_id=$1 AND executor_device_id=$2 AND state<>'fenced'", owner, device).await?;
    let assignments = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_shared_assignments WHERE executor_owner_user_id=$1 AND executor_device_id=$2 AND state<>'fenced'", owner, device).await?;
    let work = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2 AND w.state IN ('queued','acknowledged','started')", owner, device).await?;

    // Pools. An open contribution is withdrawn as its owner would withdraw
    // it; every contribution of the device, open or already closed, is marked
    // as belonging to the replaced database.
    let open = sql_query("SELECT id,state,request_id FROM hagency_agent_v1.management_contributions WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_replaced_epoch IS NULL AND state IN ('draft','pending_admission','admitted') ORDER BY id")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .load::<Swept>(db)
        .await?;
    sql_query("UPDATE hagency_agent_v1.management_contributions SET state='revoked',revision=revision+1,updated_at_ms=$4,ledger_replaced_epoch=$3 WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_replaced_epoch IS NULL AND state IN ('draft','pending_admission','admitted')")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .bind::<BigInt, _>(epoch)
        .bind::<BigInt, _>(now)
        .execute(db)
        .await?;
    let closed_before = sql_query("UPDATE hagency_agent_v1.management_contributions SET ledger_replaced_epoch=$3 WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_replaced_epoch IS NULL")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .bind::<BigInt, _>(epoch)
        .execute(db)
        .await?;
    for c in &open {
        sql_query("UPDATE hagency_agent_v1.management_requests SET state='revoked',revision=revision+1,decided_at_ms=$2 WHERE id=$1")
            .bind::<Text, _>(&c.request_id)
            .bind::<BigInt, _>(now)
            .execute(db)
            .await?;
        sql_query("UPDATE hagency_agent_v1.management_outbox SET state='cancelled' WHERE request_id=$1 AND state='pending'")
            .bind::<Text, _>(&c.request_id)
            .execute(db)
            .await?;
    }

    // Grants. Revocation is the owner-withdrawal path: the schema fences every
    // Agent intent under the grant, its assignment, processes and open work.
    let grants = sql_query(format!("UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1 WHERE state='active' AND contribution_id IN {REPLACED}"))
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .bind::<BigInt, _>(epoch)
        .execute(db)
        .await?;

    // Project allocations close without a native release.
    let held = sql_query(format!("SELECT a.id FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id JOIN hagency_agent_v1.management_projects m ON m.id=a.project_id WHERE (g.contribution_id IN {REPLACED} OR m.contribution_id IN {REPLACED}) ORDER BY a.id"))
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .bind::<BigInt, _>(epoch)
        .load::<Key>(db)
        .await?;
    let mut allocations = 0i64;
    for a in &held {
        if allocations::end_for_replaced_ledger(db, p, policy, &a.id, now).await? {
            allocations += 1;
        }
    }
    // Project plans of the withdrawn pools close as they do on a withdrawal.
    // Their allocations are already closed, so no release is queued.
    for c in &open {
        projects::revoke_contribution_projects(db, p, policy, &c.id, now).await?;
    }

    // An Agent intent can only be bound to this device through one of its
    // grants, so the fences above reached all of them. This makes it certain.
    sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE executor_owner_user_id=$1 AND executor_device_id=$2 AND state<>'fenced'")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .execute(db)
        .await?;

    // A fenced process normally stops when the device reports it stopped. This
    // device will not report on the earlier database's processes again.
    let processes = sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET state='stopped',permanent_fence=true,activity_until_ms=0 WHERE state<>'stopped' AND assignment_id IN (SELECT id FROM hagency_agent_v1.management_shared_assignments WHERE executor_owner_user_id=$1 AND executor_device_id=$2)")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .execute(db)
        .await?;

    // A native close can no longer be confirmed or continued. The settlement
    // ends with the amounts the server holds; its stored receipt is unchanged.
    let parents = sql_query(format!("UPDATE hagency_agent_v1.management_parent_settlements SET state='closed',revision=revision+1 WHERE state<>'closed' AND grant_id IN (SELECT id FROM hagency_agent_v1.management_resource_grants WHERE contribution_id IN {REPLACED})"))
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .bind::<BigInt, _>(epoch)
        .execute(db)
        .await?;

    // Commands the earlier database never answered. They are not rewritten.
    let agent_commands = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.executor_owner_user_id=$1 AND i.executor_device_id=$2 AND w.receipt IS NULL", owner, device).await?;
    let project_commands = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND state<>'recorded'", owner, device).await?;
    let parent_commands = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND state<>'recorded'", owner, device).await?;
    let budget_commands = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_agent_budget_commands c WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results r WHERE r.command_id=c.id)", owner, device).await?;
    let settlement_commands = count(db, "SELECT count(*)::bigint AS now FROM hagency_agent_v1.management_agent_settlement_commands c WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results r WHERE r.command_id=c.id)", owner, device).await?;

    // People who were told of the pool learn of its end as on a withdrawal. A
    // draft was never shown to anyone but its owner.
    for c in open.iter().filter(|c| c.state != "draft") {
        let result = admission(db, &c.id, p, policy, now).await?;
        notify(
            db,
            &result.request,
            [
                result.contribution.owner_mxid.clone(),
                result.contribution.coordinator_mxid.clone(),
                policy.administrator().to_owned(),
            ],
            now,
        )
        .await?;
    }
    Ok(serde_json::json!({
        "reason": "executor_ledger_replaced",
        "contributionsRevoked": open.len(),
        "contributionsAlreadyClosed": closed_before,
        "grantsRevoked": grants,
        "allocationsReleased": allocations,
        "parentSettlementsClosed": parents,
        "agentIntentsFenced": intents,
        "agentAssignmentsFenced": assignments,
        "sharedProcessesStopped": processes,
        "sharedWorkEnded": work,
        "unansweredAgentCommands": agent_commands,
        "unansweredProjectCommands": project_commands,
        "unansweredParentCommands": parent_commands,
        "unansweredBudgetCommands": budget_commands,
        "unansweredSettlementCommands": settlement_commands,
    }))
}

impl DomainStore {
    /// Declares the ledger database the calling executor device now reports
    /// from. Repeating the current id changes nothing. An id from an earlier
    /// epoch is never accepted again: its numbers were already used.
    ///
    /// A new id after an earlier declaration means the database was replaced,
    /// and ends what the earlier one left behind. The first declaration does
    /// so only when the device states the database is `fresh`.
    pub async fn declare_executor_ledger(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorLedgerInput,
    ) -> Result<ExecutorLedger> {
        input.validate()?;
        let (device, _) = device(p)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                // The domain transaction lock serializes concurrent declarations
                // and every receipt of this device. The table's unique keys
                // are the backstop.
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let current = sql_query("SELECT ledger_id,epoch FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2 ORDER BY epoch DESC LIMIT 1")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .get_result::<Declared>(db)
                    .await
                    .optional()?;
                if let Some(current) = &current
                    && current.ledger_id == input.ledger_id
                {
                    return Ok(ExecutorLedger {
                        ledger_id: current.ledger_id.clone(),
                        epoch: current.epoch,
                    });
                }
                let retired = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_id=$3) AS matched")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .bind::<Text, _>(&input.ledger_id)
                    .get_result::<Flag>(db)
                    .await?
                    .matched;
                if retired {
                    return Err(Error::Conflict("agent_executor_ledger_retired"));
                }
                let previous = current.map_or(0, |current| current.epoch);
                let epoch = previous + 1;
                sql_query("INSERT INTO hagency_agent_v1.management_executor_ledgers(owner_user_id,executor_device_id,ledger_id,epoch,declared_at_ms) VALUES($1,$2,$3,$4,$5)")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .bind::<Text, _>(&input.ledger_id)
                    .bind::<BigInt, _>(epoch)
                    .bind::<BigInt, _>(now)
                    .execute(db)
                    .await?;
                Self::audit(db, p, "executor.ledger.declare", &input.ledger_id, now).await?;
                // An earlier declared database is replaced by this one. Before
                // any declaration the device may simply be naming the database
                // it always had, unless it says otherwise.
                if previous >= 1 || input.fresh {
                    let ended = sweep(db, p, policy, device, epoch, now).await?;
                    sql_query("UPDATE hagency_agent_v1.management_executor_ledgers SET swept=true,sweep=$4 WHERE owner_user_id=$1 AND executor_device_id=$2 AND epoch=$3")
                        .bind::<Text, _>(&p.user_id)
                        .bind::<Text, _>(device)
                        .bind::<BigInt, _>(epoch)
                        .bind::<Text, _>(wire(&ended)?)
                        .execute(db)
                        .await?;
                    Self::audit(db, p, "executor.ledger.sweep", &input.ledger_id, now).await?;
                }
                Ok(ExecutorLedger {
                    ledger_id: input.ledger_id.clone(),
                    epoch,
                })
            })
            .await
    }
}
