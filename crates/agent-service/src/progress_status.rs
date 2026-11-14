//! Live turn status beside the request (top level, or in its real thread): "Working…" posted once, then
//! edited (`m.replace`, same puppet sender) as the executing device reports
//! steps, at most once per 1.5 s, and finally edited to "Done in 41 s · 6
//! steps" or to the request's failure text. Reports are fenced like replies
//! (owner, device, lease epoch, dispatch, execution, binding generation, fresh
//! Matrix authority); stale ones are refused. Cosmetic: a failed post or edit
//! never affects the turn, its reply or its usage, and nothing here reruns work.
use super::*;
use crate::{gateway::Gateway, matrix_client::MatrixClient};
use diesel::sql_types::Jsonb;
use serde_json::Value;

/// Minimum time between two posts or edits of one status message.
pub(crate) const MIN_EDIT_INTERVAL_MS: i64 = 1500;
/// Recent steps a device may report; matches the engine's `RECENT_STEPS`.
const MAX_RECENT: usize = 5;
const MAX_STEP_BYTES: usize = 512;
const WORKING: &str = "⏳ Working…";
const DONE: &str = "✅ Done in {s} s · {n} steps";
const DONE_ONE: &str = "✅ Done in {s} s · 1 step";

#[derive(Debug, diesel::QueryableByName)]
pub(crate) struct ProgressStatus {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Text)]
    pub execution_id: String,
    #[diesel(sql_type=BigInt)]
    pub started_at_ms: i64,
    #[diesel(sql_type=Jsonb)]
    pub recent: Value,
    #[diesel(sql_type=BigInt)]
    pub step_count: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub final_body: Option<String>,
    #[diesel(sql_type=BigInt)]
    pub revision: i64,
    #[diesel(sql_type=BigInt)]
    pub sent_revision: i64,
    #[diesel(sql_type=Text)]
    pub matrix_txn_id: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub status_event_id: Option<String>,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub worker_token: Option<String>,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub event_id: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
    #[diesel(sql_type=Text)]
    pub event_state: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub event_execution_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub outcome: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    pub reply_at_ms: Option<i64>,
}
const SELECT: &str = "SELECT s.id,s.binding_id,s.binding_generation,s.execution_id,s.started_at_ms,s.recent,s.step_count,s.final_body,s.revision,s.sent_revision,s.matrix_txn_id,s.status_event_id,s.state,s.worker_token,e.requester_mxid,e.room_id,e.event_id,e.thread_root,e.state AS event_state,e.execution_id AS event_execution_id,e.outcome,(SELECT r.created_at_ms FROM hagency_agent_v1.reply_outbox r WHERE r.owner_event_id=e.id) AS reply_at_ms FROM hagency_agent_v1.progress_status s JOIN hagency_agent_v1.owner_events e ON e.id=s.owner_event_id";

/// Plain text with control characters folded to spaces, bounded per step.
fn clean(step: &str) -> Result<String> {
    let step: String = step
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let step = step.trim();
    if step.is_empty() || step.len() > MAX_STEP_BYTES {
        return Err(Error::Invalid("invalid_progress_step"));
    }
    Ok(step.to_owned())
}
/// Earlier steps shown under the current one.
const SHOWN_EARLIER: usize = 3;
/// The live body: current step on the header line, the few before it below,
/// oldest first (rehearsal 2026-11-12: newest-first read backwards).
pub(crate) fn live_body(recent: &[String]) -> String {
    let Some((current, earlier)) = recent.split_last() else {
        return WORKING.to_owned();
    };
    let mut body = format!("{WORKING} {current}");
    for step in &earlier[earlier.len().saturating_sub(SHOWN_EARLIER)..] {
        body.push_str("\n· ");
        body.push_str(step);
    }
    body
}
/// The collapsed summary for a turn that produced a reply.
pub(crate) fn done_body(elapsed_ms: i64, steps: i64) -> String {
    let seconds = ((elapsed_ms.max(0) + 500) / 1000).max(1).to_string();
    let template = if steps == 1 { DONE_ONE } else { DONE };
    template
        .replace("{s}", &seconds)
        .replace("{n}", &steps.to_string())
}
/// The final body once the dispatch left `running`: the summary, or why it failed.
fn final_body(s: &ProgressStatus, now: i64) -> String {
    match (s.event_state.as_str(), s.outcome.as_deref()) {
        ("completed", Some("replied")) => {
            done_body(s.reply_at_ms.unwrap_or(now) - s.started_at_ms, s.step_count)
        }
        (_, Some("rejected")) => request_notices::body_for("rejected", "unobserved").to_owned(),
        ("unknown", _) => request_notices::body_for("interrupted", "unobserved").to_owned(),
        _ => request_notices::body_for("failed", "unobserved").to_owned(),
    }
}
pub(super) fn body_of(s: &ProgressStatus) -> String {
    if let Some(body) = &s.final_body {
        return body.clone();
    }
    let recent: Vec<String> = serde_json::from_value(s.recent.clone()).unwrap_or_default();
    live_body(&recent)
}
#[path = "progress_status_content.rs"]
mod content;
pub(crate) use content::status_content;

