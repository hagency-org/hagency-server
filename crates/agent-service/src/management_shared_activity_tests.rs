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
    let changed = sql_query("UPDATE hagency_agent_v1.sessions SET ticket_window_ms=$1 WHERE id=$2")
        .bind::<BigInt, _>(window_ms)
        .bind::<Text, _>(&f.owner.p.session_id)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(changed, 1);
}

/// One clock: the start window, the first lease and each renewal last as long
/// as the window of the session they are issued to, not thirty seconds flat.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_lasts_as_long_as_a_followed_session_window() {
    let (f, _request, _admin, _assignment, m) = Box::pin(prepared()).await;
    follow_window(&f, 120_000).await;
    let (delivery, activity) = qualify(&f).await;
    let start = delivery.command.expires_at_ms - delivery.command.issued_at_ms;
    assert!((30_001..=120_000).contains(&start), "{start}");
    let first = activity.expires_at_ms - now_ms();
    assert!((30_001..=120_000).contains(&first), "{first}");
    let service = svc(&f, m.gateway.clone());
    let input = SharedActivityRenewInput {
        authority: activity.authority.clone(),
        renewal_id: format!("followed_renew_{}", activity.authority.runtime_command_id),
    };
    let response = post(
        &service,
        "shared-execution/activity/renew",
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    let next: SharedActivityLease = serde_json::from_value(response["activity"].clone()).unwrap();
    let renewed = next.expires_at_ms - response["observedAtMs"].as_i64().unwrap();
    assert!((30_001..=120_000).contains(&renewed), "{renewed}");
    // The first lease ended within the run's own start window after it: the
    // bound the sweep and the executor read once the run is fenced.
    assert!(activity.expires_at_ms <= delivery.command.expires_at_ms + start);
}

/// A run issued with a thirty-second start window keeps thirty seconds for
/// its first lease, also when its session follows a longer window a moment
/// later: the bound read from the command does not move after it was issued.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_first_lease_keeps_the_window_its_run_was_issued_with() {
    let (f, _request, _admin, _assignment, _m) = Box::pin(prepared()).await;
    let delivery = shared_poll(&f).await;
    let start = delivery.command.expires_at_ms - delivery.command.issued_at_ms;
    assert!((1..=30_000).contains(&start), "{start}");
    follow_window(&f, 120_000).await;
    let activity = save(&f, &delivery, "qualified", 200)
        .await
        .unwrap()
        .activity
        .unwrap();
    let first = activity.expires_at_ms - now_ms();
    assert!((1..=30_000).contains(&first), "{first}");
    assert!(activity.expires_at_ms <= delivery.command.expires_at_ms + 30_000);
}
