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
