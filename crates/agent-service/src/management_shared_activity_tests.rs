//! The one-shot spawn window must not become an idle lifetime limit.
use super::*;

pub(super) async fn renew_past_spawn(
    f: &Fixture,
    m: &MatrixFixture,
    mut activity: SharedActivityLease,
    spawn_expiry: i64,
) -> SharedActivityLease {
    let service = svc(f, m.gateway.clone());
    let digest = activity.command_digest.clone();
    let process = activity.process_digest.clone();
    let mut renewals = 0;
    while now_ms() <= spawn_expiry {
        // Exercise real elapsed time and the real HTTP/Gateway/PG boundary.
        // Each new lease is still bounded to 30s; none reissues the spawn.
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        let input = SharedActivityRenewInput {
            authority: activity.authority.clone(),
            renewal_id: format!(
                "idle_renew_{}_{}",
                activity.authority.runtime_command_id, renewals
            ),
        };
        let response = post(
            &service,
            "shared-execution/activity/renew",
            &f.owner.token,
            serde_json::to_value(&input).unwrap(),
            StatusCode::OK,
        )
        .await;
        let next: SharedActivityLease =
            serde_json::from_value(response["activity"].clone()).unwrap();
        assert_eq!(next.authority.epoch, activity.authority.epoch + 1);
        assert_eq!(next.command_digest, digest);
        assert_eq!(next.process_digest, process);
        assert!(next.expires_at_ms > now_ms());
        assert!(next.expires_at_ms <= response["observedAtMs"].as_i64().unwrap() + 30_000);
        let replay = post(
            &service,
            "shared-execution/activity/renew",
            &f.owner.token,
            serde_json::to_value(input).unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            replay["activity"], response["activity"],
            "exact replay cannot extend activity"
        );
        activity = next;
        renewals += 1;
    }
    assert!(
        renewals >= 2,
        "the test must exercise successive bounded heartbeats"
    );
    activity
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_v1_idle_renewal_survives_original_spawn_deadline() {
    let (f, request, _admin, _assignment, m) = Box::pin(prepared()).await;
    let (delivery, activity) = qualify(&f).await;
    let original_bytes = serde_json::to_vec(&delivery.command).unwrap();
    let activity = renew_past_spawn(&f, &m, activity, delivery.command.expires_at_ms).await;
    let observed = f
        .domain
        .shared_execution_status(&f.outsider.p, &f.policy, &request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.state, "qualified");
    assert!(observed.work_available);
    assert!(!observed.capacity_released);
    assert_eq!(
        observed.activity_expires_at_ms,
        Some(activity.expires_at_ms)
    );
    let original = history(&f, &delivery).await;
    assert_eq!(
        serde_json::to_vec(&original.command).unwrap(),
        original_bytes
    );
    assert!(!original.execution_allowed);
    assert!(!original.cleanup_allowed);
    assert_eq!(original.recorded.as_ref().unwrap().state, "qualified");
    let queued = work::queued(&f, &request, &m, &activity, "after_initial_spawn_window").await;
    assert_eq!(queued.runtime_command_digest, delivery.command_digest);
}

/// What the session endpoints record for a client that follows the
/// authorization window it was granted.
async fn follow_window(f: &Fixture, window_ms: i64) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let changed =
        sql_query("UPDATE hagency_agent_v1.sessions SET activity_window_ms=$1 WHERE id=$2")
            .bind::<BigInt, _>(window_ms)
            .bind::<Text, _>(&f.owner.p.session_id)
            .execute(&mut db)
            .await
            .unwrap();
    assert_eq!(changed, 1);
}
/// The end kept for the first lease of a runtime, and how many rows keep one.
async fn first_lease_end(f: &Fixture, runtime: &str) -> (i64, i64) {
    #[derive(diesel::QueryableByName)]
    struct Kept {
        #[diesel(sql_type=BigInt)]
        ends: i64,
        #[diesel(sql_type=BigInt)]
        rows: i64,
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let kept = sql_query("SELECT coalesce(max(expires_at_ms),0)::bigint AS ends,count(*)::bigint AS rows FROM hagency_agent_v1.management_shared_activity_renewals WHERE runtime_id=$1 AND from_epoch=0")
        .bind::<Text, _>(runtime)
        .get_result::<Kept>(&mut db)
        .await
        .unwrap();
    (kept.ends, kept.rows)
}
async fn renew_activity(
    f: &Fixture,
    m: &MatrixFixture,
    activity: &SharedActivityLease,
    id: &str,
) -> (SharedActivityLease, i64) {
    let input = SharedActivityRenewInput {
        authority: activity.authority.clone(),
        renewal_id: format!("{id}_{}", activity.authority.runtime_command_id),
    };
    let response = post(
        &svc(f, m.gateway.clone()),
        "shared-execution/activity/renew",
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    (
        serde_json::from_value(response["activity"].clone()).unwrap(),
        response["observedAtMs"].as_i64().unwrap(),
    )
}

/// What a host holds follows its session's window, and what it is offered
/// does not. Under a followed two-minute window the start window is thirty
/// seconds as ever, and the first lease and each renewal last as long as the
/// window. The first lease's end is kept, as a renewal's is.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_lasts_as_long_as_a_followed_session_window() {
    let (f, _request, _admin, _assignment, m) = Box::pin(prepared()).await;
    follow_window(&f, 120_000).await;
    let (delivery, activity) = qualify(&f).await;
    let start = delivery.command.expires_at_ms - delivery.command.issued_at_ms;
    assert!((1..=30_000).contains(&start), "{start}");
    assert!(delivery.lease_until_ms <= delivery.command.expires_at_ms);
    let first = activity.expires_at_ms - now_ms();
    assert!((30_001..=120_000).contains(&first), "{first}");
    assert_eq!(
        first_lease_end(&f, &delivery.command.command_id).await,
        (activity.expires_at_ms, 1)
    );
    let (next, observed) = renew_activity(&f, &m, &activity, "followed_renew").await;
    let renewed = next.expires_at_ms - observed;
    assert!((30_001..=120_000).contains(&renewed), "{renewed}");
    // The receipt sent again is answered with the lease in hand: the same
    // end, and no second first lease.
    let again = save(&f, &delivery, "qualified", 200)
        .await
        .unwrap()
        .activity
        .unwrap();
    assert_eq!(again.expires_at_ms, next.expires_at_ms);
    assert_eq!(
        first_lease_end(&f, &delivery.command.command_id).await,
        (activity.expires_at_ms, 1)
    );
}

/// A session that does not follow a window is an older client. Its first
/// lease and its renewals are thirty seconds from now, as they always were,
/// and the first one's end is kept too.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_keeps_thirty_seconds_for_a_session_that_follows_no_window() {
    let (f, _request, _admin, _assignment, m) = Box::pin(prepared()).await;
    let (delivery, activity) = qualify(&f).await;
    let first = activity.expires_at_ms - now_ms();
    assert!((1..=30_000).contains(&first), "{first}");
    assert!(activity.expires_at_ms <= delivery.command.expires_at_ms + 30_000);
    assert_eq!(
        first_lease_end(&f, &delivery.command.command_id).await,
        (activity.expires_at_ms, 1)
    );
    let (next, observed) = renew_activity(&f, &m, &activity, "default_renew").await;
    assert!((1..=30_000).contains(&(next.expires_at_ms - observed)));
}