impl TransportStore {
    async fn progress(db: &mut AsyncPgConnection, id: &str) -> Result<ProgressStatus> {
        Ok(sql_query(format!("{SELECT} WHERE s.id=$1"))
            .bind::<Text, _>(id)
            .get_result(db)
            .await?)
    }
    /// Device report for its own running execution. Creates the status once, then
    /// records the latest steps; the worker posts or edits it.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_progress(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        dispatch_id: &str,
        execution_id: &str,
        recent: &[String],
        steps: u32,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<bool> {
        key(execution_id)?;
        if recent.len() > MAX_RECENT {
            return Err(Error::Invalid("invalid_progress_step"));
        }
        let recent = Value::from(
            recent
                .iter()
                .map(|s| clean(s))
                .collect::<Result<Vec<_>>>()?,
        );
        self.observe_event_authority(p, dispatch_id, f, now).await?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            let e = Self::dispatch(db, p, dispatch_id).await?;
            let scope = Self::dispatch_scope(db, p, lease, &e, f, now).await?;
            fenced(&e, p, lease)?;
            if e.execution_id.as_deref() != Some(execution_id) || e.state != "running" {
                return Err(Error::Conflict("execution_not_running"));
            }
            plaintext_sendable(&scope, &e.requester_mxid, f, now)?;
            let changed = sql_query("INSERT INTO hagency_agent_v1.progress_status(id,owner_event_id,execution_id,binding_id,binding_generation,started_at_ms,recent,step_count,matrix_txn_id,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'live') ON CONFLICT(owner_event_id) DO UPDATE SET recent=EXCLUDED.recent,step_count=greatest(progress_status.step_count,EXCLUDED.step_count),revision=progress_status.revision+1 WHERE progress_status.state='live' AND progress_status.execution_id=EXCLUDED.execution_id AND EXCLUDED.step_count>=progress_status.step_count AND (progress_status.recent,progress_status.step_count) IS DISTINCT FROM (EXCLUDED.recent,greatest(progress_status.step_count,EXCLUDED.step_count))")
                .bind::<Text, _>(format!("progress_{}", hash(dispatch_id))).bind::<Text, _>(dispatch_id)
                .bind::<Text, _>(execution_id).bind::<Text, _>(&e.binding_id).bind::<BigInt, _>(e.binding_generation)
                .bind::<BigInt, _>(now).bind::<Jsonb, _>(recent.clone()).bind::<BigInt, _>(i64::from(steps))
                .bind::<Text, _>(format!("hg-progress-{}", hash(dispatch_id))).execute(db).await?;
            Ok(changed > 0)
        })
        .await
    }
    /// Trusted worker scan. Finalizes statuses whose dispatch left `running`,
    /// cancels ones whose binding changed, and returns those due a post or edit.
    pub(crate) async fn progress_candidates(
        &self,
        cursor: &str,
        now: i64,
    ) -> Result<Vec<ProgressStatus>> {
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::lock(db).await?;
            let now = Self::clock(db, now).await?;
            sql_query("UPDATE hagency_agent_v1.progress_status s SET state='cancelled' WHERE s.state IN ('live','final') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.id=s.binding_id AND b.generation=s.binding_generation AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused)").execute(db).await?;
            let ended: Vec<ProgressStatus> = sql_query(format!("{SELECT} WHERE s.state='live' AND (e.state<>'running' OR e.execution_id IS DISTINCT FROM s.execution_id) LIMIT 100"))
                .load(db).await?;
            // Not gated on the send lease: a claimed live revision is superseded, and
            // confirming it marks only that revision sent, so the summary still goes out.
            for s in ended {
                // A turn that ended before its first post needs no status at all.
                if s.status_event_id.is_none() {
                    sql_query("UPDATE hagency_agent_v1.progress_status SET state='cancelled' WHERE id=$1").bind::<Text, _>(&s.id).execute(db).await?;
                    continue;
                }
                sql_query("UPDATE hagency_agent_v1.progress_status SET state='final',final_body=$2,revision=revision+1 WHERE id=$1 AND state='live'")
                    .bind::<Text, _>(&s.id).bind::<Text, _>(final_body(&s, now)).execute(db).await?;
            }
            Ok(sql_query(format!("{SELECT} WHERE s.state IN ('live','final') AND s.sent_revision<s.revision AND s.worker_until_ms<=$1 AND s.last_sent_ms<=$1-$3 ORDER BY (s.id<=$2),s.id LIMIT 20"))
                .bind::<BigInt, _>(now).bind::<Text, _>(cursor).bind::<BigInt, _>(MIN_EDIT_INTERVAL_MS).load(db).await?)
        })
        .await
    }
    /// Claim one post/edit under fresh Matrix authority; the binding must be the
    /// status's own generation, and a live edit needs its execution still running.
    pub(crate) async fn claim_progress(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ProgressStatus> {
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::lock(db).await?;
            let now = Self::clock(db, now).await?;
            let s = Self::progress(db, id).await?;
            let running = s.event_state == "running" && s.event_execution_id.as_deref() == Some(&s.execution_id);
            if !matches!(s.state.as_str(), "live" | "final") || s.sent_revision >= s.revision || (s.state == "live" && !running) {
                return Err(Error::Conflict("progress_not_due"));
            }
            let due = sql_query("SELECT (worker_until_ms<=$2 AND last_sent_ms<=$2-$3) AS matched FROM hagency_agent_v1.progress_status WHERE id=$1")
                .bind::<Text, _>(id).bind::<BigInt, _>(now).bind::<BigInt, _>(MIN_EDIT_INTERVAL_MS).get_result::<Flag>(db).await?;
            if !due.matched {
                return Err(Error::Conflict("progress_not_due"));
            }
            let scope = Self::scope(db, &s.binding_id).await?;
            if !scope.active || scope.binding_generation != s.binding_generation {
                return Err(Error::Unauthorized("progress_scope_revoked"));
            }
            plaintext_sendable(&scope, &s.requester_mxid, f, now)?;
            sql_query("UPDATE hagency_agent_v1.progress_status SET worker_token=$2,worker_until_ms=$3 WHERE id=$1")
                .bind::<Text, _>(id).bind::<Text, _>(secret_token()).bind::<BigInt, _>(now + self.limits.worker_lease_ms).execute(db).await?;
            Self::progress(db, id).await
        })
        .await
    }
    /// Record one send. Success marks `revision` sent (and the status event on the
    /// first post); a permission denial stops the status; other errors retry later.
    pub(crate) async fn confirm_progress(
        &self,
        id: &str,
        worker: &str,
        revision: i64,
        outcome: &Result<String>,
        now: i64,
    ) -> Result<()> {
        let mut db = self.db.lock().await?;
        let now = Self::clock(&mut db, now).await?;
        let updated = match outcome {
            Ok(event) => sql_query("UPDATE hagency_agent_v1.progress_status SET status_event_id=coalesce(status_event_id,$3),sent_revision=greatest(sent_revision,$4),last_sent_ms=$5,worker_token=NULL,worker_until_ms=0,state=CASE WHEN state='final' AND greatest(sent_revision,$4)>=revision THEN 'done' ELSE state END WHERE id=$1 AND worker_token=$2")
                .bind::<Text, _>(id).bind::<Text, _>(worker).bind::<Text, _>(event).bind::<BigInt, _>(revision).bind::<BigInt, _>(now)
                .execute(&mut *db).await?,
            Err(Error::Conflict("matrix_permission_missing")) => sql_query("UPDATE hagency_agent_v1.progress_status SET state='cancelled',worker_token=NULL WHERE id=$1 AND worker_token=$2")
                .bind::<Text, _>(id).bind::<Text, _>(worker).execute(&mut *db).await?,
            Err(_) => sql_query("UPDATE hagency_agent_v1.progress_status SET worker_token=NULL,worker_until_ms=$3 WHERE id=$1 AND worker_token=$2")
                .bind::<Text, _>(id).bind::<Text, _>(worker).bind::<BigInt, _>(now + 5000).execute(&mut *db).await?,
        };
        if updated == 0 {
            return Err(Error::Conflict("stale_progress_worker"));
        }
        Ok(())
    }
}

