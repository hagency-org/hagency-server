#[path = "management_agent_settlement_tests.rs"]
mod settlement;
#[path = "management_agent_budget_tests.rs"]
mod budget;
#[path = "management_agent_cleanup_tests.rs"]
mod cleanup;
use super::*;
use crate::management_agent_executor_types::*;
#[path = "management_agent_matrix_tests.rs"]
mod matrix;

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    matrix::remove_migration(db).await;
    db.batch_execute("DROP INDEX hagency_agent_v1.managed_agent_executor_scope").await.unwrap();
    for (name, table) in [
        ("allocation", "management_allocations"),
        ("setup", "management_project_setups"),
        ("request", "management_agent_requests"),
        ("grant", "management_resource_grants"),
        ("contribution", "management_contributions"),
        ("plan", "management_projects"),
        ("project", "projects"),
        ("room", "rooms"),
        ("device", "devices"),
        ("session", "sessions"),
        ("user", "users"),
    ] {
        db.batch_execute(&format!(
            "DROP TRIGGER managed_agent_{name}_fence ON hagency_agent_v1.{table}"
        ))
        .await
        .unwrap();
    }
    db.batch_execute("DROP TRIGGER queue_agent_executor ON hagency_agent_v1.management_agent_intents;DROP TRIGGER fence_managed_binding ON hagency_agent_v1.bindings;DROP TABLE hagency_agent_v1.management_shared_assignments,hagency_agent_v1.management_agent_executor_commands;DROP FUNCTION hagency_agent_v1.queue_agent_executor(),hagency_agent_v1.retain_agent_executor(),hagency_agent_v1.validate_managed_assignment(),hagency_agent_v1.fence_managed_binding(),hagency_agent_v1.fence_agent_dependency();CREATE OR REPLACE FUNCTION hagency_agent_v1.fence_unimplemented_shared_agent() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF TG_OP='UPDATE' AND NEW.execution_kind<>OLD.execution_kind THEN RAISE EXCEPTION 'immutable_agent_execution_kind'; END IF; IF NEW.execution_kind<>'personal' THEN RAISE EXCEPTION 'managed_agent_native_receipt_required'; END IF; RETURN NEW; END; $$;ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=8;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=8)").await.unwrap();
}
async fn approved() -> (Fixture, ManagedAgentRequest, Actor) {
    let (mut f, p, s, admin, gw) = ready_agent().await;
    f.policy = f.policy.with_agent_executor(true);
    let r = request(&f, &p, &s, &gw).await;
    let r = f
        .domain
        .decide_managed_agent(
            &admin.p,
            &f.policy,
            &r.id,
            decide(&r, "executor_approve"),
            &gw,
        )
        .await
        .unwrap();
    (f, r, admin)
}
async fn poll(f: &Fixture) -> AgentExecutorDelivery {
    f.domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
fn reservation(d: &AgentExecutorDelivery) -> AgentExecutorResultInput {
    let AgentExecutorCommand::Reserve(c) = &d.command else {
        panic!("reserve required")
    };
    AgentExecutorResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentExecutorReceipt::Reserve(ManagedAgentReservationReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            request_id: c.request_id.clone(),
            agent_id: c.proposed_agent_id.clone(),
            allocation_id: c.allocation_id.clone(),
            agent_reservation_id: Some(format!("child_{}", c.proposed_agent_id)),
            definition_digest: c.definition_digest.clone(),
            state: "reserved".into(),
            budget_revision: 1,
            ledger_revision: 100,
            allocated_tokens: c.requested_tokens,
            retained_tokens: c.requested_tokens,
            period_key: c.period_key.clone(),
            reason: None,
        }),
    }
}
fn id(d: &AgentExecutorDelivery) -> &str {
    match &d.command {
        AgentExecutorCommand::Reserve(c) => &c.command_id,
        AgentExecutorCommand::Fence(c) => &c.command_id,
    }
}
fn fence_result(d: &AgentExecutorDelivery, reserved: bool) -> AgentExecutorResultInput {
    let AgentExecutorCommand::Fence(c) = &d.command else {
        panic!("fence required")
    };
    AgentExecutorResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentExecutorReceipt::Fence(ManagedAgentFenceReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            reserve_command_digest: c.reserve_command_digest.clone(),
            request_id: c.reserve_command.request_id.clone(),
            agent_id: c.reserve_command.proposed_agent_id.clone(),
            agent_reservation_id: reserved
                .then(|| format!("child_{}", c.reserve_command.proposed_agent_id)),
            state: if reserved { "fenced" } else { "absent_fenced" }.into(),
            fence_revision: 101,
            ledger_revision: 101,
            known_consumed_tokens: 0,
            retained_tokens: if reserved { None } else { Some(0) },
            usage_complete: !reserved,
            process_state: if reserved { "unknown" } else { "not_started" }.into(),
        }),
    }
}
async fn submit(
    f: &Fixture,
    d: &AgentExecutorDelivery,
    r: AgentExecutorResultInput,
) -> crate::Result<AgentExecutorAcknowledgement> {
    f.domain
        .record_managed_agent_executor_result(&f.owner.p, &f.policy, id(d), r)
        .await
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_exact_http_delivery_replay_and_personal_isolation() {
    let (f, r, _) = approved().await;
    let disabled = f.policy.clone().with_agent_executor(false);
    assert_eq!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &disabled, ExecutorPollInput::default())
            .await
            .unwrap_err()
            .to_string(),
        "agent_executor_unavailable"
    );
    for p in [&f.outsider.p, &f.admin.p, &f.coordinator.p] {
        assert!(
            f.domain
                .poll_managed_agent_executor(p, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
    }
    let s = service(&f, true);
    let reply = post(
        &s,
        "agent-executor/poll",
        &f.owner.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let d: AgentExecutorDelivery = serde_json::from_value(reply["delivery"].clone()).unwrap();
    assert!(d.execution_allowed);
    assert!(
        f.domain
            .recover_managed_agent_executor(
                &f.outsider.p,
                &f.policy,
                id(&d),
                ExecutorRecoverInput {
                    command_digest: d.command_digest.clone()
                }
            )
            .await
            .is_err()
    );
    let mut bad = reservation(&d);
    bad.command_digest = "f".repeat(64);
    assert!(submit(&f, &d, bad).await.is_err());
    let body = serde_json::to_value(reservation(&d)).unwrap();
    let value = post(
        &s,
        &format!("agent-executor/{}/result", id(&d)),
        &f.owner.token,
        body.clone(),
        StatusCode::OK,
    )
    .await;
    let assignment: PendingManagedAssignment =
        serde_json::from_value(value["assignment"].clone()).unwrap();
    assert_eq!(assignment.manager_mxid, f.outsider.p.mxid);
    assert!(!assignment.execution_available);
    assert_eq!(assignment.state, "pending_native");
    let replay = post(
        &s,
        &format!("agent-executor/{}/result", id(&d)),
        &f.owner.token,
        body,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay["assignment"]["id"], assignment.id);
    let recovery = f
        .domain
        .recover_managed_agent_executor(
            &f.owner.p,
            &f.policy,
            id(&d),
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recovery.execution_allowed);
    assert!(recovery.recorded.is_some());
    assert!(
        f.domain
            .agent(&f.outsider.p, &r.proposed_agent_id, now_ms())
            .await
            .is_err()
    );
    assert!(
        f.domain
            .set_execution_device(
                &f.outsider.p,
                &r.proposed_agent_id,
                crate::domain::SetExecutionDevice {
                    expected_generation: 1
                },
                now_ms()
            )
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT execution_kind='shared_project' AND state='suspended' AND owner_user_id=$2 AND execution_device_id IS NULL AS matched FROM hagency_agent_v1.agents WHERE id=$1").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&f.outsider.p.user_id).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(
        sql_query("UPDATE hagency_agent_v1.agents SET state='active' WHERE id=$1")
            .bind::<Text, _>(&r.proposed_agent_id)
            .execute(&mut db)
            .await
            .is_err()
    );
    assert!(sql_query("UPDATE hagency_agent_v1.agents SET execution_device_id=$2,generation=generation+1 WHERE id=$1").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.is_err());
    assert!(sql_query("INSERT INTO hagency_agent_v1.bindings(id,agent_id,project_id,room_id,state) SELECT $2,$1,domain_project_id,room_id,'joining' FROM hagency_agent_v1.management_agent_requests WHERE id=$3").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(format!("bad_{}",secret_token())).bind::<Text,_>(&r.id).execute(&mut db).await.is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_cancel_unknown_queues_once_and_late_receipt_never_activates() {
    let (f, r, _) = approved().await;
    let d = poll(&f).await;
    let input = RevokeInput {
        command_id: "cancel_inflight_agent".into(),
        expected_revision: r.revision,
    };
    f.domain
        .cancel_managed_agent_request(&f.outsider.p, &f.policy, &r.id, input.clone())
        .await
        .unwrap();
    f.domain
        .cancel_managed_agent_request(&f.outsider.p, &f.policy, &r.id, input)
        .await
        .unwrap();
    let late = submit(&f, &d, reservation(&d)).await.unwrap();
    assert!(late.assignment.is_none());
    let cleanup = poll(&f).await;
    assert!(cleanup.execution_allowed);
    submit(&f, &cleanup, fence_result(&cleanup, true))
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.fence')=1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=$2) AS matched").bind::<Text,_>(&r.id).bind::<Text,_>(&r.proposed_agent_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_cleanup_before_reserve_and_tombstone_conflict() {
    for exists in [false, true] {
        let (f, r, _) = approved().await;
        let d = poll(&f).await;
        f.domain
            .cancel_managed_agent_request(
                &f.outsider.p,
                &f.policy,
                &r.id,
                RevokeInput {
                    command_id: "cancel_before_result".into(),
                    expected_revision: r.revision,
                },
            )
            .await
            .unwrap();
        let cleanup = poll(&f).await;
        submit(&f, &cleanup, fence_result(&cleanup, exists))
            .await
            .unwrap();
        let late = submit(&f, &d, reservation(&d)).await;
        if exists {
            assert!(late.unwrap().assignment.is_none());
        } else {
            assert_eq!(
                late.unwrap_err().to_string(),
                "agent_executor_fence_conflict"
            );
        }
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_only_manager_revokes_and_cleanup_preserves_unknown() {
    let (f, r, admin) = approved().await;
    let d = poll(&f).await;
    submit(&f, &d, reservation(&d)).await.unwrap();
    let input = RevokeInput {
        command_id: "revoke_agent".into(),
        expected_revision: r.revision,
    };
    for p in [&f.owner.p, &f.admin.p, &f.coordinator.p] {
        assert_eq!(
            f.domain
                .revoke_managed_agent_request(p, &f.policy, &r.id, input.clone())
                .await
                .unwrap_err()
                .to_string(),
            "assigned_project_administrator_required"
        );
    }
    f.domain
        .revoke_managed_agent_request(&admin.p, &f.policy, &r.id, input)
        .await
        .unwrap();
    let cleanup = poll(&f).await;
    let reply = submit(&f, &cleanup, fence_result(&cleanup, true))
        .await
        .unwrap();
    assert_eq!(reply.assignment.unwrap().state, "fenced");
    let recovered = f
        .domain
        .recover_managed_agent_executor(
            &f.owner.p,
            &f.policy,
            id(&cleanup),
            ExecutorRecoverInput {
                command_digest: cleanup.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    let Some(AgentExecutorReceipt::Fence(receipt)) = recovered.recorded else {
        panic!("fence")
    };
    assert_eq!(receipt.retained_tokens, None);
    assert!(!receipt.usage_complete);
    assert_eq!(receipt.process_state, "unknown");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_atomic_result_rollback_and_fresh_generation_cleanup() {
    let (f, r, _) = approved().await;
    let d = poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fail_agent_assignment_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'rollback_fixture'; END $$;CREATE TRIGGER fail_agent_assignment_test BEFORE INSERT ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_agent_assignment_test()").await.unwrap();
    assert!(submit(&f, &d, reservation(&d)).await.is_err());
    assert!(sql_query("SELECT receipt IS NULL AS matched FROM hagency_agent_v1.management_agent_executor_commands WHERE id=$1").bind::<Text,_>(id(&d)).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fail_agent_assignment_test ON hagency_agent_v1.management_shared_assignments;DROP FUNCTION hagency_agent_v1.fail_agent_assignment_test()").await.unwrap();
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
    assert!(submit(&f, &d, reservation(&d)).await.is_err());
    let recover = f
        .domain
        .recover_managed_agent_executor(
            &p,
            &f.policy,
            id(&d),
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recover.execution_allowed);
    let mut result = reservation(&d);
    result.token = recover.token;
    assert!(
        f.domain
            .record_managed_agent_executor_result(&p, &f.policy, id(&d), result)
            .await
            .unwrap()
            .assignment
            .is_none()
    );
    let cleanup = f
        .domain
        .poll_managed_agent_executor(&p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.execution_allowed);
    assert_eq!(
        match cleanup.command {
            AgentExecutorCommand::Fence(c) => c.reserve_command.request_id,
            _ => panic!("fence"),
        },
        r.id
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_result_races_cancel_and_expired_ticket_recovers_without_execution() {
    let (f, r, _) = approved().await;
    let d = poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_agent_executor_commands SET lease_until_ms=1 WHERE id=$1").bind::<Text,_>(id(&d)).execute(&mut db).await.unwrap();
    assert_eq!(
        submit(&f, &d, reservation(&d))
            .await
            .unwrap_err()
            .to_string(),
        "agent_executor_ticket_required"
    );
    let recovered = f
        .domain
        .recover_managed_agent_executor(
            &f.owner.p,
            &f.policy,
            id(&d),
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recovered.execution_allowed);
    let mut result = reservation(&d);
    result.token = recovered.token;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.domain
            .record_managed_agent_executor_result(&f.owner.p, &f.policy, id(&d), result),
        other.cancel_managed_agent_request(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "race_cancel".into(),
                expected_revision: r.revision
            }
        )
    );
    a.unwrap();
    b.unwrap();
    if let Some(a) = f
        .domain
        .managed_agent_assignment(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
    {
        assert_eq!(a.state, "fenced");
        assert!(!a.execution_available);
    }
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=$1 AND state='active') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1) AND (SELECT count(*) FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$2 AND w.operation='agent.fence')=1 AS matched").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_cancel_queue_failure_rolls_back_and_resource_withdrawal_fences() {
    let (f, r, _) = approved().await;
    let d = poll(&f).await;
    submit(&f, &d, reservation(&d)).await.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fail_agent_cleanup_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.operation='agent.fence' THEN RAISE EXCEPTION 'rollback_fixture'; END IF; RETURN NEW; END $$;CREATE TRIGGER fail_agent_cleanup_test BEFORE INSERT ON hagency_agent_v1.management_agent_executor_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_agent_cleanup_test()").await.unwrap();
    assert!(
        f.domain
            .cancel_managed_agent_request(
                &f.outsider.p,
                &f.policy,
                &r.id,
                RevokeInput {
                    command_id: "queue_rollback".into(),
                    expected_revision: r.revision
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .managed_agent_assignment(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "pending_native"
    );
    assert_eq!(
        f.domain
            .management_agent_request(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .state,
        "approved_waiting_executor"
    );
    db.batch_execute("DROP TRIGGER fail_agent_cleanup_test ON hagency_agent_v1.management_agent_executor_commands;DROP FUNCTION hagency_agent_v1.fail_agent_cleanup_test()").await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1 WHERE id IN (SELECT a.grant_id FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_agent_requests r ON r.allocation_id=a.id WHERE r.id=$1)").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    assert_eq!(
        f.domain
            .managed_agent_assignment(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "fenced"
    );
    assert!(matches!(
        poll(&f).await.command,
        AgentExecutorCommand::Fence(_)
    ));
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v8_agent_executor_migration_rolls_back_preserves_personal_and_backfills_intents() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "agent_executor_upgrade_{}",
        &crate::hash(&secret_token())[..12]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
  let auth=Store::open(url.as_str(),"example.test","https://example.test/auth/").await.unwrap();
  let domain=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();let owner=actor(&auth,"before_executor").await;
  let original=domain.create_agent(&owner.p,CreateAgent {display_name:"Original".into(),idempotency_key:"before_executor".into()},now_ms()).await.unwrap();
  let mut f=fixture_at(url.to_string()).await;f.policy=f.policy.with_project_reservations(true).with_project_setup(true).with_agent_admission(true).with_agent_executor(true);
  let admitted=admitted(&f).await;let plan=approved_plan(&f,&admitted,&f.outsider.p).await;
  f.domain.publish_management_grant(&f.owner.p,&f.policy,&admitted.contribution.id,grant_input(&f,&admitted,&f.outsider.p).await).await.unwrap();
  let allocation=allocation(&f,&plan).await;accepted(&f,&allocation).await;let delivery=delivered(&f).await;let allocation=record(&f,&delivery,"reserved",11).await.allocation;
  let admin=same_actor(&f.auth,&f.outsider).await;f.domain.assign_management_project_admin(&f.admin.p,&f.policy,&plan.id,assign(&admin)).await.unwrap();
  let setup=f.domain.begin_management_project_setup(&f.outsider.p,&f.policy,&plan.id,begin(&plan,&allocation)).await.unwrap().setup.unwrap();let (ready,gw)=confirm_ok(&f,&setup).await;let setup=ready.setup.unwrap();
  let request=request(&f,&plan,&setup,&gw).await;let request=f.domain.decide_managed_agent(&admin.p,&f.policy,&request.id,decide(&request,"old_approval"),&gw).await.unwrap();
  f.domain.cancel_managed_agent_request(&f.outsider.p,&f.policy,&request.id,RevokeInput {command_id:"old_cancel".into(),expected_revision:request.revision}).await.unwrap();
  let mut db=AsyncPgConnection::establish(url.as_str()).await.unwrap();
  let before=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&request.id).get_result::<Payload>(&mut db).await.unwrap().payload;
  remove_migration(&mut db).await;
  db.batch_execute("CREATE TABLE hagency_agent_v1.management_agent_executor_commands(id text)").await.unwrap();assert!(DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.is_err());
  assert!(sql_query("SELECT version=8 AND to_regclass('hagency_agent_v1.management_shared_assignments') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
  db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_executor_commands").await.unwrap();
  for _ in 0..2 {DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();assert_eq!(domain.agent(&owner.p,&original.id,now_ms()).await.unwrap().owner_user_id,owner.p.user_id);}
  let after=sql_query("SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&request.id).get_result::<Payload>(&mut db).await.unwrap().payload;assert_eq!(before,after);
  assert!(sql_query("SELECT count(*)=2 AS matched FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1").bind::<Text,_>(&request.id).get_result::<Check>(&mut db).await.unwrap().matched);
 }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}
#[test]
fn agent_fence_canonical_fixture_is_separate_and_cannot_claim_unknown_refunded() {
    let value: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-fence-v1.json"
    ))
    .unwrap();
    let c: ManagedAgentFenceCommand = serde_json::from_value(value["command"].clone()).unwrap();
    c.validate("example.test").unwrap();
    let r: ManagedAgentFenceReceipt = serde_json::from_value(value["receipt"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    assert_eq!(allocation_digest(&c).unwrap(), value["commandDigest"]);
    assert_eq!(allocation_digest(&r).unwrap(), value["receiptDigest"]);
    let mut bad = r.clone();
    bad.usage_complete = true;
    assert!(bad.validate_for(&c).is_err());
    let mut bad = r.clone();
    bad.state = "absent_fenced".into();
    assert!(bad.validate_for(&c).is_err());
    let mut bad = value["receipt"].clone();
    bad["executorActor"] = "forged".into();
    assert!(serde_json::from_value::<AgentExecutorReceipt>(bad).is_err());
    let mut maximum = c;
    maximum.reserve_command.definition.description = "\"".repeat(2000);
    maximum.reserve_command.definition_digest =
        allocation_digest(&maximum.reserve_command.definition).unwrap();
    maximum.reserve_command_digest = allocation_digest(&maximum.reserve_command).unwrap();
    maximum.validate("example.test").unwrap();
    assert!(serde_json::to_vec(&maximum).unwrap().len() < 16 * 1024);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_expired_allocation_sweeps_cleanup_but_policy_pause_does_not_revoke() {
    let (mut f, r, _) = approved().await;
    f.policy = f
        .policy
        .with_agent_admission(false)
        .with_project_reservations(false);
    let paused = poll(&f).await;
    assert!(!paused.execution_allowed);
    assert!(matches!(paused.command, AgentExecutorCommand::Reserve(_)));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT state<>'fenced' AS matched FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
    f.policy = f
        .policy
        .with_agent_admission(true)
        .with_project_reservations(true);
    sql_query("UPDATE hagency_agent_v1.management_allocations SET expires_at_ms=1 WHERE id IN (SELECT allocation_id FROM hagency_agent_v1.management_agent_requests WHERE id=$1)").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    // The unchanged 'reserved' state is insufficient; polling detects expiry
    // without waiting for a request GET or fabricating a native receipt.
    let cleanup = poll(&f).await;
    assert!(cleanup.execution_allowed);
    assert!(matches!(cleanup.command, AgentExecutorCommand::Fence(_)));
    assert!(
        f.domain
            .managed_agent_assignment(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_project_receipt_revision_collision_rejected_in_both_arrival_orders() {
    for agent_first in [true, false] {
        let (f, r, _) = approved().await;
        let agent = poll(&f).await;
        if agent_first {
            submit(&f, &agent, reservation(&agent)).await.unwrap();
        }
        let allocation = f
            .domain
            .management_project_allocation(&f.outsider.p, &f.policy, &r.project_id)
            .await
            .unwrap()
            .unwrap();
        f.domain
            .release_management_allocation(
                &f.outsider.p,
                &f.policy,
                &allocation.id,
                RevokeInput {
                    command_id: "release_collision".into(),
                    expected_revision: allocation.revision,
                },
            )
            .await
            .unwrap();
        let release = delivered(&f).await;
        // Refusal is a distinct durable ledger mutation too. It must not be
        // mistaken for a successful release or share the agent mutation's ID.
        let release_result = result(&release, "refused", 100);
        if agent_first {
            assert_eq!(
                f.domain
                    .record_management_executor_result(
                        &f.owner.p,
                        &f.policy,
                        &release.command.command_id,
                        release_result
                    )
                    .await
                    .unwrap_err()
                    .to_string(),
                "agent_executor_ledger_revision_changed"
            );
        } else {
            f.domain
                .record_management_executor_result(
                    &f.owner.p,
                    &f.policy,
                    &release.command.command_id,
                    release_result,
                )
                .await
                .unwrap();
            assert_eq!(
                submit(&f, &agent, reservation(&agent))
                    .await
                    .unwrap_err()
                    .to_string(),
                "agent_executor_ledger_revision_changed"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_parent_receipt_revision_collision_rejected_in_both_arrival_orders() {
    use crate::management_parent_types::*;
    for agent_first in [true, false] {
        let (mut f, r, _) = approved().await;
        f.policy = f.policy.with_parent_settlement(true);
        let agent = poll(&f).await;
        if agent_first {
            submit(&f, &agent, reservation(&agent)).await.unwrap();
        }
        let allocation = f
            .domain
            .management_project_allocation(&f.outsider.p, &f.policy, &r.project_id)
            .await
            .unwrap()
            .unwrap();
        let g = f
            .domain
            .management_resource_grant(&f.owner.p, &f.policy, &allocation.grant_id)
            .await
            .unwrap();
        f.domain
            .close_management_parent(
                &f.owner.p,
                &f.policy,
                &g.id,
                ParentCloseInput {
                    command_id: "parent_collision".into(),
                    expected_grant_revision: g.revision,
                },
            )
            .await
            .unwrap();
        let parent = f
            .domain
            .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .unwrap();
        let report = ParentResultInput {
            command_digest: parent.command_digest.clone(),
            token: parent.token.clone(),
            receipt: NativeGrantSettlementReceipt {
                version: 1,
                command_id: parent.command.command_id.clone(),
                command_digest: parent.command_digest.clone(),
                grant_id: g.id,
                grant_receipt_digest: g.receipt_digest,
                enrollment_id: g.receipt.enrollment_id,
                period_key: g.receipt.period_key,
                fence_revision: 100,
                ledger_revision: 100,
                state: "retained".into(),
                retained_tokens: 100_000,
                known_consumed_tokens: 0,
                unresolved_tokens: 100_000,
                liability_count: 1,
                liability_digest: "b".repeat(64),
            },
        };
        if agent_first {
            assert_eq!(
                f.domain
                    .record_management_parent_result(
                        &f.owner.p,
                        &f.policy,
                        &parent.command.command_id,
                        report
                    )
                    .await
                    .unwrap_err()
                    .to_string(),
                "agent_executor_ledger_revision_changed"
            );
        } else {
            f.domain
                .record_management_parent_result(
                    &f.owner.p,
                    &f.policy,
                    &parent.command.command_id,
                    report,
                )
                .await
                .unwrap();
            assert_eq!(
                submit(&f, &agent, reservation(&agent))
                    .await
                    .unwrap_err()
                    .to_string(),
                "agent_executor_ledger_revision_changed"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn agent_executor_native_child_identity_cannot_be_reused_by_reserve_or_cleanup() {
    let (mut f, p, s, admin, gw) = ready_agent().await;
    f.policy = f.policy.with_agent_executor(true);
    for key in ["first_agent", "second_agent"] {
        let r = f
            .domain
            .request_managed_agent(
                &f.outsider.p,
                &f.policy,
                &p.id,
                input(&f, &p, &s, key).await,
                &gw,
            )
            .await
            .unwrap();
        f.domain
            .decide_managed_agent(
                &admin.p,
                &f.policy,
                &r.id,
                decide(&r, &format!("approve_{key}")),
                &gw,
            )
            .await
            .unwrap();
    }
    let first = poll(&f).await;
    let first_result = reservation(&first);
    let AgentExecutorReceipt::Reserve(ref receipt) = first_result.receipt else {
        panic!("reserve")
    };
    let child = receipt.agent_reservation_id.clone();
    submit(&f, &first, first_result).await.unwrap();
    let second = poll(&f).await;
    let mut bad = reservation(&second);
    let AgentExecutorReceipt::Reserve(ref mut receipt) = bad.receipt else {
        panic!("reserve")
    };
    receipt.agent_reservation_id = child.clone();
    receipt.ledger_revision = 101;
    assert_eq!(
        submit(&f, &second, bad).await.unwrap_err().to_string(),
        "agent_executor_child_reused"
    );
    let AgentExecutorCommand::Reserve(ref command) = second.command else {
        panic!("reserve")
    };
    f.domain
        .cancel_managed_agent_request(
            &f.outsider.p,
            &f.policy,
            &command.request_id,
            RevokeInput {
                command_id: "cancel_second".into(),
                expected_revision: command.request_revision,
            },
        )
        .await
        .unwrap();
    let cleanup = poll(&f).await;
    let mut bad = fence_result(&cleanup, true);
    let AgentExecutorReceipt::Fence(ref mut receipt) = bad.receipt else {
        panic!("fence")
    };
    receipt.agent_reservation_id = child;
    receipt.ledger_revision = 102;
    receipt.fence_revision = 102;
    assert_eq!(
        submit(&f, &cleanup, bad).await.unwrap_err().to_string(),
        "agent_executor_child_reused"
    );
}
