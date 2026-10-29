use super::*;
use crate::management_shared_demand_types::*;
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    context_tests::remove_migration(db).await;
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_demand_bindings,hagency_agent_v1.management_shared_demand_starts,hagency_agent_v1.management_shared_demand_blocks,hagency_agent_v1.management_shared_demands;DROP FUNCTION hagency_agent_v1.validate_shared_demand(),hagency_agent_v1.retain_shared_demand();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=16;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=16)").await.unwrap();
    // This disposable test DB can contain newer rows from earlier test cases.
    // Remove only schema17-only intake states before restoring schema16 checks.
    db.batch_execute("ALTER TABLE hagency_agent_v1.management_shared_intake DISABLE TRIGGER USER;DELETE FROM hagency_agent_v1.management_shared_intake WHERE state='bound' OR reason IN('head_changed','budget_blocked','usage_unrepresentable');ALTER TABLE hagency_agent_v1.management_shared_intake ENABLE TRIGGER USER").await.unwrap();
    // Reinstall the exact previous function/constraints for a real16 source.
    let sql = include_str!("management_shared_intake_schema.sql");
    let f = sql
        .split_once("CREATE FUNCTION hagency_agent_v1.retain_shared_intake()")
        .unwrap()
        .1
        .split_once("CREATE TRIGGER retain_shared_control")
        .unwrap()
        .0;
    db.batch_execute(&format!(
        "CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_intake(){f}"
    ))
    .await
    .unwrap();
    db.batch_execute("ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_state_check;ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_state_check CHECK(state IN('pending','paused','cancelled','expired','blocked','legacy_delivered'));ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_reason_check;ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_reason_check CHECK(reason IN('explicit_stop','paused_at_receipt','manual_start_required','before_running_intent','event_expired','assignment_authority_changed','already_delivered','room_authority_changed','run_control_changed'));ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_check1;ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_check1 CHECK((state='pending')=(reason IS NULL))").await.unwrap();
}
async fn setup() -> (
    Fixture,
    ManagedAgentRequest,
    Actor,
    PendingManagedAssignment,
    MatrixFixture,
) {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_shared_durable_intake(true)
        .with_shared_successors(true)
        .with_shared_demands(true);
    intake_tests::arm(&f, &a, &m).await;
    (f, r, admin, a, m)
}
async fn received(f: &Fixture) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // A test receipt is after the committed Run transition on the same clock.
    // Host/container wall clocks are independent; a fixed host sleep is not a fence.
    db.batch_execute("SELECT pg_sleep(0.002)").await.unwrap();
    sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