/// One worker pass: post or edit every due status. Errors stay per status.
pub(crate) async fn send_progress(
    t: &TransportStore,
    g: &Gateway,
    m: &MatrixClient,
    cursor: &mut String,
    ctx: &crate::worker_runtime::QueueContext,
) -> Result<()> {
    let mut unavailable_rooms = BTreeSet::new();
    for candidate in t.progress_candidates(cursor, crate::api::now_ms()).await? {
        if ctx.stopping() {
            break;
        }
        *cursor = candidate.id.clone();
        let scope = match t.binding_scope(&candidate.binding_id).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        if unavailable_rooms.contains(&scope.room_id) {
            continue;
        }
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            g.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &candidate.requester_mxid,
                &scope.puppet_mxid,
            ),
        )
        .await;
        let Ok(Ok(f)) = observed else {
            unavailable_rooms.insert(scope.room_id.clone());
            continue;
        };
        let status = match t
            .claim_progress(&candidate.id, &f, crate::api::now_ms())
            .await
        {
            Ok(status) => status,
            Err(Error::Conflict(_) | Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        let worker = status
            .worker_token
            .clone()
            .ok_or(Error::Unavailable("missing_sender_claim"))?;
        let (txn, content) = status_content(scope.space_id.is_empty(), &status);
        let outcome = m
            .send(&scope.puppet_mxid, &scope.room_id, &txn, content)
            .await;
        if let Err(error) = &outcome {
            tracing::warn!(stage = "progress_status", status = %status.id, %error, "Status post or edit failed");
        }
        t.confirm_progress(
            &status.id,
            &worker,
            status.revision,
            &outcome,
            crate::api::now_ms(),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "progress_status_tests.rs"]
mod tests;
