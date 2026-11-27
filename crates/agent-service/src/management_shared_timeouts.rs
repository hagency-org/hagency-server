//! Time bound for uncertain shared execution: an attempt that was issued but
//! never claimed, claimed with an unknown process, or whose stop is
//! unconfirmed never holds an Agent's queue indefinitely.
//!
//! After the grace period the server fences and stops the attempt. It ends a
//! budget demand the attempt started, and a v2 demand whose attempt reported
//! a process unless that demand's start said its executor takes messages
//! back. Such a demand, and one whose attempt never reported a process, waits
//! for the executor's evidence, and the wait of the first is bounded too (see
//! `REOFFER_GRACE_MS`). The server invents no receipt, process identity or
//! usage: the executor supplies explicit evidence, `not_started` when nothing
//! was claimed or a `stopped` receipt with reason `uncertain_timeout`, and the
//! next attempt names that evidence as its predecessor stop.
use super::*;

/// Grace after the attempt's own deadline or the last process evidence.
pub(super) const UNCERTAIN_GRACE_MS: i64 = 180_000;
/// How long a message whose process stopped, and whose start said that its
/// executor takes messages back, waits for the executor beyond the time bound
/// above: for its evidence that the process stopped and its retry's start.
/// It is counted once per message, from its first process: from the end of
/// that process's time bound (its last evidence and `UNCERTAIN_GRACE_MS`) or
/// from the executor's first request for a retry, whichever is later. Later
/// attempts do not move it. Then the event ends (`head_changed`,
/// `retry_lapsed`), no new attempt starts for it, and the queue moves on. An
/// attempt whose start is committed has a process of its own and is not
/// lapsed while it runs; if it ends without its turn after the bound, the
/// event ends at once.
pub(super) const REOFFER_GRACE_MS: i64 = UNCERTAIN_GRACE_MS;

// Execute tickets never outlive `expires_at_ms`, so the attempt deadline also
// bounds every execute lease. Observe and cleanup tickets are not evidence of
// a live process and never extend the bound. Activity leases and work
// transitions are the server's process evidence. A fence clears the current
// activity end; the first lease is minted inside the window and lasts at most
// 30 s, and every renewal keeps its own end.
const LAST_EVIDENCE: &str = "GREATEST(r.expires_at_ms,r.activity_until_ms,\
 CASE WHEN r.activity_epoch>0 THEN r.expires_at_ms+30000 ELSE 0 END,\
 coalesce((SELECT max(x.expires_at_ms) FROM hagency_agent_v1.management_shared_activity_renewals x WHERE x.runtime_id=r.id),0),\
 coalesce((SELECT max(GREATEST(coalesce(w.acknowledged_at_ms,0),coalesce(w.started_at_ms,0),coalesce(w.completed_at_ms,0))) FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=r.id),0))";

/// True when this exact attempt is uncertain and its grace period has ended.
pub(super) async fn due(db: &mut AsyncPgConnection, id: &str, now: i64) -> Result<bool> {
    Ok(sql_query(format!("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=$1 AND r.payload::jsonb->>'version' IN ('2','3') AND r.state IN ('pending','issued','qualified','unknown') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_result_tickets t WHERE t.runtime_id=r.id AND t.mode='execute' AND t.expires_at_ms>$2) AND {LAST_EVIDENCE}+$3<=$2) AS matched"))
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(now)
        .bind::<BigInt, _>(UNCERTAIN_GRACE_MS)
        .get_result::<Flag>(db)
        .await?
        .matched)
}

/// Stopped by the time bound and still waiting for the executor's explicit
/// evidence. Such an attempt gets a cleanup ticket so evidence can arrive.
pub(super) async fn awaiting_evidence(db: &mut AsyncPgConnection, r: &Runtime) -> Result<bool> {
    if r.state != "stopped" || runtime_version(r)? == 1 {
        return Ok(false);
    }
    Ok(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND payload::jsonb->>'state' IN ('stopped','not_started')) AS matched")
        .bind::<Text, _>(&r.id)
        .get_result::<Flag>(db)
        .await?
        .matched)
}

/// Stop every due attempt (optionally only one executor's) and end the demand
/// each one started where `demands::interrupt` says so. Bounded per call; a
/// later call continues.
pub(super) async fn sweep(
    db: &mut AsyncPgConnection,
    executor: Option<(&str, &str)>,
    now: i64,
) -> Result<()> {
    let ids = candidates(db, executor, now).await?;
    for id in ids {
        stop(db, &id.id).await?;
    }
    Ok(())
}

