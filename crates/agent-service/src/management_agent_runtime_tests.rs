use super::*;
#[path="management_shared_execution_tests.rs"]
mod shared;
use crate::management_agent_runtime_types::*;
#[derive(QueryableByName)]
struct Revision {
    #[diesel(sql_type=BigInt)]
    now: i64,
}
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    shared::remove_migration(db).await;
    db.batch_execute("DROP TRIGGER queue_managed_runtime_stop ON hagency_agent_v1.management_shared_assignments;DROP TABLE hagency_agent_v1.management_agent_runtime_observations,hagency_agent_v1.management_agent_runtime_commands;DROP FUNCTION hagency_agent_v1.queue_managed_runtime_stop(),hagency_agent_v1.retain_managed_runtime(),hagency_agent_v1.validate_managed_runtime();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=10;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=10)").await.unwrap();
}
async fn ready() -> (
    Fixture,
    ManagedAgentRequest,
    Actor,
    PendingManagedAssignment,
    MatrixFixture,
) {
    let (mut f, r, admin, a) = prepared_matrix().await;
    f.policy = f.policy.with_agent_runtime_provisioning(true);
    let m = matrix_fixture(&f, &r).await;
    permit(&f).await;
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    (f, r, admin, a, m)
}
async fn poll_runtime(f: &Fixture) -> ManagedRuntimeDelivery {
    f.domain
        .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
fn result(d: &ManagedRuntimeDelivery, state: &str, revision: i64) -> ManagedRuntimeResultInput {
    let c = d.command.original();
    ManagedRuntimeResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: ManagedRuntimeReceipt {
            version: 1,
            observation_id: format!("{}_observation_{revision}", d.command.id()),
            command_id: d.command.id().into(),
            command_digest: d.command_digest.clone(),
            assignment_id: c.assignment_id.clone(),
            agent_reservation_id: c.agent_reservation_id.clone(),
            reserve_command_digest: c.reserve_command_digest.clone(),
            reserve_receipt_digest: c.reserve_receipt_digest.clone(),
            effect_id: format!("{}_provision", c.agent_reservation_id),
            ledger_revision: revision,
            state: state.into(),
            process_state: match state {
                "qualified" => "live",
                "failed" | "not_started" => "not_started",
                "stopped" => "stopped",
                _ => "unknown",
            }
            .into(),
            runtime_digest: (state == "qualified").then(|| "a".repeat(64)),
        },
    }
}
async fn submit_runtime(
    f: &Fixture,
    d: &ManagedRuntimeDelivery,
    r: ManagedRuntimeResultInput,
) -> crate::Result<ManagedRuntimeAcknowledgement> {
    f.domain
        .record_managed_runtime(&f.owner.p, &f.policy, d.command.id(), r)
        .await
}
async fn revoke(f: &Fixture, r: &ManagedAgentRequest, admin: &Actor) {
    f.domain
        .revoke_managed_agent_request(
            &admin.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "revoke_runtime".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_finite_delivery_requires_actual_matrix_and_never_enables_personal_work() {
    let (mut f, r, _admin, a) = prepared_matrix().await;
    f.policy = f.policy.with_agent_runtime_provisioning(true);
    assert!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    let m = matrix_fixture(&f, &r).await;
    permit(&f).await;
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    let d = poll_runtime(&f).await;
    assert!(d.execution_allowed);
    assert!(matches!(d.command, ManagedRuntimeCommand::Provision(_)));
    assert!(d.lease_until_ms <= d.command.original().expires_at_ms);
    let again = poll_runtime(&f).await;
    assert_eq!(again.command_digest, d.command_digest);
    assert_eq!(again.command.id(), d.command.id());
    let old = f
        .domain
        .recover_managed_runtime(
            &f.owner.p,
            &f.policy,
            d.command.id(),
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!old.execution_allowed);
    assert_eq!(
        old.command.original().expires_at_ms,
        d.command.original().expires_at_ms
    );
    let accepted = submit_runtime(&f, &old, result(&old, "qualified", 200))
        .await
        .unwrap();
    assert_eq!(accepted.runtime.state, "qualified");
    assert!(!accepted.runtime.work_available);
    assert!(!accepted.runtime.capacity_released);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT a.state='suspended' AND a.execution_device_id IS NULL AND a.owner_user_id=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id) AS matched FROM hagency_agent_v1.agents a WHERE a.id=$1").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&f.outsider.p.user_id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_roles_default_off_closed_body_and_exact_receipt_replay() {
    let (f, r, admin, _a, m) = ready().await;
    assert!(
        f.domain
            .poll_managed_runtime(
                &f.owner.p,
                &f.policy.clone().with_agent_runtime_provisioning(false),
                ExecutorPollInput::default()
            )
            .await
            .is_err()
    );
    let d = poll_runtime(&f).await;
    for p in [&f.admin.p, &f.coordinator.p, &f.outsider.p, &admin.p] {
        assert!(
            f.domain
                .poll_managed_runtime(p, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            f.domain
                .record_managed_runtime(p, &f.policy, d.command.id(), result(&d, "qualified", 200))
                .await
                .is_err()
        );
    }
    let service = svc(&f, m.gateway.clone());
    let mut raw = serde_json::to_value(result(&d, "qualified", 200)).unwrap();
    raw["canExecute"] = json!(true);
    post(
        &service,
        &format!("agent-runtime/{}/result", d.command.id()),
        &f.owner.token,
        raw,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let mut bad = result(&d, "qualified", 200);
    bad.receipt.agent_reservation_id = "wrong_child".into();
    assert!(submit_runtime(&f, &d, bad).await.is_err());
    let input = result(&d, "qualified", 200);
    let accepted = submit_runtime(&f, &d, input.clone()).await.unwrap();
    let replay = submit_runtime(&f, &d, input.clone()).await.unwrap();
    assert_eq!(accepted.receipt_digest, replay.receipt_digest);
    let mut bad = input;
    bad.receipt.runtime_digest = Some("b".repeat(64));
    assert!(submit_runtime(&f, &d, bad).await.is_err());
    assert!(
        f.domain
            .managed_runtime(&actor(&f.auth, "unrelated_runtime_reader").await.p, &f.policy, &r.id)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_revoke_queues_once_and_late_qualification_cannot_restore_or_refund() {
    let (f, r, admin, a, _m) = ready().await;
    let d = poll_runtime(&f).await;
    revoke(&f, &r, &admin).await;
    let stop = poll_runtime(&f).await;
    assert!(matches!(stop.command, ManagedRuntimeCommand::Stop(_)));
    let again = poll_runtime(&f).await;
    assert_eq!(stop.command_digest, again.command_digest);
    let late = submit_runtime(&f, &d, result(&d, "qualified", 200))
        .await
        .unwrap();
    assert_eq!(late.runtime.state, "cleanup_pending");
    assert!(!late.runtime.work_available);
    assert!(!late.runtime.capacity_released);
    let unknown = submit_runtime(&f, &again, result(&again, "unknown", 201))
        .await
        .unwrap();
    assert_eq!(unknown.runtime.process_state, "unknown");
    let stopped = submit_runtime(&f, &again, result(&again, "stopped", 202))
        .await
        .unwrap();
    assert_eq!(stopped.runtime.state, "stopped");
    assert!(!stopped.runtime.capacity_released);
    assert!(
        submit_runtime(&f, &d, result(&d, "qualified", 203))
            .await
            .is_err()
    );
    assert!(
        submit_runtime(&f, &again, result(&again, "unknown", 203))
            .await
            .is_err()
    );
    assert!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=2 AS matched FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_expiry_permanently_fences_without_replacement_command_or_implied_stop() {
    let (f, r, _admin, a, _m) = ready().await;
    let d = f
        .domain
        .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .expect("ready assignment must issue its original provision command");
    // PostgreSQL mints and enforces this deadline. A process-clock sleep can
    // finish before it, especially when PostgreSQL runs in a Docker VM.
    // Wait for actual database time; do not move the deadline or fake expiry.
    let wait_until = tokio::time::Instant::now() + std::time::Duration::from_secs(35);
    loop {
        let observed = f.domain.management_observed_at_ms().await.unwrap();
        let remaining = d.command.original().expires_at_ms - observed;
        if remaining <= 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < wait_until,
            "database clock did not reach the original provision deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(
            remaining.clamp(1, 250) as u64,
        ))
        .await;
    }
    f.domain.sweep_managed_runtime().await.unwrap();
    let stop = f
        .domain
        .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .expect("expired provision must issue its original cleanup command");
    assert!(matches!(stop.command, ManagedRuntimeCommand::Stop(_)));
    let old = f
        .domain
        .recover_managed_runtime(
            &f.owner.p,
            &f.policy,
            d.command.id(),
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!old.execution_allowed);
    let value = f
        .domain
        .managed_runtime(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.state, "cleanup_pending");
    assert_eq!(value.process_state, "unknown");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=$1 AND operation='agent.runtime.provision'").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_observation_rollback_and_cross_protocol_revision_collision() {
    let (f, r, admin, _a, _m) = ready().await;
    let d = poll_runtime(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_runtime_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fixture_runtime_reject BEFORE INSERT ON hagency_agent_v1.management_agent_runtime_observations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_runtime_reject()").await.unwrap();
    assert!(
        submit_runtime(&f, &d, result(&d, "qualified", 200))
            .await
            .is_err()
    );
    db.batch_execute("DROP TRIGGER fixture_runtime_reject ON hagency_agent_v1.management_agent_runtime_observations;DROP FUNCTION hagency_agent_v1.fixture_runtime_reject()").await.unwrap();
    assert!(sql_query("SELECT count(*)=0 AS matched FROM hagency_agent_v1.management_agent_runtime_observations WHERE command_id=$1").bind::<Text,_>(d.command.id()).get_result::<Check>(&mut db).await.unwrap().matched);
    let c = d.command.original();
    let base=sql_query("SELECT (receipt::jsonb->>'ledgerRevision')::bigint AS now FROM hagency_agent_v1.management_agent_executor_commands WHERE id=$1").bind::<Text,_>(&c.reserve_command.command_id).get_result::<Revision>(&mut db).await.unwrap().now;
    assert!(
        submit_runtime(&f, &d, result(&d, "qualified", base))
            .await
            .is_err()
    );
    submit_runtime(&f, &d, result(&d, "qualified", 200))
        .await
        .unwrap();
    revoke(&f, &r, &admin).await;
    let fence = poll(&f).await;
    let mut clash = fence_result(&fence, true);
    if let AgentExecutorReceipt::Fence(ref mut receipt) = clash.receipt {
        receipt.ledger_revision = 200;
        receipt.fence_revision = 200;
    }
    assert!(submit(&f, &fence, clash).await.is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v10_runtime_migration_is_atomic_and_preserves_assignment_and_matrix_identity() {
    let (f, r, _admin, a, _m) = ready().await;
    let before = read_matrix(&f, &r).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute(
        "CREATE TABLE hagency_agent_v1.management_agent_runtime_observations(dummy text)",
    )
    .await
    .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=10 AND to_regclass('hagency_agent_v1.management_agent_runtime_commands') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_runtime_observations")
        .await
        .unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let after = read_matrix(&f, &r).await;
    assert_eq!(before.puppet_mxid, after.puppet_mxid);
    assert_eq!(after.assignment_id, a.id);
    assert!(!after.work_available);
    assert!(
        f.domain
            .managed_runtime(&f.owner.p, &f.policy, &r.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_rejects_non_codex_canonical_resource_before_or_after_issuance() {
    let (f, r, _admin, _a, _m) = ready().await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_contributions SET runtime='claude' WHERE id=(SELECT contribution_id FROM hagency_agent_v1.management_projects WHERE id=$1)").bind::<Text,_>(&r.project_id).execute(&mut db).await.unwrap();
    assert!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    let (f, r, _admin, _a, _m) = ready().await;
    let d = poll_runtime(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_contributions SET runtime='octos' WHERE id=(SELECT contribution_id FROM hagency_agent_v1.management_projects WHERE id=$1)").bind::<Text,_>(&r.project_id).execute(&mut db).await.unwrap();
    let historical = f
        .domain
        .recover_managed_runtime(
            &f.owner.p,
            &f.policy,
            d.command.id(),
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!historical.execution_allowed);
    let reply = submit_runtime(&f, &historical, result(&historical, "qualified", 200))
        .await
        .unwrap();
    assert_eq!(reply.runtime.state, "cleanup_pending");
    assert!(!reply.runtime.work_available);
}

#[test]
fn runtime_canonical_fixtures_preserve_original_reserve_and_have_no_work_or_refund_claim() {
    let f: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-runtime-v1.json"
    ))
    .unwrap();
    for (kind, receipt) in [("provision", "qualified"), ("stop", "stopped")] {
        let c: ManagedRuntimeCommand =
            serde_json::from_value(f[format!("{kind}Command")].clone()).unwrap();
        c.validate("example.test").unwrap();
        assert_eq!(
            allocation_digest(&c).unwrap(),
            f[format!("{kind}CommandDigest")]
        );
        let r: ManagedRuntimeReceipt =
            serde_json::from_value(f[format!("{receipt}Receipt")].clone()).unwrap();
        r.validate_for(&c).unwrap();
        assert_eq!(
            allocation_digest(&r).unwrap(),
            f[format!("{receipt}ReceiptDigest")]
        );
        let mut bad = serde_json::to_value(&r).unwrap();
        bad["capacityReleased"] = json!(true);
        assert!(serde_json::from_value::<ManagedRuntimeReceipt>(bad).is_err());
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_concurrent_poll_and_generation_rotation_only_allow_original_cleanup() {
    let (f, _r, _admin, _a, _m) = ready().await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default()),
        other.poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
    );
    let a = a.unwrap().unwrap();
    let b = b.unwrap().unwrap();
    assert_eq!(a.command_digest, b.command_digest);
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
    assert!(p.device_generation > f.owner.p.device_generation);
    assert!(
        submit_runtime(&f, &b, result(&b, "qualified", 200))
            .await
            .is_err()
    );
    let historical = f
        .domain
        .recover_managed_runtime(
            &p,
            &f.policy,
            b.command.id(),
            ManagedRuntimeRecoverInput {
                command_digest: b.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!historical.execution_allowed);
    let reply = f
        .domain
        .record_managed_runtime(
            &p,
            &f.policy,
            b.command.id(),
            result(&historical, "qualified", 200),
        )
        .await
        .unwrap();
    assert_eq!(reply.runtime.state, "cleanup_pending");
    let cleanup = f
        .domain
        .poll_managed_runtime(&p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.execution_allowed);
    assert!(matches!(cleanup.command, ManagedRuntimeCommand::Stop(_)));
    let recovered = f
        .domain
        .recover_managed_runtime(
            &p,
            &f.policy,
            cleanup.command.id(),
            ManagedRuntimeRecoverInput {
                command_digest: cleanup.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!recovered.execution_allowed);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn runtime_stop_enqueue_failure_rolls_back_revocation_and_original_identity() {
    let (f, r, admin, a, _m) = ready().await;
    let d = poll_runtime(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_runtime_stop_failure() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF NEW.operation='agent.runtime.stop' THEN RAISE EXCEPTION 'fixture';END IF;RETURN NEW;END;$$;CREATE TRIGGER fixture_runtime_stop_failure BEFORE INSERT ON hagency_agent_v1.management_agent_runtime_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_runtime_stop_failure()").await.unwrap();
    assert!(
        f.domain
            .revoke_managed_agent_request(
                &admin.p,
                &f.policy,
                &r.id,
                RevokeInput {
                    command_id: "runtime_revoke_failure".into(),
                    expected_revision: r.revision
                }
            )
            .await
            .is_err()
    );
    assert!(sql_query("SELECT state='pending_native' AS matched FROM hagency_agent_v1.management_shared_assignments WHERE id=$1").bind::<Text,_>(&a.id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_runtime_stop_failure ON hagency_agent_v1.management_agent_runtime_commands;DROP FUNCTION hagency_agent_v1.fixture_runtime_stop_failure()").await.unwrap();
    revoke(&f, &r, &admin).await;
    let cleanup = poll_runtime(&f).await;
    let ManagedRuntimeCommand::Stop(c) = cleanup.command else {
        panic!("stop")
    };
    assert_eq!(c.provision_command_digest, d.command_digest);
}
