use super::*;
use crate::management_shared_session_types::{SharedSessionRoute, SharedSessionRouteInput};

fn input(
    delivery: &crate::management_shared_attempt_types::SharedAttemptDelivery,
) -> SharedSessionRouteInput {
    SharedSessionRouteInput {
        command_digest: delivery.command_digest.clone(),
        token: delivery.token.clone(),
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_session_route_is_read_only_and_requires_original_execute_ticket() {
    let (f, request, admin, assignment, matrix) = Box::pin(setup()).await;
    let (original, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &original, 201).await;
    retain(&f, &request, &matrix, "session_route").await;
    let offered = demand(&f).await;
    let start = start_input(&offered, &original, &stopped);
    let delivery = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &offered.offer.demand_id,
            start.clone(),
            &matrix.gateway,
        )
        .await
        .unwrap();
    let id = &delivery.command.command_id;
    let replay = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &offered.offer.demand_id,
            start,
            &matrix.gateway,
        )
        .await
        .unwrap();
    assert!(!replay.execution_allowed);
    for actor in [&f.outsider, &f.admin, &f.coordinator, &admin] {
        assert!(
            f.domain
                .shared_native_session_route(
                    &actor.p,
                    &f.policy,
                    id,
                    input(&delivery),
                    &matrix.gateway
                )
                .await
                .is_err()
        );
    }
    for bad in [
        input(&replay),
        SharedSessionRouteInput {
            token: "0".repeat(64),
            ..input(&delivery)
        },
        SharedSessionRouteInput {
            command_digest: "0".repeat(64),
            ..input(&delivery)
        },
    ] {
        assert!(
            f.domain
                .shared_native_session_route(&f.owner.p, &f.policy, id, bad, &matrix.gateway)
                .await
                .is_err()
        );
    }
    let service = svc(&f, matrix.gateway.clone());
    let path = format!("shared-execution/runtime/{id}/session-route");
    let mut extra = json!(input(&delivery));
    extra["requesterMxid"] = json!(f.owner.p.mxid);
    post(
        &service,
        &path,
        &f.owner.token,
        extra,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let answer = post(
        &service,
        &path,
        &f.owner.token,
        json!(input(&delivery)),
        StatusCode::OK,
    )
    .await;
    let route: SharedSessionRoute = serde_json::from_value(answer["route"].clone()).unwrap();
    route.validate().unwrap();
    assert_eq!(route.event_id, offered.offer.event_id);
    assert_eq!(route.assignment_id, assignment.id);
    assert_eq!(route.requester_mxid, f.outsider.p.mxid);
    assert_eq!(route.thread_root, route.event_id);
    assert!(!route.encrypted);
    assert_eq!(
        f.domain
            .shared_native_session_route(
                &f.owner.p,
                &f.policy,
                id,
                input(&delivery),
                &matrix.gateway
            )
            .await
            .unwrap(),
        Some(route.clone())
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT r.state='issued' AND r.process_digest IS NULL AND r.activity_epoch=0 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=r.id) AS matched FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=$1")
        .bind::<Text,_>(id).get_result::<Check>(&mut db).await.unwrap().matched);
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            id,
            attempts::result(&delivery, "qualified", 203),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let work = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &matrix.gateway)
        .await
        .unwrap()
        .unwrap();
    assert!(route.matches_work(&work));
    assert!(
        f.domain
            .shared_native_session_route(
                &f.owner.p,
                &f.policy,
                id,
                input(&delivery),
                &matrix.gateway
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_session_route_rechecks_control_and_authority_after_matrix_wait() {
    for replace in [false, true] {
        let (f, request, _admin, assignment, matrix) = Box::pin(setup()).await;
        let (original, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &original, 201).await;
        retain(
            &f,
            &request,
            &matrix,
            &format!("session_route_recheck_{replace}"),
        )
        .await;
        let offered = demand(&f).await;
        let delivery = f
            .domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &offered.offer.demand_id,
                start_input(&offered, &original, &stopped),
                &matrix.gateway,
            )
            .await
            .unwrap();
        let (gateway, entered, release) = intake_tests::delayed_matrix(&matrix);
        let pending = f.domain.shared_native_session_route(
            &f.owner.p,
            &f.policy,
            &delivery.command.command_id,
            input(&delivery),
            &gateway,
        );
        let invalidate = async {
            entered.notified().await;
            if replace {
                invalidate_authority(&f, &request.project_id).await;
            } else {
                f.domain
                    .change_shared_run_control(
                        &f.owner.p,
                        &f.policy,
                        &assignment.id,
                        intake_tests::change("session_route_stop", 2, "paused"),
                        &matrix.gateway,
                    )
                    .await
                    .unwrap();
            }
            release.close();
        };
        let (result, ()) = tokio::join!(pending, invalidate);
        assert!(result.is_err());
    }
}
