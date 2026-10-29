use super::*;

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    intake_tests::remove_migration(db).await;
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_result_tickets;DROP FUNCTION hagency_agent_v1.retain_shared_result_ticket();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=14;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=14)").await.unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_v1_original_execute_survives_history_poll_and_concurrent_reads() {
    let (f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let original_until = d.lease_until_ms;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (observed, polled) = tokio::join!(
        history(&f, &d),
        other.poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
    );
    assert!(!observed.execution_allowed);
    assert!(!observed.cleanup_allowed);
    assert_eq!(polled.unwrap().unwrap().command_digest, d.command_digest);
    let service = svc(&f, m.gateway.clone());
    let ack = post(
        &service,
        &format!("shared-execution/runtime/{}/result", d.command.command_id),
        &f.owner.token,
        serde_json::to_value(evidence(&d, "qualified", 200)).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(ack["state"], "qualified");
    assert!(ack["activity"].is_object());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT expires_at_ms=$3 AND mode='execute' AS matched FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND token_hash=$2").bind::<Text,_>(&d.command.command_id).bind::<Text,_>(crate::hash(&d.token)).bind::<BigInt,_>(original_until).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_observation_never_mints_activity_and_wrong_scope_is_denied() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let observed = history(&f, &d).await;
    let mut bad = evidence(&d, "qualified", 200);
    bad.token = "0".repeat(64);
    assert_eq!(
        f.domain
            .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .unwrap_err()
            .to_string(),
        "shared_result_ticket_changed"
    );
    let mut bad = evidence(&d, "qualified", 200);
    bad.command_digest = "0".repeat(64);
    assert!(
        f.domain
            .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .record_shared_process(
                &f.outsider.p,
                &f.policy,
                &d.command.command_id,
                evidence(&d, "qualified", 200)
            )
            .await
            .is_err()
    );
    let ack = save(&f, &observed, "qualified", 200).await.unwrap();
    assert!(ack.activity.is_none());
    assert_eq!(ack.state, "issued");
    let recovery = history(&f, &d).await;
    assert!(!recovery.execution_allowed);
    assert!(!recovery.cleanup_allowed);
    let polled = shared_poll(&f).await;
    assert!(!polled.execution_allowed);
    assert!(!polled.cleanup_allowed);
    assert!(
        save(&f, &polled, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_none()
    );
    // Only the original still-live execute ticket may promote this same receipt.
    let promoted = save(&f, &d, "qualified", 200).await.unwrap();
    assert_eq!(promoted.state, "qualified");
    assert!(promoted.activity.is_some());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_revoke_and_replaced_generation_fence_all_original_execute_tickets() {
    let (f, r, admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let _observer = history(&f, &d).await;
    revoke(&f, &r, &admin).await;
    let ack = save(&f, &d, "qualified", 200).await.unwrap();
    assert!(ack.activity.is_none());
    assert_eq!(ack.state, "cleanup_pending");
    let cleanup = shared_poll(&f).await;
    assert!(cleanup.cleanup_allowed);
    assert_eq!(
        save(&f, &cleanup, "stopped", 201).await.unwrap().state,
        "stopped"
    );
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let token = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Replacement".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let current = f.auth.authenticate(&token, now_ms(), true).await.unwrap();
    assert!(current.device_generation.unwrap() > f.owner.p.device_generation.unwrap());
    assert_eq!(
        f.domain
            .record_shared_process(
                &current,
                &f.policy,
                &d.command.command_id,
                evidence(&d, "qualified", 200)
            )
            .await
            .unwrap_err()
            .to_string(),
        "shared_result_ticket_changed"
    );
    let cleanup = f
        .domain
        .poll_shared_runtime(&current, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
    let ack = f
        .domain
        .record_shared_process(
            &current,
            &f.policy,
            &d.command.command_id,
            evidence(&cleanup, "stopped", 201),
        )
        .await
        .unwrap();
    assert!(ack.activity.is_none());
    assert_eq!(ack.state, "stopped");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_live_cap_never_evicts_original_and_expired_gc_preserves_history() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Sixty-three legitimate concurrent observers fill the bounded capability set.
    for _ in 0..63 {
        history(&f, &d).await;
    }
    let overflow = f
        .domain
        .recover_shared_runtime(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(overflow.to_string(), "shared_result_ticket_capacity");
    assert!(
        save(&f, &d, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_some()
    );
    assert!(
        sql_query(
            "DELETE FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1"
        )
        .bind::<Text, _>(&d.command.command_id)
        .execute(&mut db)
        .await
        .is_err()
    );
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_result_tickets SET expires_at_ms=expires_at_ms+1 WHERE runtime_id=$1").bind::<Text,_>(&d.command.command_id).execute(&mut db).await.is_err());
    // A separate runtime has one expired token and one still-valid execute token.
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let old = now_ms() - 1000;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_result_tickets(runtime_id,token_hash,executor_device_id,executor_generation,mode,issued_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,'observe',$5,$6)").bind::<Text,_>(&d.command.command_id).bind::<Text,_>(crate::hash("expired-fixture-token")).bind::<Text,_>(f.owner.p.device_id.as_deref().unwrap()).bind::<BigInt,_>(f.owner.p.device_generation.unwrap()).bind::<BigInt,_>(old-1000).bind::<BigInt,_>(old).execute(&mut db).await.unwrap();
    let mut expired = evidence(&d, "qualified", 200);
    expired.token = "expired-fixture-token".into();
    assert_eq!(
        f.domain
            .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, expired)
            .await
            .unwrap_err()
            .to_string(),
        "shared_result_ticket_changed"
    );
    history(&f, &d).await;
    assert!(sql_query("SELECT count(*)=2 AS matched FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(
        save(&f, &d, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_some()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_schema14_atomic_upgrade_preserves_pending_original_ticket() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.retain_shared_result_ticket() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN OLD;END;$$").await.unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=14 AND to_regclass('hagency_agent_v1.management_shared_result_tickets') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP FUNCTION hagency_agent_v1.retain_shared_result_ticket()")
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let ack = reopened
        .record_shared_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "qualified", 200),
        )
        .await
        .unwrap();
    assert!(ack.activity.is_some());
    assert_eq!(history(&f, &d).await.command_digest, d.command_digest);
    assert!(
        sql_query("SELECT version=28 AS matched FROM hagency_agent_v1.management_deployment")
            .get_result::<Check>(&mut db)
            .await
            .unwrap()
            .matched
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_two_store_cap_is_atomic_and_original_ticket_remains_valid() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    for _ in 0..62 {
        history(&f, &d).await;
    }
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let request = ManagedRuntimeRecoverInput {
        command_digest: d.command_digest.clone(),
        observation_id: None,
    };
    let (left, right) = tokio::join!(
        f.domain.recover_shared_runtime(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            request.clone()
        ),
        other.recover_shared_runtime(&f.owner.p, &f.policy, &d.command.command_id, request)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let error = if let Err(e) = left {
        e
    } else {
        right.unwrap_err()
    };
    assert_eq!(error.to_string(), "shared_result_ticket_capacity");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=64 AS matched FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(
        save(&f, &d, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_some()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_ticket_migration_never_rebinds_legacy_execute_or_cleanup_to_new_generation() {
    for cleanup_first in [false, true] {
        let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
        let d = shared_poll(&f).await;
        let token = f
            .auth
            .register_device(
                &f.owner.session,
                RegisterDevice {
                    installation_id: "macbook".into(),
                    name: "New generation".into(),
                },
                now_ms(),
            )
            .await
            .unwrap()
            .token;
        let current = f.auth.authenticate(&token, now_ms(), true).await.unwrap();
        let old = if cleanup_first {
            f.domain
                .poll_shared_runtime(&current, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .unwrap()
        } else {
            d.clone()
        };
        assert_eq!(old.cleanup_allowed, cleanup_first);
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        remove_migration(&mut db).await;
        let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap();
        assert!(sql_query("SELECT executor_generation=$3 AS matched FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND token_hash=$2").bind::<Text,_>(&d.command.command_id).bind::<Text,_>(crate::hash(&old.token)).bind::<BigInt,_>(f.owner.p.device_generation.unwrap()).get_result::<Check>(&mut db).await.unwrap().matched);
        assert_eq!(
            reopened
                .record_shared_process(
                    &current,
                    &f.policy,
                    &d.command.command_id,
                    evidence(&old, "stopped", 201)
                )
                .await
                .unwrap_err()
                .to_string(),
            "shared_result_ticket_changed"
        );
        let recovered = reopened
            .recover_shared_runtime(
                &current,
                &f.policy,
                &d.command.command_id,
                ManagedRuntimeRecoverInput {
                    command_digest: d.command_digest.clone(),
                    observation_id: None,
                },
            )
            .await
            .unwrap();
        assert!(!recovered.execution_allowed);
        assert!(!recovered.cleanup_allowed);
        let ack = reopened
            .record_shared_process(
                &current,
                &f.policy,
                &d.command.command_id,
                evidence(&recovered, "stopped", 201),
            )
            .await
            .unwrap();
        assert_eq!(ack.state, "stopped");
        assert!(ack.activity.is_none());
    }
}