async fn retain(f: &Fixture, r: &ManagedAgentRequest, m: &MatrixFixture, id: &str) -> i64 {
    let e = work::message(f, r, id).await;
    let now = received(f).await;
    f.domain
        .route_shared_event(&f.policy, &e, now, &m.gateway)
        .await
        .unwrap();
    now
}
async fn demand(f: &Fixture) -> SharedDemandStatus {
    f.domain
        .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .unwrap()
}
fn start_input(
    d: &SharedDemandStatus,
    original: &SharedRuntimeDelivery,
    stop: &SharedProcessReceipt,
) -> SharedDemandStartInput {
    SharedDemandStartInput {
        demand_digest: d.demand_digest.clone(),
        expected_control_revision: d.offer.control_revision,
        start: attempts::start(original, stop),
    }
}
#[test]
fn shared_demand_canonical_offer_start_binding_preserves_existing_bytes() {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-demand-v1.json"
    ))
    .unwrap();
    let o: SharedDemandOffer = serde_json::from_value(v["offer"].clone()).unwrap();
    o.validate().unwrap();
    assert_eq!(allocation_digest(&o).unwrap(), v["demandDigest"]);
    let s: SharedDemandStartInput = serde_json::from_value(v["start"].clone()).unwrap();
    s.validate_for(&o).unwrap();
    assert_eq!(allocation_digest(&s).unwrap(), v["startDigest"]);
    let b: SharedDemandBinding = serde_json::from_value(v["binding"].clone()).unwrap();
    assert_eq!(allocation_digest(&b).unwrap(), v["bindingDigest"]);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_idle_promotion_preserves_original_event_and_one_work() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (_d, activity) = qualify(&f).await;
    let time = retain(&f, &r, &m, "demand_idle_first").await;
    retain(&f, &r, &m, "demand_idle_second").await;
    let offered = demand(&f).await;
    assert_eq!(offered.offer.event_id, "$demand_idle_first");
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.created_at_ms, time);
    assert_eq!(w.assignment_id, a.id);
    let b = f
        .domain
        .shared_demand_work_binding(
            &f.owner.p,
            &w.id,
            SharedDemandWorkInput {
                work_digest: allocation_digest(&w).unwrap(),
                runtime_command_digest: w.runtime_command_digest.clone(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.offer.event_id, w.event_id);
    assert_eq!(b.binding.work_id, w.id);
    let same = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same.id, w.id);
    let old = f
        .domain
        .recover_shared_demand(
            &f.owner.p,
            &offered.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: offered.demand_digest,
            },
        )
        .await
        .unwrap();
    assert_eq!(old.state, "bound");
    assert_eq!(old.binding.unwrap().work_id, w.id);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_stopped_head_atomic_start_replay_and_promotion() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    let time = retain(&f, &r, &m, "demand_successor").await;
    let o = demand(&f).await;
    let input = start_input(&o, &d, &stopped);
    let service = svc(&f, m.gateway.clone());
    let accepted = post(
        &service,
        &format!("shared-execution/demands/{}/start", o.offer.demand_id),
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    let next: crate::management_shared_attempt_types::SharedAttemptDelivery =
        serde_json::from_value(accepted["delivery"].clone()).unwrap();
    assert!(next.execution_allowed);
    let replay = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &o.offer.demand_id, input, &m.gateway)
        .await
        .unwrap();
    assert!(!replay.execution_allowed);
    assert_eq!(replay.command_digest, next.command_digest);
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &next.command.command_id,
            attempts::result(&next, "qualified", 203),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.created_at_ms, time);
    assert_eq!(w.runtime_command_digest, next.command_digest);
}

