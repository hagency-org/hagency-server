//! ADR-193 amendment: an event whose attempt provably never started runs again
//! in place. The demand, its event and its FIFO place stay; the retry is a new
//! attempt naming the not_started one. Only the executor's own evidence, sent
//! with its explicit request, opens a retry: the time bound never does, and an
//! executor that never asks keeps today's terminal behavior.
//!
//! The same holds for an attempt whose process did start and then ended
//! before a turn began for the event (time-limit rule 3: the line between
//! what is redone and what is never guessed is the start of a turn), when
//! the start that began it said that its executor takes messages back
//! (`reoffers`). The evidence is then the process's `stopped` receipt, and
//! whether a turn began is the server's own record, not the executor's claim:
//! a start is recorded before the turn and refused once the process is
//! fenced. If the event had been given to that process, the database released
//! it at the fence (migration 2026-11-27-000159), so it is a waiting event
//! again here. A `stopped` receipt for any other start is refused on this
//! route, as before.
use super::*;

#[derive(QueryableByName)]
pub(super) struct LatestStart {
    #[diesel(sql_type=BigInt)]
    pub(super) retry: i64,
    #[diesel(sql_type=Text)]
    pub(super) runtime_id: String,
    /// The start said that its executor takes messages back.
    #[diesel(sql_type=Bool)]
    pub(super) reoffers: bool,
}
/// The demand's latest started attempt: its original start (retry 0) or its
/// newest retry start.
pub(super) async fn latest_start(
    db: &mut AsyncPgConnection,
    demand: &str,
) -> Result<Option<LatestStart>> {
    Ok(sql_query("SELECT retry,runtime_id,hagency_agent_v1.shared_start_reoffers($1,runtime_id) AS reoffers FROM (SELECT 0::bigint AS retry,runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=$1 UNION ALL SELECT retry,runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=$1) s ORDER BY retry DESC LIMIT 1")
        .bind::<Text, _>(demand).get_result(db).await.optional()?)
}
#[derive(QueryableByName)]
pub(super) struct Offered {
    #[diesel(sql_type=BigInt)]
    pub(super) retry: i64,
    #[diesel(sql_type=Text)]
    pub(super) previous_runtime_id: String,
}
/// A retry the executor asked for whose start is not committed yet.
pub(super) async fn offered(db: &mut AsyncPgConnection, demand: &str) -> Result<Option<Offered>> {
    Ok(sql_query("SELECT r.retry,r.previous_runtime_id FROM hagency_agent_v1.management_shared_demand_retries r WHERE r.demand_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_retry_starts s WHERE s.demand_id=r.demand_id AND s.retry=r.retry) ORDER BY r.retry DESC LIMIT 1")
        .bind::<Text, _>(demand).get_result(db).await.optional()?)
}
pub(super) async fn started(
    db: &mut AsyncPgConnection,
    demand: &str,
    retry: i64,
) -> Result<Option<Started>> {
    Ok(sql_query("SELECT runtime_id,payload,digest FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=$1 AND retry=$2")
        .bind::<Text, _>(demand).bind::<BigInt, _>(retry).get_result(db).await.optional()?)
}
#[derive(QueryableByName)]
struct Retry {
    #[diesel(sql_type=Text)]
    previous_runtime_id: String,
    #[diesel(sql_type=Text)]
    receipt_id: String,
}
/// Open retry `n` after the receipt `receipt` of `previous` (not_started, or
/// stopped with no turn begun for the event; the insert's trigger checks
/// which), or end the demand once the cap is reached. A demand that is no
/// longer current (stopped, cancelled, expired, moved on, or bound to work
/// that did begin), or whose retry's start would be refused (a budget was
/// applied, or its period has turned), is left to its own state.
async fn open(
    db: &mut AsyncPgConnection,
    p: &Principal,
    d: &Demand,
    previous: &str,
    n: i64,
    receipt: &str,
    server: &str,
) -> Result<()> {
    let old = sql_query("SELECT previous_runtime_id,receipt_id FROM hagency_agent_v1.management_shared_demand_retries WHERE demand_id=$1 AND retry=$2")
        .bind::<Text, _>(&d.id).bind::<BigInt, _>(n).get_result::<Retry>(db).await.optional()?;
    if let Some(old) = old {
        if old.previous_runtime_id != previous || old.receipt_id != receipt {
            return Err(Error::Conflict("shared_demand_retry_changed"));
        }
        return Ok(());
    }
    let q = queued(db, &d.intake_id).await?;
    let now = clock(db).await?;
    // A message given back whose wait is over ends now, as a read or the
    // sweep would end it, and nothing is opened for it.
    if timeouts::reoffer_lapsed(db, &d.id, now).await? {
        return end_wait(db, d, &q).await;
    }
    let Ok(w) = authorize(db, p, &q, server, now, true).await else {
        return Ok(());
    };
    if start_refusal(db, d, &q, &w, now).await?.is_some()
        || status(db, d).await?.state != "start_committed"
        || head(db, &q.assignment_id)
            .await?
            .is_none_or(|h| h.id != previous)
    {
        return Ok(());
    }
    if n > MAX_DEMAND_RETRIES {
        return block(db, d, &q, "head_changed", "\"retry_exhausted\"").await;
    }
    sql_query("INSERT INTO hagency_agent_v1.management_shared_demand_retries(demand_id,retry,previous_runtime_id,receipt_id,created_at_ms) VALUES($1,$2,$3,$4,$5)")
        .bind::<Text, _>(&d.id).bind::<BigInt, _>(n).bind::<Text, _>(previous)
        .bind::<Text, _>(receipt).bind::<BigInt, _>(now).execute(db).await?;
    Ok(())
}
impl DomainStore {
    /// Record the executor's evidence that the demand's latest attempt never
    /// started, or stopped (exactly as the attempt result route does, with
    /// the same ticket) and, in the same transaction, re-offer the same
    /// event if no turn began for it.
    pub async fn retry_shared_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedDemandRetryInput,
    ) -> Result<SharedDemandRetryReply> {
        key(id)?;
        input.validate()?;
        // The same predicate decides whether discovery advertises this route.
        if !policy.shared_demand_retries() {
            return Err(Error::Unavailable("shared_demand_retry_unavailable"));
        }
        self.db
            .write_transaction(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let d = load(db, id).await?;
                let q = queued(db, &d.intake_id).await?;
                let o = offer(&d, &q)?;
                let now = clock(db).await?;
                authorize(db, p, &q, &self.server, now, false).await?;
                if d.digest != input.demand_digest
                    || o.control_revision != input.expected_control_revision
                {
                    return Err(Error::Conflict("shared_demand_changed"));
                }
                let latest = latest_start(db, &d.id)
                    .await?
                    .ok_or(Error::Conflict("shared_demand_retry_changed"))?;
                if latest.runtime_id != input.attempt_id || input.retry != latest.retry + 1 {
                    return Err(Error::Conflict("shared_demand_retry_changed"));
                }
                // A process that ran is retried only for a start that said
                // its executor takes messages back, and only while the
                // re-offer is on; otherwise the route refuses the receipt and
                // records nothing, as before (the plain result route records
                // it).
                if input.result.receipt.state == "stopped"
                    && !(latest.reoffers && policy.shared_demand_reoffers())
                {
                    return Err(Error::Invalid("invalid_shared_demand_retry"));
                }
                let acknowledgement = attempts::record_in(
                    db,
                    p,
                    policy,
                    &self.server,
                    &input.attempt_id,
                    &input.result,
                )
                .await?;
                open(
                    db,
                    p,
                    &d,
                    &input.attempt_id,
                    input.retry,
                    &acknowledgement.observation_id,
                    &self.server,
                )
                .await?;
                Ok(SharedDemandRetryReply {
                    acknowledgement,
                    demand: status(db, &d).await?,
                })
            })
            .await
    }
}