async fn candidates(
    db: &mut AsyncPgConnection,
    executor: Option<(&str, &str)>,
    now: i64,
) -> Result<Vec<Id>> {
    Ok(sql_query(format!("SELECT r.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE r.payload::jsonb->>'version' IN ('2','3') AND r.state IN ('pending','issued','qualified','unknown') AND ($1::text IS NULL OR (a.executor_owner_user_id=$1 AND a.executor_device_id=$2)) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_result_tickets t WHERE t.runtime_id=r.id AND t.mode='execute' AND t.expires_at_ms>$3) AND {LAST_EVIDENCE}+$4<=$3 ORDER BY r.id LIMIT 64"))
        .bind::<Nullable<Text>,_>(executor.map(|e| e.0))
        .bind::<Nullable<Text>,_>(executor.map(|e| e.1))
        .bind::<BigInt,_>(now).bind::<BigInt,_>(UNCERTAIN_GRACE_MS)
        .load::<Id>(db).await?)
}

// A demand whose latest start said that its executor takes messages back,
// whose process has stopped, and which is still waiting: no binding, no
// block. `$1` narrows it to one demand.
const LAPSED: &str = "SELECT d.id FROM hagency_agent_v1.management_shared_intake q \
 JOIN hagency_agent_v1.management_shared_demands d ON d.intake_id=q.id \
 JOIN LATERAL (SELECT runtime_id FROM (SELECT 0::bigint AS retry,runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=d.id \
  UNION ALL SELECT retry,runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=d.id) x ORDER BY retry DESC LIMIT 1) s ON true \
 JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=s.runtime_id \
 WHERE q.state='pending' AND ($1::text IS NULL OR d.id=$1) AND r.state='stopped' \
  AND hagency_agent_v1.shared_start_reoffers(d.id,r.id) \
  AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings b WHERE b.demand_id=d.id) \
  AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_blocks k WHERE k.demand_id=d.id)";

/// True when such a waiting demand's bounded wait has ended.
pub(super) async fn reoffer_lapsed(
    db: &mut AsyncPgConnection,
    demand: &str,
    now: i64,
) -> Result<bool> {
    Ok(!lapsed(db, Some(demand), now).await?.is_empty())
}

// The first process is the one of the demand's original start (`r` below is
// that process, not the latest one), and the first request is retry 1.
async fn lapsed(db: &mut AsyncPgConnection, demand: Option<&str>, now: i64) -> Result<Vec<Id>> {
    Ok(sql_query(format!("{LAPSED} AND GREATEST((SELECT {LAST_EVIDENCE} FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=(SELECT o.runtime_id FROM hagency_agent_v1.management_shared_demand_starts o WHERE o.demand_id=d.id))+$3,coalesce((SELECT t.created_at_ms FROM hagency_agent_v1.management_shared_demand_retries t WHERE t.demand_id=d.id AND t.retry=1),0))+$4<=$2 ORDER BY d.id LIMIT 64"))
        .bind::<Nullable<Text>, _>(demand)
        .bind::<BigInt, _>(now)
        .bind::<BigInt, _>(UNCERTAIN_GRACE_MS)
        .bind::<BigInt, _>(REOFFER_GRACE_MS)
        .load::<Id>(db)
        .await?)
}

async fn stop(db: &mut AsyncPgConnection, id: &str) -> Result<()> {
    // The fence's record says that the time bound gave this process up
    // (`stopSource`, migration 2026-11-26-000157).
    sql_query("SELECT set_config('hagency.time_bound_stop',$1,true)")
        .bind::<Text, _>(id)
        .execute(db)
        .await?;
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET state='stopped',permanent_fence=true,activity_until_ms=0 WHERE id=$1")
        .bind::<Text,_>(id).execute(db).await?;
    demands::interrupt(db, id).await
}

impl DomainStore {
    /// Background enforcement: no executor poll or healthy client is needed
    /// to stop an uncertain attempt and release its Agent's queue, or to end a
    /// message whose executor did not come back for it in time.
    pub(crate) async fn sweep_shared_attempt_timeouts(&self) -> Result<()> {
        let ids = self
            .db
            .transaction(async |db| {
                let now = clock(db).await?;
                candidates(db, None, now).await
            })
            .await?;
        for id in ids {
            self.db
                .write_transaction(async |db| {
                    let now = clock(db).await?;
                    if due(db, &id.id, now).await? {
                        stop(db, &id.id).await?;
                    }
                    Ok(())
                })
                .await?;
        }
        let ids = self
            .db
            .transaction(async |db| {
                let now = clock(db).await?;
                lapsed(db, None, now).await
            })
            .await?;
        for id in ids {
            self.db
                .write_transaction(async |db| {
                    let now = clock(db).await?;
                    if reoffer_lapsed(db, &id.id, now).await? {
                        demands::lapse(db, &id.id).await?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }
}
