//! ADR-193 amendment: an event whose attempt provably never started runs again
//! in place. The demand, its event and its FIFO place stay; the retry is a new
//! attempt naming the not_started one. Only the executor's own evidence, sent
//! with its explicit request, opens a retry: the time bound never does, and an
//! executor that never asks keeps today's terminal behavior.
use super::*;

#[derive(QueryableByName)]
pub(super) struct LatestStart {
    #[diesel(sql_type=BigInt)]
    pub(super) retry: i64,
    #[diesel(sql_type=Text)]
    pub(super) runtime_id: String,
}
/// The demand's latest started attempt: its original start (retry 0) or its
/// newest retry start.
pub(super) async fn latest_start(
    db: &mut AsyncPgConnection,
    demand: &str,
) -> Result<Option<LatestStart>> {
    Ok(sql_query("SELECT retry,runtime_id FROM (SELECT 0::bigint AS retry,runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=$1 UNION ALL SELECT retry,runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=$1) s ORDER BY retry DESC LIMIT 1")
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
/// Open retry `n` after the not_started receipt `receipt` of `previous`, or
/// end the demand once the cap is reached. A demand that is no longer current
/// (stopped, cancelled, expired, moved on) is left to its own state.
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
    if q.state != "pending"
        || q.expires_at_ms <= now
        || binding(db, &d.id).await?.is_some()
        || status(db, d).await?.state != "start_committed"
        || head(db, &q.assignment_id).await?.is_none_or(|h| h.id != previous)
        || authorize(db, p, &q, server, now, true).await.is_err()
    {
        return Ok(());
    }
    if n > MAX_DEMAND_RETRIES {
        return block(db, d, &q, "head_changed", "\"retry_exhausted\"").await;
    }
    sql_query("INSERT INTO hagency_agent_v1.management_shared_demand_retries VALUES($1,$2,$3,$4,$5)")
        .bind::<Text, _>(&d.id).bind::<BigInt, _>(n).bind::<Text, _>(previous)
        .bind::<Text, _>(receipt).bind::<BigInt, _>(now).execute(db).await?;
    Ok(())
}
impl DomainStore {
    /// Record the executor's not_started evidence for the demand's latest
    /// attempt (exactly as the attempt result route does, on its cleanup
    /// ticket) and, in the same transaction, re-offer the same event.
    pub async fn retry_shared_demand(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedDemandRetryInput,
    ) -> Result<SharedDemandRetryReply> {
        key(id)?;
        input.validate()?;
        enabled_demand(policy)?;
        let mut g = self.db.lock().await?;
        (*g).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, 0).await?;
            let d = load(db, id).await?;
            let q = queued(db, &d.intake_id).await?;
            let o = offer(&d, &q)?;
            let now = clock(db).await?;
            authorize(db, p, &q, &self.server, now, false).await?;
            if d.digest != input.demand_digest || o.control_revision != input.expected_control_revision {
                return Err(Error::Conflict("shared_demand_changed"));
            }
            let latest = latest_start(db, &d.id)
                .await?
                .ok_or(Error::Conflict("shared_demand_retry_changed"))?;
            if latest.runtime_id != input.attempt_id || input.retry != latest.retry + 1 {
                return Err(Error::Conflict("shared_demand_retry_changed"));
            }
            let acknowledgement =
                attempts::record_in(db, p, policy, &self.server, &input.attempt_id, &input.result)
                    .await?;
            open(db, p, &d, &input.attempt_id, input.retry, &acknowledgement.observation_id, &self.server).await?;
            Ok(SharedDemandRetryReply {
                acknowledgement,
                demand: status(db, &d).await?,
            })
        })
        .await
    }
}