/// One session, and its window is made shorter while its process works (the
/// host lowered its setting). A renewal does not end before the lease it
/// renews: the host holds that end already. This is the same session
/// throughout; under another session that ends sooner, the lease ends with
/// that session, as every lease does.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_lease_is_not_cut_when_the_window_of_its_session_is_made_shorter() {
    let (f, _request, _admin, _assignment, m) = Box::pin(prepared()).await;
    follow_window(&f, 120_000).await;
    let (_delivery, activity) = qualify(&f).await;
    assert!(activity.expires_at_ms - now_ms() > 60_000);
    follow_window(&f, 60_000).await;
    let (shorter, observed) = renew_activity(&f, &m, &activity, "shorter_renew").await;
    assert!(shorter.expires_at_ms >= activity.expires_at_ms);
    assert!(shorter.expires_at_ms - observed <= 120_000);
}

/// A renewal of the session without the header: an older client took the
/// session over. It checks each lease against thirty seconds and would
/// refuse a longer one, so its next lease is thirty seconds from now, as it
/// always was, also when that is before the lease in hand. The shorter end
/// is meant: the longer one is of no use to a client that cannot take it.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_lease_is_thirty_seconds_from_now_once_its_session_follows_no_window() {
    let (f, _request, _admin, _assignment, m) = Box::pin(prepared()).await;
    follow_window(&f, 120_000).await;
    let (_delivery, activity) = qualify(&f).await;
    assert!(activity.expires_at_ms - now_ms() > 60_000);
    follow_window(&f, 30_000).await;
    let (older, observed) = renew_activity(&f, &m, &activity, "older_renew").await;
    assert!((1..=30_000).contains(&(older.expires_at_ms - observed)));
    assert!(older.expires_at_ms < activity.expires_at_ms);
}