async fn recovered(f: &Fixture, o: &SharedDemandStatus) -> SharedDemandStatus {
    f.domain
        .recover_shared_demand(
            &f.owner.p,
            &o.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: o.demand_digest.clone(),
            },
        )
        .await
        .unwrap()
}
async fn no_successor(f: &Fixture, assignment: &str) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1) AS matched").bind::<Text,_>(assignment).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_http_roles_exact_device_generation_and_disabled_history() {
    let (mut f, r, admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "demand_roles").await;
    let o = demand(&f).await;
    let input = start_input(&o, &d, &stopped);
    let service = svc(&f, m.gateway.clone());
    let path = format!("shared-execution/demands/{}/start", o.offer.demand_id);
    for who in [&f.outsider, &f.admin, &f.coordinator, &admin] {
        assert!(
            f.domain
                .start_shared_demand(
                    &who.p,
                    &f.policy,
                    &o.offer.demand_id,
                    input.clone(),
                    &m.gateway
                )
                .await
                .is_err()
        );
        assert!(
            f.domain
                .recover_shared_demand(
                    &who.p,
                    &o.offer.demand_id,
                    SharedDemandRecoverInput {
                        demand_digest: o.demand_digest.clone()
                    }
                )
                .await
                .is_err()
        );
    }
    let mut extra = serde_json::to_value(&input).unwrap();
    extra["ownerMxid"] = json!(f.owner.p.mxid);
    post(
        &service,
        &path,
        &f.owner.token,
        extra,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let response = post(
        &service,
        "shared-execution/demands/poll",
        &f.owner.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["demand"]["demandDigest"], o.demand_digest);
    assert!(response["observedAtMs"].is_i64());
    let other = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "other-demand-device".into(),
                name: "Other".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&other.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .recover_shared_demand(
                &p,
                &o.offer.demand_id,
                SharedDemandRecoverInput {
                    demand_digest: o.demand_digest.clone()
                }
            )
            .await
            .is_err()
    );
    let current = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Reauthorized".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&current.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &o.offer.demand_id,
                input.clone(),
                &m.gateway
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .start_shared_demand(&p, &f.policy, &o.offer.demand_id, input, &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .poll_shared_demand(&p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none()
    );
    f.policy = f
        .policy
        .with_shared_demands(false)
        .with_shared_durable_intake(false)
        .with_shared_execution(false)
        .with_shared_successors(false);
    assert!(
        f.domain
            .poll_shared_demand(&p, &f.policy, ExecutorPollInput::default())
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .recover_shared_demand(
                &p,
                &o.offer.demand_id,
                SharedDemandRecoverInput {
                    demand_digest: o.demand_digest
                }
            )
            .await
            .unwrap()
            .state,
        "offered"
    );
    no_successor(&f, &a.id).await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_start_stop_run_aba_and_admin_change_after_matrix_await() {
    for replace in [false, true] {
        let (f, r, _admin, a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        retain(&f, &r, &m, &format!("demand_aba_{}", r.id)).await;
        let o = demand(&f).await;
        let input = start_input(&o, &d, &stopped);
        let (gateway, entered, release) = intake_tests::delayed_matrix(&m);
        let pending = f.domain.start_shared_demand(
            &f.owner.p,
            &f.policy,
            &o.offer.demand_id,
            input,
            &gateway,
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
                        intake_tests::change("demand_stop", 2, "paused"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
                f.domain
                    .change_shared_run_control(
                        &f.outsider.p,
                        &f.policy,
                        &a.id,
                        intake_tests::change("demand_run", 3, "running"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
            }
            release.close();
        };
        let (result, _) = tokio::join!(pending, invalidate);
        assert!(result.is_err());
        no_successor(&f, &a.id).await;
        if !replace {
            assert_eq!(recovered(&f, &o).await.state, "cancelled");
        }
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_two_store_exact_retry_one_commit_and_changed_body_refused() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "demand_concurrent").await;
    let o = demand(&f).await;
    let input = start_input(&o, &d, &stopped);
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        f.domain.start_shared_demand(
            &f.owner.p,
            &f.policy,
            &o.offer.demand_id,
            input.clone(),
            &m.gateway
        ),
        other.start_shared_demand(
            &f.owner.p,
            &f.policy,
            &o.offer.demand_id,
            input.clone(),
            &m.gateway
        )
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.command_digest, right.command_digest);
    assert_ne!(left.execution_allowed, right.execution_allowed);
    let mut changed = input.clone();
    changed.start.start_intent_id.push_str("_new");
    assert!(
        other
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &o.offer.demand_id,
                changed,
                &m.gateway
            )
            .await
            .is_err()
    );
    let off = f
        .policy
        .clone()
        .with_shared_demands(false)
        .with_shared_successors(false);
    let original = other
        .start_shared_demand(&f.owner.p, &off, &o.offer.demand_id, input, &m.gateway)
        .await
        .unwrap();
    assert!(!original.execution_allowed);
    assert_eq!(original.command_digest, left.command_digest);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1)=1 AND (SELECT count(*) FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=$2)=1 AS matched").bind::<Text,_>(&a.id).bind::<Text,_>(&o.offer.demand_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_unrelated_head_advance_retains_offer_and_blocks_replacement() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "demand_head").await;
    let o = demand(&f).await;
    let mut manual = attempts::start(&d, &stopped);
    manual.start_intent_id.push_str("_explicit");
    let next = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, manual, &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none()
    );
    let saved = recovered(&f, &o).await;
    assert_eq!(saved.state, "head_changed");
    assert_eq!(saved.offer.predecessor_command_id, d.command.command_id);
    assert!(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &o.offer.demand_id,
                start_input(&o, &d, &stopped),
                &m.gateway
            )
            .await
            .is_err()
    );
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &next.command.command_id,
            attempts::result(&next, "qualified", 203),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_start_and_promotion_roll_back_with_original_ids_retryable() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "demand_rollback").await;
    let o = demand(&f).await;
    let input = start_input(&o, &d, &stopped);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fail_demand_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fail_demand_fixture BEFORE INSERT ON hagency_agent_v1.management_shared_demand_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_demand_fixture()").await.unwrap();
    assert!(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &o.offer.demand_id,
                input.clone(),
                &m.gateway
            )
            .await
            .is_err()
    );
    no_successor(&f, &a.id).await;
    assert_eq!(recovered(&f, &o).await.state, "offered");
    db.batch_execute(
        "DROP TRIGGER fail_demand_fixture ON hagency_agent_v1.management_shared_demand_starts",
    )
    .await
    .unwrap();
    let next = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &o.offer.demand_id, input, &m.gateway)
        .await
        .unwrap();
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &next.command.command_id,
            attempts::result(&next, "qualified", 203),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    db.batch_execute("CREATE TRIGGER fail_demand_fixture BEFORE INSERT ON hagency_agent_v1.management_shared_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_demand_fixture()").await.unwrap();
    assert!(
        f.domain
            .poll_shared_work(
                &f.owner.p,
                &f.policy,
                activity.authority.clone(),
                &m.gateway
            )
            .await
            .is_err()
    );
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_event_pins WHERE assignment_id=$2) AS matched").bind::<Text,_>(&next.command.command_id).bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fail_demand_fixture ON hagency_agent_v1.management_shared_demand_bindings;DROP FUNCTION hagency_agent_v1.fail_demand_fixture()").await.unwrap();
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
            .await
            .unwrap()
            .is_some()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_reordered_event_fifo_duplicate_and_immutable_binding_sql() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (_d, activity) = qualify(&f).await;
    let newer = work::message(&f, &r, "demand_fifo_newer").await;
    let older = work::message(&f, &r, "demand_fifo_older").await;
    let earlier = received(&f).await;
    let later = received(&f).await;
    assert!(earlier < later);
    f.domain
        .route_shared_event(&f.policy, &newer, later, &m.gateway)
        .await
        .unwrap();
    f.domain
        .route_shared_event(&f.policy, &older, earlier, &m.gateway)
        .await
        .unwrap();
    f.domain
        .route_shared_event(&f.policy, &older, earlier, &m.gateway)
        .await
        .unwrap();
    let o = demand(&f).await;
    assert_eq!(o.offer.event_id, older.event_id);
    let w = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.event_id, older.event_id);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=2 AS matched FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    for table in [
        "management_shared_demands",
        "management_shared_demand_bindings",
    ] {
        assert!(
            db.batch_execute(&format!("DELETE FROM hagency_agent_v1.{table}"))
                .await
                .is_err()
        );
        assert!(
            db.batch_execute(&format!(
                "UPDATE hagency_agent_v1.{table} SET payload='{{}}'"
            ))
            .await
            .is_err()
        );
    }
    assert!(sql_query("INSERT INTO hagency_agent_v1.management_shared_demand_bindings SELECT demand_id,work_id,runtime_id,'{}',digest FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=$1").bind::<Text,_>(&o.offer.demand_id).execute(&mut db).await.is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_budget_block_retains_unknown_or_overflow_without_start_ids() {
    for unknown in [false, true] {
        let (f, r, _admin, a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        retain(&f, &r, &m, &format!("demand_budget_{}", r.id)).await;
        let o = demand(&f).await;
        let mut budget = attempts::start(&d, &stopped).budget_observation;
        budget.retained_tokens = budget.allocated_tokens;
        budget.remaining_tokens = 0;
        let input = SharedDemandBlockInput {
            demand_digest: o.demand_digest.clone(),
            expected_control_revision: o.offer.control_revision,
            reason: if unknown {
                "usage_unrepresentable"
            } else {
                "budget_blocked"
            }
            .into(),
            budget: (!unknown).then_some(budget),
        };
        for _ in 0..2 {
            assert_eq!(
                f.domain
                    .block_shared_demand(&f.owner.p, &o.offer.demand_id, input.clone())
                    .await
                    .unwrap()
                    .state,
                input.reason
            );
        }
        retain(&f, &r, &m, &format!("demand_budget_later_{}", r.id)).await;
        assert!(
            f.domain
                .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .demand
                .is_none()
        );
        assert!(
            f.domain
                .start_shared_demand(
                    &f.owner.p,
                    &f.policy,
                    &o.offer.demand_id,
                    start_input(&o, &d, &stopped),
                    &m.gateway
                )
                .await
                .is_err()
        );
        assert_eq!(recovered(&f, &o).await.state, input.reason);
        no_successor(&f, &a.id).await;
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_known_overrun_retains_actual_usage_and_zero_remaining_without_start() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "demand_overrun_work").await;
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    let input = SharedWorkInput {
        authority: activity.authority,
        work_id: w.id.clone(),
        work_digest: allocation_digest(&w).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    let t = work::ticket(&f, &d, &w).await;
    let mut result = work::outcome(&d, &w, &t, 211, "completed");
    let consumed = d.command.reserve_command.requested_tokens + 717;
    result.receipt.known_consumed_tokens = consumed;
    result.receipt.retained_tokens = Some(consumed);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, result)
        .await
        .unwrap();
    attempts::stop(&f, &d, 212).await;
    retain(&f, &r, &m, "demand_after_overrun").await;
    assert!(
        f.domain
            .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT w.known_consumed_tokens=$2 AND w.retained_tokens=$2 AND w.usage_complete AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_blocks b ON b.demand_id=d.id WHERE d.payload::jsonb->>'assignmentId'=$3 AND b.reason='budget_blocked') AS matched FROM hagency_agent_v1.management_shared_work w WHERE w.id=$1").bind::<Text,_>(&w.id).bind::<BigInt,_>(consumed).bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    no_successor(&f, &a.id).await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_disabled_link_cannot_issue_work_or_renew_but_evidence_recovers() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "demand_disabled").await;
    let o = demand(&f).await;
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    let off = f.policy.clone().with_shared_demands(false);
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &off, activity.authority.clone(), &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .renew_shared_activity(
                &f.owner.p,
                &off,
                SharedActivityRenewInput {
                    authority: activity.authority,
                    renewal_id: "disabled_demand".into()
                },
                &m.gateway
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .shared_demand_work_binding(
                &f.owner.p,
                &w.id,
                SharedDemandWorkInput {
                    work_digest: allocation_digest(&w).unwrap(),
                    runtime_command_digest: w.runtime_command_digest
                }
            )
            .await
            .unwrap()
            .unwrap()
            .offer
            .demand_id,
        o.offer.demand_id
    );
    let recovery = f
        .domain
        .recover_shared_runtime(
            &f.owner.p,
            &off,
            &d.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest,
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!recovery.execution_allowed);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_schema16_upgrade_atomic_original_bytes_and_controls_preserved() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    retain(&f, &r, &m, "demand_upgrade_pending").await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_demand_starts(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=16 AND to_regclass('hagency_agent_v1.management_shared_demands') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_demand_starts")
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        reopened
            .shared_run_control(&f.owner.p, &a.id)
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(history(&f, &d).await.command_digest, d.command_digest);
    let o = reopened
        .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .unwrap();
    assert_eq!(o.offer.event_id, "$demand_upgrade_pending");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_policy_discovery_is_versioned_and_default_off_does_not_issue() {
    let (mut f, _r, _admin, _a, m) = Box::pin(setup()).await;
    f.policy = f
        .policy
        .with_shared_demands(false)
        .with_shared_durable_intake(false)
        .with_shared_execution(false)
        .with_shared_successors(false);
    let service = svc(&f, m.gateway.clone());
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&service)
        .await;
    let value = response.take_json::<Value>().await.unwrap();
    assert!(
        value["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "shared-durable-demand-v1")
    );
    let mut session = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.owner.token)
        .send(&service)
        .await;
    assert_eq!(session.status_code, Some(StatusCode::OK));
    let session = session.take_json::<Value>().await.unwrap();
    assert!(
        session["features"]
            .as_array()
            .unwrap()
            .contains(&json!("shared-run-control-v1"))
    );
    post(
        &service,
        "shared-execution/demands/poll",
        &f.owner.token,
        json!({}),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert!(
        !ManagementPolicy::new(f.admin.p.mxid.clone(), "example.test")
            .unwrap()
            .shared_demands()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_blocked_assignment_does_not_starve_another_owner_assignment() {
    let (f, r, admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "demand_fair_blocked").await;
    let o = demand(&f).await;
    f.domain
        .block_shared_demand(
            &f.owner.p,
            &o.offer.demand_id,
            SharedDemandBlockInput {
                demand_digest: o.demand_digest,
                expected_control_revision: o.offer.control_revision,
                reason: "usage_unrepresentable".into(),
                budget: None,
            },
        )
        .await
        .unwrap();
    retain(&f, &r, &m, "demand_fair_blocked_later").await;
    let (second, b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let runtime = shared_poll(&f).await;
    assert_eq!(runtime.command.assignment_id, b.id);
    save(&f, &runtime, "qualified", 310).await.unwrap();
    attempts::stop(&f, &runtime, 311).await;
    retain(&f, &second, &m, "demand_fair_available").await;
    let polled = f
        .domain
        .poll_shared_demand(
            &f.owner.p,
            &f.policy,
            ExecutorPollInput {
                after: Some(b.id.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(polled.demand.unwrap().offer.assignment_id, b.id);
    assert_eq!(polled.next_after, Some(b.id));
    no_successor(&f, &a.id).await;
    assert_eq!(stopped.state, "stopped");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_exact_expired_offer_retains_history_and_next_pending_progresses() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let event = work::message(&f, &r, "demand_short_fixture").await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let id = format!("sin_{}", crate::entity_id().unwrap());
    let received = now_ms() - 1000;
    let expiry = now_ms() + 700;
    // A short finite fixture avoids moving clocks or rewriting retained history.
    sql_query("INSERT INTO hagency_agent_v1.management_shared_intake VALUES($1,$2,$3,$4,$5,2,$6,$7,'pending',NULL)").bind::<Text,_>(&id).bind::<Text,_>(&a.id).bind::<Text,_>(&event.event_id).bind::<Text,_>(serde_json::to_string(&event).unwrap()).bind::<Text,_>(allocation_digest(&event).unwrap()).bind::<BigInt,_>(received).bind::<BigInt,_>(expiry).execute(&mut db).await.unwrap();
    let o = demand(&f).await;
    assert_eq!(o.offer.predecessor_command_digest, d.command_digest);
    tokio::time::sleep(std::time::Duration::from_millis(
        (expiry - now_ms() + 20).max(1) as u64,
    ))
    .await;
    assert_eq!(recovered(&f, &o).await.state, "expired");
    retain(&f, &r, &m, "demand_after_expiry").await;
    assert_eq!(demand(&f).await.offer.event_id, "$demand_after_expiry");
    assert!(sql_query("INSERT INTO hagency_agent_v1.management_shared_intake SELECT id||'_bad',assignment_id,event_id||'_bad',event_payload,event_digest,control_revision,received_at_ms,received_at_ms,'pending',NULL FROM hagency_agent_v1.management_shared_intake WHERE id=$1").bind::<Text,_>(&id).execute(&mut db).await.is_err());
}
async fn acknowledge_start_item(
    f: &Fixture,
    m: &MatrixFixture,
    a: &SharedActivityLease,
    w: &SharedWorkItem,
) {
    let input = SharedWorkInput {
        authority: a.authority.clone(),
        work_id: w.id.clone(),
        work_digest: allocation_digest(w).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
}
async fn complete_item(f: &Fixture, w: &SharedWorkItem, process: &str, revision: i64) {
    use crate::management_shared_work_types::{SharedWorkRecoverInput, SharedWorkResultUpload};
    let ticket = f
        .domain
        .recover_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            SharedWorkRecoverInput {
                runtime_command_digest: w.runtime_command_digest.clone(),
                work_digest: allocation_digest(w).unwrap(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let body = "Hello from the exact retained event.";
    let input = SharedWorkResultUpload {
        work_digest: ticket.work_digest.clone(),
        token: ticket.token,
        receipt: SharedWorkReceipt {
            version: 1,
            observation_id: format!("{}_complete_{revision}", w.id),
            work_id: w.id.clone(),
            work_digest: ticket.work_digest,
            execution_id: w.execution_id.clone(),
            call_id: format!("{}_call", w.id),
            runtime_command_digest: w.runtime_command_digest.clone(),
            process_digest: process.into(),
            ledger_revision: revision,
            state: "completed".into(),
            known_consumed_tokens: 17,
            retained_tokens: Some(17),
            usage_complete: true,
            body: Some(body.into()),
            body_digest: Some(crate::hash(body)),
        },
    };
    for _ in 0..2 {
        f.domain
            .record_shared_work_result(&f.owner.p, &f.policy, &w.id, input.clone())
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_two_events_same_assignment_two_original_work_items_exact_matrix_replies() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "demand_two_first").await;
    retain(&f, &r, &m, "demand_two_second").await;
    let first = demand(&f).await;
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    acknowledge_start_item(&f, &m, &activity, &w).await;
    complete_item(&f, &w, &"a".repeat(64), 211).await;
    let sender = work::sender().await;
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &sender.matrix, &mut String::new())
        .await
        .unwrap();
    let stopped = attempts::stop(&f, &d, 212).await;
    let second = demand(&f).await;
    assert_eq!(second.offer.event_id, "$demand_two_second");
    assert_ne!(first.offer.demand_id, second.offer.demand_id);
    let next = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &second.offer.demand_id,
            start_input(&second, &d, &stopped),
            &m.gateway,
        )
        .await
        .unwrap();
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &next.command.command_id,
            attempts::result(&next, "qualified", 214),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let next_work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next_work.event_id, second.offer.event_id);
    assert_ne!(next_work.id, w.id);
    acknowledge_start_item(&f, &m, &activity, &next_work).await;
    complete_item(&f, &next_work, &"d".repeat(64), 216).await;
    for _ in 0..2 {
        f.domain
            .send_shared_replies(&f.policy, &m.gateway, &sender.matrix, &mut String::new())
            .await
            .unwrap();
    }
    let old = work::message(&f, &r, "demand_two_first").await;
    f.domain
        .route_shared_event(&f.policy, &old, first.offer.received_at_ms, &m.gateway)
        .await
        .unwrap();
    assert_eq!(sender.calls.lock().unwrap().len(), 2);
    assert_eq!(recovered(&f, &first).await.binding.unwrap().work_id, w.id);
    assert_eq!(
        recovered(&f, &second).await.binding.unwrap().work_id,
        next_work.id
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1)=1 AND (SELECT count(*) FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1)=2 AND (SELECT count(*) FROM hagency_agent_v1.management_shared_replies x JOIN hagency_agent_v1.management_shared_work w ON w.id=x.work_id JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1 AND x.state='sent')=2 AS matched").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_disabled_before_reply_send_retains_evidence_without_matrix_effect() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (_d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "demand_no_disabled_reply").await;
    let w = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    acknowledge_start_item(&f, &m, &activity, &w).await;
    complete_item(&f, &w, &"a".repeat(64), 211).await;
    let sender = work::sender().await;
    let off = f.policy.clone().with_shared_demands(false);
    f.domain
        .send_shared_replies(&off, &m.gateway, &sender.matrix, &mut String::new())
        .await
        .unwrap();
    assert!(sender.calls.lock().unwrap().is_empty());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT w.state='completed' AND w.known_consumed_tokens=17 AND w.usage_complete AND r.state='blocked' AS matched FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_replies r ON r.work_id=w.id WHERE w.id=$1").bind::<Text,_>(&w.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_intake_disabled_before_promotion_cannot_bypass_retained_event() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (_d, activity) = qualify(&f).await;
    let received = retain(&f, &r, &m, "demand_no_legacy_fallback").await;
    let offered = demand(&f).await;
    let event = work::message(&f, &r, "demand_no_legacy_fallback").await;
    let off = f
        .policy
        .clone()
        .with_shared_demands(false)
        .with_shared_durable_intake(false);
    for _ in 0..2 {
        f.domain
            .route_shared_event(&off, &event, received, &m.gateway)
            .await
            .unwrap();
    }
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &off, activity.authority, &m.gateway)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(recovered(&f, &offered).await.state, "offered");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_event_pins WHERE assignment_id=$1 AND event_id=$2) AS matched").bind::<Text,_>(&a.id).bind::<Text,_>(&event.event_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_control_readiness_distinguishes_roles_mode_policy_and_business_authority() {
    let (f, r, admin, a, m) = Box::pin(setup()).await;
    let owner = f
        .domain
        .shared_run_control_readiness(&f.owner.p, &f.policy, &a.id)
        .await
        .unwrap();
    assert!(!owner.can_run && owner.can_stop && owner.requires_local_stop);
    assert_eq!(owner.run_blocking_reason.as_deref(), Some("role_required"));
    assert!(owner.automatic_delivery_enabled);
    let manager = f
        .domain
        .shared_run_control_readiness(&f.outsider.p, &f.policy, &a.id)
        .await
        .unwrap();
    assert!(!manager.can_run && manager.can_stop && !manager.requires_local_stop);
    assert_eq!(manager.control.mode, "running");
    assert_eq!(
        manager.run_blocking_reason.as_deref(),
        Some("already_running")
    );
    for actor in [&f.admin, &actor(&f.auth, "unrelated_control_reader").await] {
        assert!(
            f.domain
                .shared_run_control_readiness(&actor.p, &f.policy, &a.id)
                .await
                .is_err()
        );
    }
    f.domain
        .change_shared_run_control(
            &f.owner.p,
            &f.policy,
            &a.id,
            intake_tests::change("readiness_stop", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    for actor in [&f.outsider, &admin] {
        let v = f
            .domain
            .shared_run_control_readiness(&actor.p, &f.policy, &a.id)
            .await
            .unwrap();
        assert!(v.can_run && v.can_stop && !v.requires_local_stop);
        assert_eq!(v.control.mode, "paused");
    }
    let off = f
        .policy
        .clone()
        .with_shared_durable_intake(false)
        .with_shared_demands(false);
    let v = f
        .domain
        .shared_run_control_readiness(&admin.p, &off, &a.id)
        .await
        .unwrap();
    assert!(!v.can_run && v.can_stop);
    assert!(!v.automatic_delivery_enabled);
    assert_eq!(v.run_blocking_reason.as_deref(), Some("intake_disabled"));
    let s = svc(&f, m.gateway.clone());
    let mut response=TestClient::get(format!("https://example.test/hagency/api/v1/management/shared-execution/assignments/{}/control/readiness",a.id)).add_header("host","example.test",true).bearer_auth(&f.outsider.token).send(&s).await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let json = response.take_json::<Value>().await.unwrap();
    assert_eq!(json["readiness"]["canRun"], true);
    assert_eq!(json["readiness"]["requiresLocalStop"], false);
    assert!(json["observedAtMs"].is_i64());
    assert!(json["readiness"].get("online").is_none());
    invalidate_authority(&f, &r.project_id).await;
    assert!(!f.domain.shared_run_control_readiness(&admin.p, &f.policy, &a.id).await.unwrap().can_run);
    let v = f
        .domain
        .shared_run_control_readiness(&f.outsider.p, &f.policy, &a.id)
        .await
        .unwrap();
    assert!(!v.can_run && v.can_stop);
    assert_eq!(
        v.run_blocking_reason.as_deref(),
        Some("assignment_authority_changed")
    );
    assert!(!v.requires_local_stop);
    // The execution device retains its cleanup obligation after authority changes.
    let both = f
        .domain
        .shared_run_control_readiness(&f.owner.p, &off, &a.id)
        .await
        .unwrap();
    assert!(both.can_stop && both.requires_local_stop && !both.can_run);
}
