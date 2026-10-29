use super::*;
use crate::management_shared_intake_types::*;

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    demand_tests::remove_migration(db).await;
    db.batch_execute("DROP TRIGGER guard_shared_pause ON hagency_agent_v1.management_shared_runtimes;DROP TABLE hagency_agent_v1.management_shared_control_commands,hagency_agent_v1.management_shared_intake,hagency_agent_v1.management_shared_controls;DROP FUNCTION hagency_agent_v1.guard_shared_pause(),hagency_agent_v1.apply_shared_pause(),hagency_agent_v1.retain_shared_intake();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=15;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=15)").await.unwrap();
}
pub(super) fn change(command: &str, revision: i64, mode: &str) -> SharedControlInput {
    SharedControlInput {
        command_id: command.into(),
        expected_revision: revision,
        mode: mode.into(),
    }
}
pub(super) async fn arm(f: &Fixture, a: &PendingManagedAssignment, m: &MatrixFixture) -> SharedControlResult {
    f.domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &a.id,
            change("arm", 1, "running"),
            &m.gateway,
        )
        .await
        .unwrap()
}
async fn rows(f: &Fixture, a: &PendingManagedAssignment) -> SharedIntakePage {
    f.domain
        .shared_intake_history(&f.outsider.p, &a.id, SharedIntakeQuery { after: None })
        .await
        .unwrap()
}
async fn retain(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    event: &str,
) -> crate::Result<()> {
    let event = work::message(f, r, event).await;
    // The database is the admission clock. Host/container clock skew must not
    // make a fixture's freshly received event appear to come from the future.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let received = sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
        .get_result::<Count>(&mut db).await.unwrap().count;
    f.domain
        .route_shared_event(&f.policy, &event, received, &m.gateway)
        .await
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_retains_without_process_after_natural_stop_and_exact_replay() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    assert_eq!(
        f.domain
            .shared_run_control(&f.owner.p, &a.id)
            .await
            .unwrap()
            .mode,
        "manual"
    );
    let armed = arm(&f, &a, &m).await;
    let e = work::message(&f, &r, "offline_original").await;
    let received = now_ms();
    for _ in 0..2 {
        f.domain
            .route_shared_event(&f.policy, &e, received, &m.gateway)
            .await
            .unwrap();
    }
    let page = rows(&f, &a).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, "pending");
    assert_eq!(page.items[0].received_at_ms, received);
    let repeated = f
        .domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &a.id,
            change("arm_again", armed.control.revision, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(repeated.control, armed.control);
    let mut altered = e.clone();
    altered.body.push('!');
    assert!(
        f.domain
            .route_shared_event(&f.policy, &altered, received, &m.gateway)
            .await
            .is_err()
    );
    let (d, _) = qualify(&f).await;
    save(&f, &d, "stopped", 201).await.unwrap();
    retain(&f, &r, &m, "after_natural_stop").await.unwrap();
    assert_eq!(rows(&f, &a).await.items.len(), 2);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1) AS matched").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert_eq!(history(&f, &d).await.command_digest, d.command_digest);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_roles_current_admin_http_cas_and_flag_off_stop() {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    for p in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert!(
            f.domain
                .change_shared_run_control(
                    p,
                    &f.policy,
                    &a.id,
                    change("wrong_role", 1, "running"),
                    &m.gateway
                )
                .await
                .is_err()
        );
    }
    let service = svc(&f, m.gateway.clone());
    let path = format!("shared-execution/assignments/{}/control", a.id);
    post(&service,&path,&f.outsider.token,json!({"commandId":"extra","expectedRevision":1,"mode":"running","ownerMxid":f.owner.p.mxid}),StatusCode::BAD_REQUEST).await;
    let response = post(
        &service,
        &path,
        &admin.token,
        json!({"commandId":"admin_start","expectedRevision":1,"mode":"running"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["control"]["origin"], "manager");
    retain(&f, &r, &m, "before_explicit_stop").await.unwrap();
    let (d, activity) = qualify(&f).await;
    let off = f
        .policy
        .clone()
        .with_shared_durable_intake(false)
        .with_shared_execution(false)
        .with_shared_successors(false);
    let stopped = f
        .domain
        .change_shared_run_control(
            &f.owner.p,
            &off,
            &a.id,
            change("owner_stop", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(stopped.control.origin, "executor_stop");
    assert_eq!(
        rows(&f, &a).await.items[0].reason.as_deref(),
        Some("explicit_stop")
    );
    assert!(
        f.domain
            .renew_shared_activity(
                &f.owner.p,
                &f.policy,
                SharedActivityRenewInput {
                    authority: activity.authority,
                    renewal_id: "after_stop".into()
                },
                &m.gateway
            )
            .await
            .is_err()
    );
    let cleanup = shared_poll(&f).await;
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
    assert_eq!(cleanup.command_digest, d.command_digest);
    retain(&f, &r, &m, "while_paused").await.unwrap();
    assert!(
        rows(&f, &a)
            .await
            .items
            .iter()
            .any(|r| r.reason.as_deref() == Some("paused_at_receipt"))
    );
    let replay = post(
        &service,
        &path,
        &admin.token,
        json!({"commandId":"admin_start","expectedRevision":1,"mode":"running"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay["appliedRevision"], 2);
    assert_eq!(replay["control"]["mode"], "paused");
    post(
        &service,
        &path,
        &admin.token,
        json!({"commandId":"admin_start","expectedRevision":3,"mode":"running"}),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &off,
                &a.id,
                change("disabled_start", 3, "running"),
                &m.gateway
            )
            .await
            .is_err()
    );
    let restarted = f
        .domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &a.id,
            change("manager_restart", 3, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(restarted.control.revision, 4);
    // Original cancelled events stay terminal; Start neither replays nor creates a process.
    assert_eq!(
        rows(&f, &a)
            .await
            .items
            .iter()
            .filter(|x| x.state == "pending")
            .count(),
        0
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_new_approved_assignment_arms_without_owner_per_agent_vote() {
    let (mut f, first, admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    let (second, a) = Box::pin(work::another_assignment(&f, &first, &admin, &m, 2)).await;
    let control = f
        .domain
        .shared_run_control(&f.owner.p, &a.id)
        .await
        .unwrap();
    assert_eq!(control.mode, "running");
    assert_eq!(control.origin, "approved_native_assignment");
    assert_eq!(control.revision, 1);
    retain(&f, &second, &m, "new_assignment_message")
        .await
        .unwrap();
    assert_eq!(rows(&f, &a).await.items[0].state, "pending");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_filters_authority_expiry_and_preserves_reason() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    retain(&f, &r, &m, "manual").await.unwrap();
    assert_eq!(
        rows(&f, &a).await.items[0].reason.as_deref(),
        Some("manual_start_required")
    );
    arm(&f, &a, &m).await;
    let e = work::message(&f, &r, "expired").await;
    f.domain
        .route_shared_event(&f.policy, &e, now_ms() - 86_400_001, &m.gateway)
        .await
        .unwrap();
    for (i, kind) in ["foreign", "encrypted", "edit", "mention", "long"]
        .iter()
        .enumerate()
    {
        let mut e = work::message(&f, &r, &format!("ignored_{i}")).await;
        match *kind {
            // Joined members may mention the Agent; someone outside the Room may not.
            "foreign" => e.sender_mxid = "@stranger:example.test".into(),
            "encrypted" => e.encrypted = true,
            "edit" => e.is_edit = true,
            "mention" => e.mentioned_mxids.clear(),
            _ => e.body = "x".repeat(4097),
        };
        f.domain
            .route_shared_event(&f.policy, &e, now_ms(), &m.gateway)
            .await
            .unwrap();
    }
    assert_eq!(rows(&f, &a).await.items.len(), 2);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.devices SET generation=generation+1 WHERE id=$1")
        .bind::<Text, _>(f.owner.p.device_id.as_deref().unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    retain(&f, &r, &m, "replaced_executor").await.unwrap();
    let page = rows(&f, &a).await;
    assert!(
        page.items
            .iter()
            .any(|r| r.reason.as_deref() == Some("event_expired"))
    );
    assert!(
        page.items
            .iter()
            .any(|r| r.reason.as_deref() == Some("assignment_authority_changed"))
    );
    assert!(
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("stale_authority", 2, "running"),
                &m.gateway
            )
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_appservice_full_target_retries_siblings_once_and_expires_pending() {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let (second, b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let mut e = work::message(&f, &r, "batch_both").await;
    e.mentioned_mxids
        .extend(work::message(&f, &second, "unused").await.mentioned_mxids);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let expiry = now_ms() + 2500;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_intake(id,assignment_id,event_id,event_payload,event_digest,control_revision,received_at_ms,expires_at_ms,state,reason) SELECT $1||n,$2,'$old_'||n,'{}',repeat('a',64),2,$3,$4,'pending',NULL FROM generate_series(1,100) n").bind::<Text,_>(format!("fill_{}_",a.id)).bind::<Text,_>(&a.id).bind::<BigInt,_>(expiry-86_400_000).bind::<BigInt,_>(expiry).execute(&mut db).await.unwrap();
    let hs = "fixture-only-durable-intake-hs-token";
    let inbox = crate::appservice::Inbox::open(&f.url, hs.into())
        .await
        .unwrap();
    let service = Service::new(inbox.router());
    let txn = format!("zz_intake_{}", crate::entity_id().unwrap());
    let raw = json!({"events":[{"type":"m.room.message","event_id":e.event_id,"room_id":e.room_id,"sender":e.sender_mxid,"content":{"msgtype":"m.text","body":e.body,"m.mentions":{"user_ids":e.mentioned_mxids}}}]});
    for _ in 0..2 {
        let response = Box::pin(
            TestClient::put(format!(
                "https://example.test/_matrix/app/v1/transactions/{txn}"
            ))
            .bearer_auth(hs)
            .json(&raw)
            .send(&service),
        )
        .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
    }
    let transport =
        crate::transport::TransportStore::open(&f.url, crate::transport::Limits::default())
            .await
            .unwrap();
    let context = (f.domain.clone(), f.policy.clone());
    for _ in 0..2 {
        let mut cursor = "zz_intake_".to_string();
        Box::pin(crate::workers::route_with_shared(
            &inbox,
            &transport,
            &m.gateway,
            &mut cursor,
            Some(&context),
        ))
        .await
        .unwrap();
    }
    assert_eq!(rows(&f, &b).await.items.len(), 1);
    assert!(inbox.pending().await.unwrap().iter().any(|v| v.id == txn));
    tokio::time::sleep(std::time::Duration::from_millis(
        (expiry - now_ms() + 20).max(1) as u64,
    ))
    .await;
    let mut cursor = "zz_intake_".to_string();
    Box::pin(crate::workers::route_with_shared(
        &inbox,
        &transport,
        &m.gateway,
        &mut cursor,
        Some(&context),
    ))
    .await
    .unwrap();
    assert!(!inbox.pending().await.unwrap().iter().any(|v| v.id == txn));
    assert_eq!(rows(&f, &b).await.items.len(), 1);
    assert!(sql_query("SELECT count(*)=101 AND count(*) FILTER(WHERE state='expired' AND reason='event_expired')=100 AND count(*) FILTER(WHERE state='pending')=1 AS matched FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    let mut after = None;
    let mut count = 0;
    loop {
        let page = f
            .domain
            .shared_intake_history(&f.outsider.p, &a.id, SharedIntakeQuery { after })
            .await
            .unwrap();
        assert!(page.items.len() <= 16);
        count += page.items.len();
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(count, 101);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_schema15_atomic_upgrade_keeps_original_runtime_and_manual_intent() {
    let (f, _r, _admin, a, _m) = Box::pin(prepared()).await;
    let (d, _) = qualify(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_intake(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=15 AND to_regclass('hagency_agent_v1.management_shared_controls') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_intake")
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let c = reopened
        .shared_run_control(&f.outsider.p, &a.id)
        .await
        .unwrap();
    assert_eq!(c.mode, "manual");
    assert_eq!(c.origin, "migration_unknown");
    assert_eq!(history(&f, &d).await.command_digest, d.command_digest);
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.management_shared_controls WHERE assignment_id=$1")
            .bind::<Text, _>(&a.id)
            .execute(&mut db)
            .await
            .is_err()
    );
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_controls SET mode='running',running_since_ms=1 WHERE assignment_id=$1").bind::<Text,_>(&a.id).execute(&mut db).await.is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_more_than32_targets_progresses_past_full_first_page() {
    let (mut f, first, admin, first_assignment, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &first_assignment, &m).await;
    let mut event = work::message(&f, &first, "all_34_targets").await;
    let mut ids = vec![first_assignment.id.clone()];
    for n in 2..=34 {
        let (request, assignment) =
            Box::pin(work::another_assignment(&f, &first, &admin, &m, n)).await;
        event
            .mentioned_mxids
            .extend(work::message(&f, &request, "unused").await.mentioned_mxids);
        ids.push(assignment.id);
    }
    ids.sort();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Fill the entire first candidate page. Later targets must still commit.
    sql_query("INSERT INTO hagency_agent_v1.management_shared_intake(id,assignment_id,event_id,event_payload,event_digest,control_revision,received_at_ms,expires_at_ms,state,reason) SELECT a||'_'||n,a,'$capacity_'||n,'{}',repeat('a',64),1,$2,$3,'pending',NULL FROM unnest($1::text[]) a CROSS JOIN generate_series(1,100) n")
        .bind::<diesel::sql_types::Array<Text>,_>(&ids[..32])
        .bind::<BigInt,_>(now_ms()).bind::<BigInt,_>(now_ms()+86_400_000)
        .execute(&mut db).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            f.domain
                .route_shared_event(&f.policy, &event, now_ms(), &m.gateway)
                .await
                .unwrap_err()
                .to_string(),
            "shared_intake_retry_required"
        );
    }
    assert!(sql_query("SELECT count(*)=2 AND count(DISTINCT assignment_id)=2 AS matched FROM hagency_agent_v1.management_shared_intake WHERE event_id=$1")
        .bind::<Text,_>(&event.event_id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(sql_query("SELECT count(*)=3200 AS matched FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=ANY($1) AND event_id LIKE '$capacity_%'")
        .bind::<diesel::sql_types::Array<Text>,_>(&ids[..32]).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_manual_control_blocks_new_process_but_keeps_original_history() {
    let (mut f, _request, _admin, assignment, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    arm(&f, &assignment, &m).await;
    let (original, _) = qualify(&f).await;
    save(&f, &original, "stopped", 201).await.unwrap();
    f.domain
        .change_shared_run_control(
            &f.owner.p,
            &f.policy,
            &assignment.id,
            change("stop_no_restart", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    let replay = history(&f, &original).await;
    assert!(!replay.execution_allowed);
    assert_eq!(replay.recorded.unwrap().state, "stopped");
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}

pub(super) fn delayed_matrix(
    m: &MatrixFixture,
) -> (
    Gateway,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (notify, wait, data) = (entered.clone(), release.clone(), m.states.clone());
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let (notify, wait, data) = (notify.clone(), wait.clone(), data.clone());
            Box::pin(async move {
                notify.notify_one();
                let _ = wait.acquire().await;
                data.lock()
                    .unwrap()
                    .get(&room)
                    .cloned()
                    .ok_or(crate::Error::NotFound("fixture_room_missing"))
            })
        }),
        SERVICE.into(),
    );
    (gateway, entered, release)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_start_rechecks_stop_and_replaced_admin_after_matrix_await() {
    for replace in [false, true] {
        let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
        f.policy = f.policy.with_shared_durable_intake(true);
        let (delayed, entered, release) = delayed_matrix(&m);
        let pending = f.domain.change_shared_run_control(
            &admin.p,
            &f.policy,
            &a.id,
            change("start_in_flight", 1, "running"),
            &delayed,
        );
        let invalidate = async {
            entered.notified().await;
            if replace {
                invalidate_authority(&f, &r.project_id).await;
            } else {
                f.domain
                    .change_shared_run_control(
                        &f.owner.p,
                        &f.policy,
                        &a.id,
                        change("stop_during_run", 1, "paused"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
            }
            release.close();
        };
        let (result, _) = tokio::join!(pending, invalidate);
        assert!(result.is_err());
        let control = f
            .domain
            .shared_run_control(&f.outsider.p, &a.id)
            .await
            .unwrap();
        assert_eq!(control.mode, if replace { "manual" } else { "paused" });
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_control_commands WHERE assignment_id=$1 AND command_id='start_in_flight') AS matched").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_stop_during_event_observation_retains_terminal_event_once() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let event = work::message(&f, &r, "in_flight_at_stop").await;
    let received = now_ms();
    let (delayed, entered, release) = delayed_matrix(&m);
    let pending = f
        .domain
        .route_shared_event(&f.policy, &event, received, &delayed);
    let stop = async {
        entered.notified().await;
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("stop_inflight_event", 2, "paused"),
                &m.gateway,
            )
            .await
            .unwrap();
        release.close();
    };
    let (result, _) = tokio::join!(pending, stop);
    result.unwrap();
    f.domain
        .route_shared_event(&f.policy, &event, received, &m.gateway)
        .await
        .unwrap();
    let page = rows(&f, &a).await;
    assert_eq!(page.items.len(), 1);
    assert_ne!(page.items[0].state, "pending");
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_control_receipt_failure_rolls_back_pause_and_queue_cancellation() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    retain(&f, &r, &m, "pending_before_rollback").await.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.test_reject_control() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture_control_rollback';END;$$;CREATE TRIGGER test_reject_control BEFORE INSERT ON hagency_agent_v1.management_shared_control_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.test_reject_control()").await.unwrap();
    let result = f
        .domain
        .change_shared_run_control(
            &f.owner.p,
            &f.policy,
            &a.id,
            change("rollback_stop", 2, "paused"),
            &m.gateway,
        )
        .await;
    db.batch_execute("DROP TRIGGER test_reject_control ON hagency_agent_v1.management_shared_control_commands;DROP FUNCTION hagency_agent_v1.test_reject_control()").await.unwrap();
    assert!(result.is_err());
    assert_eq!(
        f.domain
            .shared_run_control(&f.outsider.p, &a.id)
            .await
            .unwrap()
            .mode,
        "running"
    );
    assert_eq!(rows(&f, &a).await.items[0].state, "pending");
    f.domain
        .change_shared_run_control(
            &f.owner.p,
            &f.policy,
            &a.id,
            change("rollback_stop", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(
        rows(&f, &a).await.items[0].reason.as_deref(),
        Some("explicit_stop")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_expired_event_finishes_during_matrix_outage_but_fresh_remains_retryable() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let unavailable = Gateway::new(
        Arc::new(|_| Box::pin(async { Err(crate::Error::Unavailable("fixture_matrix_outage")) })),
        SERVICE.into(),
    );
    let fresh = work::message(&f, &r, "fresh_during_outage").await;
    assert_eq!(
        f.domain
            .route_shared_event(&f.policy, &fresh, now_ms(), &unavailable)
            .await
            .unwrap_err()
            .to_string(),
        "shared_intake_retry_required"
    );
    assert!(rows(&f, &a).await.items.is_empty());
    let expired = work::message(&f, &r, "expired_during_outage").await;
    let received = now_ms() - 86_400_001;
    f.domain
        .route_shared_event(&f.policy, &expired, received, &unavailable)
        .await
        .unwrap();
    let page = rows(&f, &a).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].received_at_ms, received);
    assert_eq!(page.items[0].state, "expired");
    assert_eq!(page.items[0].reason.as_deref(), Some("event_expired"));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_pause_blocks_original_successor_route_until_manager_rearms() {
    let (mut f, _r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_shared_durable_intake(true)
        .with_shared_successors(true);
    arm(&f, &a, &m).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    f.domain
        .change_shared_run_control(
            &f.owner.p,
            &f.policy,
            &a.id,
            change("pause_successor", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    let input = attempts::start(&d, &stopped);
    assert_eq!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway)
            .await
            .unwrap_err()
            .to_string(),
        "shared_assignment_not_running"
    );
    f.domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &a.id,
            change("manager_rearms", 3, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
    let next = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    assert!(next.execution_allowed);
    assert_eq!(next.command.assignment_id, a.id);
    assert_eq!(history(&f, &d).await.recorded.unwrap().state, "stopped");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_retained_hard_cap_never_discards_audit_to_admit_new_event() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_shared_intake(id,assignment_id,event_id,event_payload,event_digest,control_revision,received_at_ms,expires_at_ms,state,reason) SELECT $1||n,$2,'$retained_'||n,'{}',repeat('a',64),1,1,2,'expired','event_expired' FROM generate_series(1,10000) n")
        .bind::<Text,_>(format!("retained_{}_",a.id)).bind::<Text,_>(&a.id).execute(&mut db).await.unwrap();
    assert_eq!(
        retain(&f, &r, &m, "after_retained_cap")
            .await
            .unwrap_err()
            .to_string(),
        "shared_intake_retry_required"
    );
    assert!(sql_query("SELECT count(*)=10000 AND count(*) FILTER(WHERE state='expired')=10000 AS matched FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1")
            .bind::<Text, _>(&a.id)
            .execute(&mut db)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_successor_start_cannot_cross_stop_then_run_during_matrix_await() {
    let (mut f, _r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_shared_durable_intake(true)
        .with_shared_successors(true);
    arm(&f, &a, &m).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    let input = attempts::start(&d, &stopped);
    let (delayed, entered, release) = delayed_matrix(&m);
    let pending = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input, &delayed);
    let change_run = async {
        entered.notified().await;
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("aba_stop", 2, "paused"),
                &m.gateway,
            )
            .await
            .unwrap();
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("aba_run", 3, "running"),
                &m.gateway,
            )
            .await
            .unwrap();
        release.close();
    };
    let (result, _) = tokio::join!(pending, change_run);
    assert!(
        result.is_err(),
        "old Start crossed a Stop/Run revision change"
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1) AS matched").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_event_observation_cannot_cross_stop_then_run() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let event = work::message(&f, &r, "event_crossing_stop_then_run").await;
    let received = now_ms();
    let (delayed, entered, release) = delayed_matrix(&m);
    let pending = f
        .domain
        .route_shared_event(&f.policy, &event, received, &delayed);
    let rearm = async {
        entered.notified().await;
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("event_aba_stop", 2, "paused"),
                &m.gateway,
            )
            .await
            .unwrap();
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                change("event_aba_run", 3, "running"),
                &m.gateway,
            )
            .await
            .unwrap();
        release.close();
    };
    let (result, _) = tokio::join!(pending, rearm);
    result.unwrap();
    f.domain
        .route_shared_event(&f.policy, &event, received, &m.gateway)
        .await
        .unwrap();
    let page = rows(&f, &a).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, "cancelled");
    assert_eq!(page.items[0].reason.as_deref(), Some("run_control_changed"));
    assert_eq!(page.items[0].control_revision, 4);
    assert_eq!(page.items[0].received_at_ms, received);
    assert_eq!(
        f.domain
            .shared_run_control(&f.outsider.p, &a.id)
            .await
            .unwrap()
            .mode,
        "running"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_intake_equal_millisecond_receive_floor_never_guesses_post_start_order() {
    let (mut f, r, _admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_durable_intake(true);
    arm(&f, &a, &m).await;
    let control = f
        .domain
        .shared_run_control(&f.outsider.p, &a.id)
        .await
        .unwrap();
    let event = work::message(&f, &r, "ambiguous_same_millisecond").await;
    let received = control.running_since_ms.unwrap();
    f.domain
        .route_shared_event(&f.policy, &event, received, &m.gateway)
        .await
        .unwrap();
    let page = rows(&f, &a).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].received_at_ms, received);
    assert_eq!(page.items[0].state, "cancelled");
    assert_eq!(
        page.items[0].reason.as_deref(),
        Some("before_running_intent")
    );
}
