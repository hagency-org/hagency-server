//! Current worker declaration, separate from business and process authority.
//! Missing workers leave immutable intake pending; only its presentation is
//! blocked. A later current poll can consume that same event under every normal
//! permission, room, generation, Run revision, budget and expiry check.
use super::*;
use diesel::{OptionalExtension, QueryableByName};

pub(crate) const HEADER: &str = "x-hagency-shared-protocol";
pub(crate) const CURRENT: &str = "native-context-v3;claude-v4";
const FRESH_MS: i64 = 300_000;

/// true: new worker admission; false: activity of already admitted work.
/// History, settlement, cleanup and GUI projections do not need a declaration
/// and cannot make an offline executor appear ready.
pub(crate) fn worker_route(parts: &[&str]) -> Option<bool> {
    match parts {
        [
            "shared-execution",
            "runtime" | "attempts" | "demands" | "budget-attempts" | "budget-demands",
            "poll" | "poll-v4" | "poll-v5",
        ]
        | ["shared-execution", "attempts" | "budget-attempts", "start"]
        | ["shared-execution", "demands" | "budget-demands", _, "start"]
        | [
            "shared-execution",
            "work" | "budget-work",
            "poll" | "ack" | "start",
        ]
        | [
            "shared-execution",
            "work" | "budget-work",
            _,
            "context" | "input-mode",
        ]
        | [
            "shared-execution",
            "work" | "budget-work",
            _,
            "context",
            "start",
        ]
        | [
            "shared-execution",
            "crypto",
            "poll" | "device" | "thread" | "confirm",
        ] => Some(true),
        ["shared-execution", "activity" | "budget-activity", "renew"] => Some(false),
        _ => None,
    }
}

#[derive(QueryableByName)]
struct Reason {
    #[diesel(sql_type=Nullable<Text>)]
    reason: Option<String>,
}
fn named(reason: &str) -> Error {
    Error::Conflict(if reason == "owner_daemon_unavailable" {
        "owner_daemon_unavailable"
    } else {
        "owner_daemon_update_required"
    })
}
pub(crate) async fn require(db: &mut AsyncPgConnection, p: &Principal, now: i64) -> Result<()> {
    let device = p
        .device_id
        .as_deref()
        .ok_or(Error::Unauthorized("device_required"))?;
    let generation = p
        .device_generation
        .ok_or(Error::Unauthorized("device_required"))?;
    let row =
        sql_query("SELECT hagency_agent_v1.shared_executor_protocol_reason($1,$2,$3,$4) AS reason")
            .bind::<Text, _>(&p.user_id)
            .bind::<Text, _>(device)
            .bind::<BigInt, _>(generation)
            .bind::<BigInt, _>(now)
            .get_result::<Reason>(db)
            .await?;
    if let Some(reason) = row.reason {
        return Err(named(&reason));
    }
    Ok(())
}
pub(crate) async fn assignment_reason(
    db: &mut AsyncPgConnection,
    assignment: &str,
    now: i64,
) -> Result<Option<String>> {
    let row = sql_query("SELECT hagency_agent_v1.shared_executor_protocol_reason(executor_owner_user_id,executor_device_id,executor_generation,$2) AS reason FROM hagency_agent_v1.management_shared_assignments WHERE id=$1")
        .bind::<Text,_>(assignment).bind::<BigInt,_>(now)
        .get_result::<Reason>(db).await.optional()?;
    Ok(row.and_then(|row| row.reason))
}
impl DomainStore {
    /// Only called after device authentication, for actual worker requests.
    /// An obsolete worker invalidates its former declaration even if a newer
    /// worker used the same installation recently. This grants no work.
    pub(crate) async fn observe_shared_daemon_protocol(
        &self,
        p: &Principal,
        qualified: bool,
    ) -> Result<()> {
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db,p,0).await?;
            let device = p.device_id.as_deref().ok_or(Error::Unauthorized("device_required"))?;
            let generation = p.device_generation.ok_or(Error::Unauthorized("device_required"))?;
            #[derive(QueryableByName)] struct Clock { #[diesel(sql_type=BigInt)] now:i64 }
            let now=sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now").get_result::<Clock>(db).await?.now;
            sql_query("INSERT INTO hagency_agent_v1.management_shared_executor_protocols(device_id,owner_user_id,generation,session_id,qualified,observed_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(device_id) DO UPDATE SET owner_user_id=excluded.owner_user_id,generation=excluded.generation,session_id=excluded.session_id,qualified=excluded.qualified,observed_at_ms=excluded.observed_at_ms,expires_at_ms=excluded.expires_at_ms")
                .bind::<Text,_>(device).bind::<Text,_>(&p.user_id).bind::<BigInt,_>(generation)
                .bind::<Text,_>(&p.session_id).bind::<Bool,_>(qualified).bind::<BigInt,_>(now)
                .bind::<BigInt,_>(now+FRESH_MS).execute(db).await?;
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_protocol_guards_admission_but_not_cleanup_or_readers() {
        for route in [
            "runtime/poll",
            "runtime/poll-v4",
            "runtime/poll-v5",
            "attempts/start",
            "demands/d/start",
            "budget-demands/poll-v4",
            "budget-demands/poll-v5",
            "work/ack",
            "budget-work/ack",
            "work/w/context/start",
            "crypto/poll",
        ] {
            let path = format!("shared-execution/{route}");
            assert_eq!(
                worker_route(&path.split('/').collect::<Vec<_>>()),
                Some(true),
                "{route}"
            );
        }
        for route in [
            "runtime/r/result",
            "runtime/r/recover",
            "work/w/result",
            "budget-work/w/result",
            "work/w/recover",
            "work/w/reply/observe",
            "approvals/query",
            "assignments/a/control",
            "assignments/a/intake/query",
        ] {
            let path = format!("shared-execution/{route}");
            assert_eq!(
                worker_route(&path.split('/').collect::<Vec<_>>()),
                None,
                "{route}"
            );
        }
        assert_eq!(
            worker_route(&["shared-execution", "activity", "renew"]),
            Some(false)
        );
    }
}
