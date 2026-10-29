use super::*;
use crate::{management_shared_attempt_types::*, management_shared_work_types::*};

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    ticket_tests::remove_migration(db).await;
    // Tests rewind a shared disposable database. Discard only rows introduced
    // by the newer protocol; preserve every original v1 row under test.
    db.batch_execute("DROP TRIGGER pin_shared_event ON hagency_agent_v1.management_shared_work;DROP FUNCTION hagency_agent_v1.pin_shared_event();DROP TABLE hagency_agent_v1.management_shared_event_pins,hagency_agent_v1.management_shared_attempts").await.unwrap();
    let tables = [
        "management_shared_replies",
        "management_shared_work_receipts",
        "management_shared_work",
        "management_shared_activity_renewals",
        "management_shared_process_receipts",
        "management_shared_runtimes",
    ];
    for table in tables {
        db.batch_execute(&format!(
            "ALTER TABLE hagency_agent_v1.{table} DISABLE TRIGGER USER"
        ))
        .await
        .unwrap();
    }
    db.batch_execute("DELETE FROM hagency_agent_v1.management_shared_replies WHERE work_id IN(SELECT w.id FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.payload::jsonb->>'version' IN ('2','3'));DELETE FROM hagency_agent_v1.management_shared_work_receipts WHERE work_id IN(SELECT w.id FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.payload::jsonb->>'version' IN ('2','3'));DELETE FROM hagency_agent_v1.management_shared_work WHERE runtime_id IN(SELECT id FROM hagency_agent_v1.management_shared_runtimes WHERE payload::jsonb->>'version' IN ('2','3'));DELETE FROM hagency_agent_v1.management_shared_activity_renewals WHERE runtime_id IN(SELECT id FROM hagency_agent_v1.management_shared_runtimes WHERE payload::jsonb->>'version' IN ('2','3'));DELETE FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id IN(SELECT id FROM hagency_agent_v1.management_shared_runtimes WHERE payload::jsonb->>'version' IN ('2','3'));DELETE FROM hagency_agent_v1.management_shared_runtimes WHERE payload::jsonb->>'version' IN ('2','3')").await.unwrap();
    for table in tables {
        db.batch_execute(&format!(
            "ALTER TABLE hagency_agent_v1.{table} ENABLE TRIGGER USER"
        ))
        .await
        .unwrap();
    }
    db.batch_execute("DROP INDEX hagency_agent_v1.original_shared_runtime,hagency_agent_v1.active_shared_runtime_head;ALTER TABLE hagency_agent_v1.management_shared_runtimes ADD CONSTRAINT management_shared_runtimes_assignment_id_key UNIQUE(assignment_id);ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=13;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=13)").await.unwrap();
    let sql = include_str!("management_shared_execution_schema.sql");
    let sql = sql
        .split_once("CREATE FUNCTION hagency_agent_v1.validate_shared_runtime()")
        .unwrap()
        .1
        .split_once("CREATE TRIGGER validate_shared_runtime")
        .unwrap()
        .0;
    db.batch_execute(&format!(
        "CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime(){sql}"
    ))
    .await
    .unwrap();
}
pub(super) fn start(d: &SharedRuntimeDelivery, stop: &SharedProcessReceipt) -> SharedAttemptStartInput {
    SharedAttemptStartInput {
        web_search: false, tool_policy: None,
        start_intent_id: format!("start_{}", d.command.command_id),
        assignment_id: d.command.assignment_id.clone(),
        predecessor: SharedAttemptPredecessor {
            command_id: d.command.command_id.clone(),
            command_digest: d.command_digest.clone(),
            process_attempt: 1,
            stopped_receipt_digest: allocation_digest(stop).unwrap(),
            process_digest: stop.process_digest.clone().unwrap(),
        },
        budget_observation: SharedBudgetObservation {
            observed_ledger_revision: stop.ledger_revision + 1,
            allocated_tokens: d.command.reserve_command.requested_tokens,
            known_consumed_tokens: 17,
            retained_tokens: 40,
            remaining_tokens: d.command.reserve_command.requested_tokens - 40,
            usage_complete: false,
            liability_digest: "c".repeat(64),
        },
    }
}
pub(super) async fn stop(f: &Fixture, d: &SharedRuntimeDelivery, revision: i64) -> SharedProcessReceipt {
    let ticket = history(f, d).await;
    let v = evidence(&ticket, "stopped", revision);
    f.domain
        .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, v.clone())
        .await
        .unwrap();
    v.receipt
}
pub(super) fn result(d: &SharedAttemptDelivery, state: &str, revision: i64) -> SharedAttemptResultInput {
    SharedAttemptResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: SharedAttemptProcessReceipt {
            version: 2,
            observation_id: format!("{}_obs_{revision}", d.command.command_id),
            command_id: d.command.command_id.clone(),
            command_digest: d.command_digest.clone(),
            assignment_id: d.command.assignment_id.clone(),
            agent_reservation_id: d.command.agent_reservation_id.clone(),
            process_attempt: d.command.process_attempt,
            original_runtime_digest: d.command.original_runtime_digest.clone(),
            ledger_revision: revision,
            process_digest: Some("d".repeat(64)),
            state: state.into(),
            policy_digest: (state == "qualified").then(|| "b".repeat(64)),
        },
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_activity_v2_idle_renewal_survives_original_spawn_deadline() {
    let (mut f, request, _admin, _assignment, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (original, _) = qualify(&f).await;
    let stopped = stop(&f, &original, 220).await;
    let delivery = f
        .domain
        .start_shared_attempt(
            &f.owner.p,
            &f.policy,
            start(&original, &stopped),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(delivery.execution_allowed);
    let original_bytes = serde_json::to_vec(&delivery.command).unwrap();
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &delivery.command.command_id,
            result(&delivery, "qualified", 240),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let activity =
        activity_tests::renew_past_spawn(&f, &m, activity, delivery.command.expires_at_ms).await;
    let observed = f
        .domain
        .shared_execution_status(&f.outsider.p, &f.policy, &request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.state, "qualified");
    assert!(observed.work_available);
    assert!(!observed.capacity_released);
    let recovered = f
        .domain
        .recover_shared_attempt(
            &f.owner.p,
            &f.policy,
            &delivery.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: delivery.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&recovered.command).unwrap(),
        original_bytes
    );
    assert!(!recovered.execution_allowed);
    assert!(!recovered.cleanup_allowed);
    assert_eq!(recovered.recorded.as_ref().unwrap().state, "qualified");
    let queued = work::queued(&f, &request, &m, &activity, "after_successor_spawn_window").await;
    assert_eq!(queued.runtime_command_digest, delivery.command_digest);
}

#[test]
fn shared_attempt_fixture_preserves_original_and_allows_new_spawn_window_only() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-attempt-v2.json"
    ))
    .unwrap();
    let c: SharedAttemptCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
    c.validate("example.test").unwrap();
    assert_eq!(allocation_digest(&c).unwrap(), fixture["commandDigest"]);
    assert!(c.issued_at_ms > c.original_runtime_command.expires_at_ms);
    let receipt: SharedAttemptProcessReceipt =
        serde_json::from_value(fixture["receipt"].clone()).unwrap();
    receipt.validate_for(&c).unwrap();
    assert_eq!(
        allocation_digest(&receipt).unwrap(),
        fixture["receiptDigest"]
    );
    let mut altered = fixture["command"].clone();
    altered["canExecute"] = json!(true);
    assert!(serde_json::from_value::<SharedAttemptCommand>(altered).is_err());
    let mut bad = c.clone();
    bad.budget_observation.retained_tokens = bad.budget_observation.allocated_tokens;
    bad.budget_observation.remaining_tokens = 0;
    assert!(bad.validate("example.test").is_err());
    let mut bad = c.clone();
    bad.predecessor.process_attempt = 2;
    assert!(bad.validate("example.test").is_err());
    assert!(serde_json::from_value::<SharedProcessReceipt>(fixture["receipt"].clone()).is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_exact_start_replay_roles_policy_and_head_cas() {
    let (mut f, _r, admin, _a, m) = Box::pin(prepared()).await;
    let (d, _) = qualify(&f).await;
    let stopped = stop(&f, &d, 220).await;
    let mut input = start(&d, &stopped);
    input.web_search = true;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway)
            .await
            .is_err()
    );
    f.policy = f.policy.with_shared_successors(true);
    for actor in [&f.outsider.p, &f.coordinator.p, &f.admin.p, &admin.p] {
        assert!(
            f.domain
                .start_shared_attempt(actor, &f.policy, input.clone(), &m.gateway)
                .await
                .is_err()
        );
    }
    let service = svc(&f, m.gateway.clone());
    let response = post(
        &service,
        "shared-execution/attempts/start",
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    let one: SharedAttemptDelivery = serde_json::from_value(response["delivery"].clone()).unwrap();
    assert!(one.execution_allowed);
    assert!(one.command.web_search);
    assert_eq!(one.command.process_attempt, 2);
    assert_eq!(
        allocation_digest(&one.command.original_runtime_command).unwrap(),
        d.command_digest
    );
    let again = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert!(!again.execution_allowed);
    assert_eq!(again.command_digest, one.command_digest);
    let mut different_policy = input.clone();
    different_policy.web_search = false;
    assert!(f.domain.start_shared_attempt(&f.owner.p, &f.policy, different_policy, &m.gateway).await.is_err());
    let mut replacement = input.clone();
    replacement.start_intent_id.push_str("_replacement");
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, replacement, &m.gateway)
            .await
            .is_err()
    );
    let mut changed = input.clone();
    changed.budget_observation.observed_ledger_revision += 1;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, changed, &m.gateway)
            .await
            .is_err()
    );
    let fresh = f
        .domain
        .poll_shared_attempt(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(fresh.execution_allowed);
    let mut raw = serde_json::to_value(input).unwrap();
    raw["admin"] = json!(true);
    post(
        &service,
        "shared-execution/attempts/start",
        &f.owner.token,
        raw,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let historical = f
        .domain
        .recover_shared_attempt(
            &f.owner.p,
            &f.policy,
            &one.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: one.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!historical.execution_allowed);
    let saved = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &one.command.command_id,
            result(&historical, "qualified", 240),
        )
        .await
        .unwrap();
    assert!(saved.activity.is_none());
    assert_eq!(saved.state, "issued");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_retains_old_unknown_work_and_sends_only_new_matrix_reply() {
    let (mut f, r, admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, a) = qualify(&f).await;
    let old = work::started(&f, &r, &m, &a, "old_unknown").await;
    let t = work::ticket(&f, &d, &old).await;
    let mut unknown = work::outcome(&d, &old, &t, 210, "unknown");
    unknown.receipt.usage_complete = false;
    unknown.receipt.retained_tokens = Some(40);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &old.id, unknown)
        .await
        .unwrap();
    let stopped = stop(&f, &d, 220).await;
    let input = start(&d, &stopped);
    let old_status = f
        .domain
        .managed_agent_removal(&admin.p, &r.id)
        .await
        .unwrap();
    assert_eq!(old_status.process_observation.unwrap().state, "stopped");
    let attempt = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .managed_agent_removal(&admin.p, &r.id)
            .await
            .unwrap()
            .process_observation
            .is_none(),
        "an unobserved successor must not report its predecessor as stopped"
    );
    let activity = f
        .domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &attempt.command.command_id,
            result(&attempt, "qualified", 240),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let duplicate = work::message(&f, &r, "old_unknown").await;
    f.domain
        .route_shared_event(&f.policy, &duplicate, now_ms(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(
                &f.owner.p,
                &f.policy,
                activity.authority.clone(),
                &m.gateway
            )
            .await
            .unwrap()
            .is_none()
    );
    let new = work::started(&f, &r, &m, &activity, "new_independent").await;
    assert_ne!(old.execution_id, new.execution_id);
    let ticket = f
        .domain
        .recover_shared_work_result(
            &f.owner.p,
            &f.policy,
            &new.id,
            SharedWorkRecoverInput {
                runtime_command_digest: attempt.command_digest.clone(),
                work_digest: allocation_digest(&new).unwrap(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let body = "New reply.".to_owned();
    let receipt = SharedWorkReceipt {
        version: 1,
        observation_id: "new_reply_observation".into(),
        work_id: new.id.clone(),
        work_digest: allocation_digest(&new).unwrap(),
        execution_id: new.execution_id.clone(),
        call_id: "new_call".into(),
        runtime_command_digest: attempt.command_digest.clone(),
        process_digest: "d".repeat(64),
        ledger_revision: 250,
        state: "completed".into(),
        known_consumed_tokens: 9,
        retained_tokens: Some(9),
        usage_complete: true,
        body_digest: Some(crate::hash(&body)),
        body: Some(body),
    };
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &new.id,
            SharedWorkResultUpload {
                work_digest: ticket.work_digest,
                token: ticket.token,
                receipt,
            },
        )
        .await
        .unwrap();
    let sender = work::sender().await;
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &sender.matrix, &mut String::new())
        .await
        .unwrap();
    assert_eq!(sender.calls.lock().unwrap().len(), 1);
    let rows = f
        .domain
        .shared_work_history(&admin.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    assert_eq!(rows.items.len(), 2);
    let retained = rows.items.iter().find(|v| v.id == old.id).unwrap();
    assert_eq!(retained.state, "unknown");
    assert_eq!(retained.usage.as_ref().unwrap().retained_tokens, Some(40));
    assert_eq!(
        rows.items
            .iter()
            .find(|v| v.id == new.id)
            .unwrap()
            .reply_state
            .as_deref(),
        Some("sent")
    );
    let off = f.policy.clone().with_shared_successors(false);
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &off, activity.authority, &m.gateway)
            .await
            .is_err()
    );
    // A late old-process overrun remains charged after the successor has replied.
    let ticket = work::ticket(&f, &d, &old).await;
    let mut late = work::outcome(&d, &old, &ticket, 260, "unknown");
    late.receipt.known_consumed_tokens = d.command.reserve_command.requested_tokens + 1;
    late.receipt.retained_tokens = Some(late.receipt.known_consumed_tokens);
    late.receipt.usage_complete = false;
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &old.id, late)
        .await
        .unwrap();
    let history = f
        .domain
        .recover_shared_attempt(
            &f.owner.p,
            &f.policy,
            &attempt.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: attempt.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let stopped2 = result(&history, "stopped", 270);
    f.domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &attempt.command.command_id,
            stopped2.clone(),
        )
        .await
        .unwrap();
    let mut stale = start(&d, &stopped);
    stale.start_intent_id.push_str("_after_overrun");
    stale.predecessor = SharedAttemptPredecessor {
        command_id: attempt.command.command_id.clone(),
        command_digest: attempt.command_digest,
        process_attempt: 2,
        stopped_receipt_digest: allocation_digest(&stopped2.receipt).unwrap(),
        process_digest: stopped2.receipt.process_digest.unwrap(),
    };
    stale.budget_observation.observed_ledger_revision = 271;
    assert_eq!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, stale, &m.gateway)
            .await
            .unwrap_err()
            .to_string(),
        "shared_budget_observation_changed"
    );
    let removal = f
        .domain
        .managed_agent_removal(&admin.p, &r.id)
        .await
        .unwrap();
    assert_eq!(
        removal.usage.known_consumed_tokens,
        Some(d.command.reserve_command.requested_tokens + 10)
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_missing_stop_stale_budget_and_cancel_never_create_authority() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let fake = evidence(&d, "stopped", 220).receipt;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, start(&d, &fake), &m.gateway)
            .await
            .is_err()
    );
    let stopped = stop(&f, &d, 220).await;
    let mut input = start(&d, &stopped);
    input.budget_observation.observed_ledger_revision = 219;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .is_err()
    );
    f.domain
        .cancel_managed_agent_request(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "cancel_before_successor".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, start(&d, &stopped), &m.gateway)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1) AS matched").bind::<Text,_>(&d.command.assignment_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_schema13_upgrade_rolls_back_and_preserves_v1_bytes() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let (d, _) = qualify(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_event_pins(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=13 AND to_regclass('hagency_agent_v1.management_shared_attempts') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_event_pins")
        .await
        .unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let recovered = history(&f, &d).await;
    assert_eq!(
        allocation_digest(&recovered.command).unwrap(),
        d.command_digest
    );
    assert!(
        sql_query("SELECT version=27 AS matched FROM hagency_agent_v1.management_deployment")
            .get_result::<Check>(&mut db)
            .await
            .unwrap()
            .matched
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_concurrent_same_intent_recovers_one_command_and_sql_fences_history() {
    let (mut f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let stopped = stop(&f, &d, 220).await;
    let input = start(&d, &stopped);
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway),
        other.start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway),
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.command_digest, right.command_digest);
    assert_ne!(left.execution_allowed, right.execution_allowed);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1").bind::<Text,_>(&d.command.assignment_id).get_result::<Check>(&mut db).await.unwrap().matched);
    for sql in [
        "DELETE FROM hagency_agent_v1.management_shared_attempts WHERE id=$1",
        "UPDATE hagency_agent_v1.management_shared_attempts SET input_digest=repeat('0',64) WHERE id=$1",
    ] {
        assert!(
            sql_query(sql)
                .bind::<Text, _>(&input.start_intent_id)
                .execute(&mut db)
                .await
                .is_err()
        );
    }
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=false WHERE id=$1").bind::<Text,_>(&d.command.command_id).execute(&mut db).await.is_err());
    let current = f
        .domain
        .poll_shared_attempt(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    let receipt = result(&current, "stopped", 240);
    f.domain
        .record_shared_attempt_process(
            &f.owner.p,
            &f.policy,
            &current.command.command_id,
            receipt.clone(),
        )
        .await
        .unwrap();
    let mut next = input;
    next.start_intent_id.push_str("_third");
    next.predecessor = SharedAttemptPredecessor {
        command_id: current.command.command_id.clone(),
        command_digest: current.command_digest.clone(),
        process_attempt: 2,
        stopped_receipt_digest: allocation_digest(&receipt.receipt).unwrap(),
        process_digest: receipt.receipt.process_digest.unwrap(),
    };
    next.budget_observation.observed_ledger_revision = 241;
    let third = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, next, &m.gateway)
        .await
        .unwrap();
    assert_eq!(third.command.process_attempt, 3);
    assert_eq!(third.command.original_runtime_digest, d.command_digest);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_revocation_during_matrix_read_commits_no_new_authority() {
    let (mut f, r, admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let stopped = stop(&f, &d, 220).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let data = m.states.clone();
    let notified = entered.clone();
    let wait = release.clone();
    let delayed = Gateway::new(
        Arc::new(move |room| {
            let data = data.clone();
            let notified = notified.clone();
            let wait = wait.clone();
            Box::pin(async move {
                notified.notify_one();
                let _ = wait.acquire().await;
                data.lock()
                    .unwrap()
                    .get(&room)
                    .cloned()
                    .ok_or(crate::Error::NotFound("missing"))
            })
        }),
        SERVICE.into(),
    );
    let pending =
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, start(&d, &stopped), &delayed);
    let cancellation = async {
        entered.notified().await;
        revoke(&f, &r, &admin).await;
        release.close();
    };
    let (result, _) = tokio::join!(pending, cancellation);
    assert!(result.is_err());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE assignment_id=$1) AS matched").bind::<Text,_>(&d.command.assignment_id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_generation_change_and_expired_ticket_allow_only_original_cleanup() {
    let (mut f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let stopped = stop(&f, &d, 220).await;
    let delivery = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, start(&d, &stopped), &m.gateway)
        .await
        .unwrap();
    // Ticket expiry is immutable and independent of the diagnostic latest-lease
    // row. Exercise the actual finite original ticket instead of editing it.
    // PostgreSQL owns ticket expiry. A host-clock sleep with a 20ms margin can
    // return before the database clock crosses that boundary (e.g. a Docker VM
    // clock adjustment). Observe the unchanged original ticket, with a finite
    // monotonic timeout, before asserting rejection by the production endpoint.
    #[derive(QueryableByName)]
    struct Expiry {
        #[diesel(sql_type=BigInt)] expires: i64,
        #[diesel(sql_type=BigInt)] remaining: i64,
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(40), async {
        loop {
            let expiry = sql_query("SELECT expires_at_ms AS expires,expires_at_ms-(extract(epoch FROM clock_timestamp())*1000)::bigint AS remaining FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND token_hash=$2")
                .bind::<Text,_>(&delivery.command.command_id).bind::<Text,_>(crate::hash(&delivery.token))
                .get_result::<Expiry>(&mut db).await.unwrap();
            assert_eq!(expiry.expires, delivery.lease_until_ms);
            if expiry.remaining <= 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(expiry.remaining.clamp(1,500) as u64)).await;
        }
    }).await.expect("original database ticket must actually expire");
    drop(db);
    assert!(
        f.domain
            .record_shared_attempt_process(
                &f.owner.p,
                &f.policy,
                &delivery.command.command_id,
                result(&delivery, "qualified", 240)
            )
            .await
            .is_err()
    );
    let new = f
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
        .authenticate(&new.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .recover_shared_attempt(
                &f.owner.p,
                &f.policy,
                &delivery.command.command_id,
                ManagedRuntimeRecoverInput {
                    command_digest: delivery.command_digest.clone(),
                    observation_id: None
                }
            )
            .await
            .is_err()
    );
    let history = f
        .domain
        .recover_shared_attempt(
            &p,
            &f.policy,
            &delivery.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: delivery.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!history.execution_allowed);
    assert!(!history.cleanup_allowed);
    let late = f
        .domain
        .record_shared_attempt_process(
            &p,
            &f.policy,
            &delivery.command.command_id,
            result(&history, "qualified", 240),
        )
        .await
        .unwrap();
    assert!(late.activity.is_none());
    assert_eq!(late.state, "cleanup_pending");
    let cleanup = f
        .domain
        .poll_shared_attempt(&p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
    f.domain
        .record_shared_attempt_process(
            &p,
            &f.policy,
            &delivery.command.command_id,
            result(&cleanup, "stopped", 241),
        )
        .await
        .unwrap();
    let mut next = start(&d, &stopped);
    next.start_intent_id.push_str("_newgen");
    assert!(
        f.domain
            .start_shared_attempt(&p, &f.policy, next, &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .shared_execution_status(
                &f.outsider.p,
                &f.policy,
                &d.command.reserve_command.request_id
            )
            .await
            .unwrap()
            .unwrap()
            .state
            == "stopped"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_old_spawn_expiry_does_not_destroy_live_business_assignment() {
    let (mut f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let delay = (d.command.expires_at_ms - now_ms() + 30).max(1) as u64;
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    let stopped = stop(&f, &d, 220).await;
    let fresh = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, start(&d, &stopped), &m.gateway)
        .await
        .unwrap();
    assert!(fresh.execution_allowed);
    assert!(fresh.command.issued_at_ms > d.command.expires_at_ms);
    assert_eq!(fresh.command.original_runtime_digest, d.command_digest);
    assert!(fresh.command.expires_at_ms <= d.command.reserve_command.expires_at_ms);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_intent_and_process_observation_commit_atomically() {
    let (mut f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (d, _) = qualify(&f).await;
    let stopped = stop(&f, &d, 220).await;
    let input = start(&d, &stopped);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_attempt_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fixture_attempt_reject BEFORE INSERT ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_attempt_reject()").await.unwrap();
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, input.clone(), &m.gateway)
            .await
            .is_err()
    );
    assert!(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts WHERE id=$1) AS matched").bind::<Text,_>(&input.start_intent_id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_attempt_reject ON hagency_agent_v1.management_shared_runtimes;CREATE TRIGGER fixture_attempt_reject BEFORE INSERT ON hagency_agent_v1.management_shared_process_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_attempt_reject()").await.unwrap();
    let delivery = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    let receipt = result(&delivery, "qualified", 240);
    assert!(
        f.domain
            .record_shared_attempt_process(
                &f.owner.p,
                &f.policy,
                &delivery.command.command_id,
                receipt.clone()
            )
            .await
            .is_err()
    );
    assert!(sql_query("SELECT state='issued' AND activity_epoch=0 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1) AS matched FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1").bind::<Text,_>(&delivery.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_attempt_reject ON hagency_agent_v1.management_shared_process_receipts;DROP FUNCTION hagency_agent_v1.fixture_attempt_reject()").await.unwrap();
    assert!(
        f.domain
            .record_shared_attempt_process(
                &f.owner.p,
                &f.policy,
                &delivery.command.command_id,
                receipt
            )
            .await
            .unwrap()
            .activity
            .is_some()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_original_execute_ticket_survives_start_replay_and_poll() {
    let (mut f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_successors(true);
    let (original, _) = qualify(&f).await;
    let stopped = stop(&f, &original, 220).await;
    let input = start(&original, &stopped);
    let service = svc(&f, m.gateway.clone());
    let first = post(&service, "shared-execution/attempts/start", &f.owner.token,
        serde_json::to_value(&input).unwrap(), StatusCode::OK).await;
    let delivered: SharedAttemptDelivery = serde_json::from_value(first["delivery"].clone()).unwrap();
    assert!(delivered.execution_allowed);
    // Real initialization can outlast a background worker iteration. Both
    // observation of the same saved intent and discovery must preserve this
    // original finite upload ticket while never renewing its expiry.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let replay = post(&service, "shared-execution/attempts/start", &f.owner.token,
        serde_json::to_value(&input).unwrap(), StatusCode::OK).await;
    assert_eq!(replay["delivery"]["executionAllowed"], false);
    let polled = post(&service, "shared-execution/attempts/poll", &f.owner.token,
        json!({}), StatusCode::OK).await;
    assert_eq!(polled["delivery"]["commandDigest"], delivered.command_digest);
    let ack = post(&service, &format!("shared-execution/attempts/{}/result", delivered.command.command_id),
        &f.owner.token, serde_json::to_value(result(&delivered,"qualified",240)).unwrap(), StatusCode::OK).await;
    assert_eq!(ack["state"],"qualified");
    assert!(ack["activity"]["expiresAtMs"].as_i64().unwrap()>now_ms());
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1").bind::<Text,_>(&delivered.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_v2_observation_first_then_poll_needs_original_execute_and_latest_evidence() {
    for stopped_first in [false,true] {
        let(mut f,_r,_admin,_a,m)=Box::pin(prepared()).await;f.policy=f.policy.with_shared_successors(true);
        let(original,_)=qualify(&f).await;let stopped=stop(&f,&original,220).await;
        let input=start(&original,&stopped);
        let execute=f.domain.start_shared_attempt(&f.owner.p,&f.policy,input.clone(),&m.gateway).await.unwrap();
        let observation=f.domain.start_shared_attempt(&f.owner.p,&f.policy,input,&m.gateway).await.unwrap();
        assert!(!observation.execution_allowed);
        let service=svc(&f,m.gateway.clone());
        let path=format!("shared-execution/attempts/{}/result",execute.command.command_id);
        let ack=post(&service,&path,&f.owner.token,serde_json::to_value(result(&observation,"qualified",240)).unwrap(),StatusCode::OK).await;
        assert_eq!(ack["state"],"issued");assert!(ack["activity"].is_null());
        let poll=post(&service,"shared-execution/attempts/poll",&f.owner.token,json!({}),StatusCode::OK).await;
        let polled:SharedAttemptDelivery=serde_json::from_value(poll["delivery"].clone()).unwrap();
        assert!(!polled.execution_allowed);assert!(!polled.cleanup_allowed);assert_eq!(polled.recorded.as_ref().unwrap().state,"qualified");
        if stopped_first {
            post(&service,&path,&f.owner.token,serde_json::to_value(result(&polled,"stopped",241)).unwrap(),StatusCode::OK).await;
        }
        let ack=post(&service,&path,&f.owner.token,serde_json::to_value(result(&execute,"qualified",240)).unwrap(),StatusCode::OK).await;
        if stopped_first {assert_eq!(ack["state"],"stopped");assert!(ack["activity"].is_null());}
        else {assert_eq!(ack["state"],"qualified");assert!(ack["activity"].is_object());}
        let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND ledger_revision=240").bind::<Text,_>(&execute.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_permissions_owner_scope_session_decision_and_single_consumption() {
    let (mut f, request, _admin, _assignment, m)=Box::pin(prepared()).await;
    f.policy=f.policy.with_shared_successors(true);
    let (original,_)=qualify(&f).await;let stopped=stop(&f,&original,220).await;
    let mut input=start(&original,&stopped);
    input.tool_policy=Some(crate::management_shared_tool_policy::SharedAgentToolPolicy{mode:"standard".into(),revision:1});
    let d=f.domain.start_shared_attempt(&f.owner.p,&f.policy,input.clone(),&m.gateway).await.unwrap();
    assert_eq!(d.command.tool_policy.as_ref().unwrap().mode,"standard");
    let mut altered=input.clone();altered.tool_policy.as_mut().unwrap().mode="yolo".into();
    assert!(f.domain.start_shared_attempt(&f.owner.p,&f.policy,altered,&m.gateway).await.is_err());
    let a=f.domain.record_shared_attempt_process(&f.owner.p,&f.policy,&d.command.command_id,result(&d,"qualified",240)).await.unwrap().activity.unwrap();
    let w=work::started(&f,&request,&m,&a,"$permission_session").await;
    let service=svc(&f,m.gateway.clone());
    let register=json!({"authority":a.authority,"workId":w.id,"workDigest":allocation_digest(&w).unwrap(),"id":"sap_once","remainingMs":100_000,"request":{"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread_a","turnId":"turn_a","itemId":"item_a","command":"curl https://example.test","cwd":"/workspace","reason":"Network access"},"sessionAllowed":true,"managerCanDecide":false}});
    let created=post(&service,"shared-execution/approvals/register",&f.owner.token,register.clone(),StatusCode::OK).await;
    assert_eq!(created["approval"]["roomId"],w.room_id);
    let listed=post(&service,"shared-execution/approvals/query",&f.owner.token,json!({"roomId":w.room_id}),StatusCode::OK).await;
    assert_eq!(listed["approvals"][0]["canDecide"],true);
    let foreign_room=post(&service,"shared-execution/approvals/query",&f.owner.token,json!({"roomId":"!elsewhere:example.test"}),StatusCode::OK).await;
    assert_eq!(foreign_room["approvals"],json!([]));
    let decision=json!({"id":"sap_once","digest":created["digest"],"decision":"approve_session"});
    post(&service,"shared-execution/approvals/decide",&f.outsider.token,decision.clone(),StatusCode::UNAUTHORIZED).await;
    let mut wrong=decision.clone();wrong["digest"]=json!("0".repeat(64));
    post(&service,"shared-execution/approvals/decide",&f.owner.token,wrong,StatusCode::CONFLICT).await;
    post(&service,"shared-execution/approvals/decide",&f.owner.token,decision.clone(),StatusCode::OK).await;
    post(&service,"shared-execution/approvals/decide",&f.owner.token,decision.clone(),StatusCode::OK).await;
    let mut opposite=decision.clone();opposite["decision"]=json!("deny");
    post(&service,"shared-execution/approvals/decide",&f.owner.token,opposite,StatusCode::CONFLICT).await;
    let take=json!({"authority":a.authority,"id":"sap_once","digest":created["digest"]});
    let result=post(&service,"shared-execution/approvals/take",&f.owner.token,take.clone(),StatusCode::OK).await;
    assert_eq!(result["decision"],"approve_session");
    post(&service,"shared-execution/approvals/take",&f.owner.token,take,StatusCode::CONFLICT).await;
    let mut denied=register.clone();denied["id"]=json!("sap_denied");denied["request"]["id"]=json!(8);
    let second=post(&service,"shared-execution/approvals/register",&f.owner.token,denied,StatusCode::OK).await;
    let undecided=post(&service,"shared-execution/approvals/take",&f.owner.token,json!({"authority":a.authority,"id":"sap_denied","digest":second["digest"]}),StatusCode::OK).await;
    assert!(undecided["decision"].is_null(),"session approval must not become a server-wide grant");
    post(&service,"shared-execution/approvals/decide",&f.owner.token,json!({"id":"sap_denied","digest":second["digest"],"decision":"deny"}),StatusCode::OK).await;
    let mut delegated=register.clone();delegated["id"]=json!("sap_workspace");delegated["request"]=json!({"id":10,"method":"item/fileChange/requestApproval","params":{"threadId":"thread_a","turnId":"turn_a","itemId":"file_a","grantRoot":"/workspace"},"sessionAllowed":true,"managerCanDecide":true});
    let fourth=post(&service,"shared-execution/approvals/register",&f.owner.token,delegated,StatusCode::OK).await;
    post(&service,"shared-execution/approvals/decide",&f.outsider.token,json!({"id":"sap_workspace","digest":fourth["digest"],"decision":"approve_once"}),StatusCode::OK).await;
    let mut expired=register.clone();expired["id"]=json!("sap_expired");expired["request"]["id"]=json!(9);
    let third=post(&service,"shared-execution/approvals/register",&f.owner.token,expired,StatusCode::OK).await;
    {let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();sql_query("UPDATE hagency_agent_v1.management_shared_approvals SET expires_at_ms=1 WHERE id='sap_expired'").execute(&mut db).await.unwrap();}
    post(&service,"shared-execution/approvals/decide",&f.owner.token,json!({"id":"sap_expired","digest":third["digest"],"decision":"approve_once"}),StatusCode::CONFLICT).await;
}
