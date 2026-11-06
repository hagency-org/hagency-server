//! Time bound for uncertain shared execution: an attempt that was issued but
//! never claimed, claimed with an unknown process, or whose stop is
//! unconfirmed never holds an Agent's queue indefinitely.
//!
//! After the grace period the server fences and stops the attempt and ends the
//! demand that started it. It invents no receipt, process identity or usage:
//! the executor then supplies explicit evidence, `not_started` when nothing
//! was claimed or a `stopped` receipt with reason `uncertain_timeout`, and the
//! next attempt names that evidence as its predecessor stop.
use super::*;

/// Grace after the attempt's own deadline or the last process evidence.
pub(super) const UNCERTAIN_GRACE_MS: i64 = 180_000;

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
/// each one started. Bounded per call; a later call continues.
pub(super) async fn sweep(
    db: &mut AsyncPgConnection,
    executor: Option<(&str, &str)>,
    now: i64,
) -> Result<()> {
    let ids = sql_query(format!("UPDATE hagency_agent_v1.management_shared_runtimes SET state='stopped',permanent_fence=true,activity_until_ms=0 WHERE id IN (SELECT r.id FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE r.payload::jsonb->>'version' IN ('2','3') AND r.state IN ('pending','issued','qualified','unknown') AND ($1::text IS NULL OR (a.executor_owner_user_id=$1 AND a.executor_device_id=$2)) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_result_tickets t WHERE t.runtime_id=r.id AND t.mode='execute' AND t.expires_at_ms>$3) AND {LAST_EVIDENCE}+$4<=$3 ORDER BY r.id LIMIT 64) RETURNING id"))
        .bind::<Nullable<Text>, _>(executor.map(|e| e.0))
        .bind::<Nullable<Text>, _>(executor.map(|e| e.1))
        .bind::<BigInt, _>(now)
        .bind::<BigInt, _>(UNCERTAIN_GRACE_MS)
        .load::<Id>(db)
        .await?;
    for id in ids {
        demands::interrupt(db, &id.id).await?;
    }
    Ok(())
}

impl DomainStore {
    /// Background enforcement: no executor poll or healthy client is needed
    /// to stop an uncertain attempt and release its Agent's queue.
    pub(crate) async fn sweep_shared_attempt_timeouts(&self) -> Result<()> {
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                    .await?;
                let now = clock(db).await?;
                sweep(db, None, now).await
            })
            .await
    }
}
