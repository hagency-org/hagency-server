use super::*;
use crate::management_agent_settlement_types::*;
use crate::management_allocation_types::{ExecutorRecoverInput, allocation_digest};
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_project_manager_remove_test.sql")).await.unwrap();
    db.batch_execute("DROP VIEW IF EXISTS hagency_agent_v1.management_agent_settlement_evidence;DROP TABLE IF EXISTS hagency_agent_v1.management_agent_settlement_tickets,hagency_agent_v1.management_agent_settlement_issuance,hagency_agent_v1.management_agent_settlement_results,hagency_agent_v1.management_agent_settlement_commands;DROP FUNCTION IF EXISTS hagency_agent_v1.validate_agent_settlement();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=20;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=20)").await.unwrap();
}
async fn fenced(reserved: bool) -> (Fixture, ManagedAgentRequest, Actor, AgentExecutorDelivery) {
    let (f, r, a) = approved().await;
    let d = poll(&f).await;
    if reserved {
        submit(&f, &d, reservation(&d)).await.unwrap();
    }
    let r = f
        .domain
        .revoke_managed_agent_request(
            &a.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "settlement_revoke".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
    let d = poll(&f).await;
    submit(&f, &d, fence_result(&d, reserved)).await.unwrap();
    (f, r, a, d)
}
pub(super) async fn poll_settlement(f: &Fixture) -> AgentSettlementDelivery {
    f.domain
        .poll_agent_settlement(&f.owner.p, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn result(
    f: &Fixture,
    d: &AgentSettlementDelivery,
    ledger: i64,
    spent: i64,
    held: i64,
    complete: bool,
) -> AgentSettlementResult {
    let c = &d.command;
    let fc = &c.fence_command;
    let original = &fc.reserve_command;
    let child = f
        .domain
        .managed_agent_assignment(&f.owner.p, &f.policy, &original.request_id)
        .await
        .unwrap();
    let budget = if child.is_some() {
        Some(
            f.domain
                .agent_budget(&f.outsider.p, &f.policy, &original.request_id)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    AgentSettlementResult {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: ManagedAgentSettlementReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            fence_receipt_digest: c.fence_receipt_digest.clone(),
            reserve_command_digest: fc.reserve_command_digest.clone(),
            request_id: original.request_id.clone(),
            agent_id: original.proposed_agent_id.clone(),
            agent_reservation_id: child.map(|c| c.agent_reservation_id),
            period_key: original.period_key.clone(),
            fence_revision: 101,
            ledger_revision: ledger,
            state: if complete { "settled" } else { "unresolved" }.into(),
            process_state: if budget.is_none() {
                "not_started"
            } else if complete {
                "stopped"
            } else {
                "unknown"
            }
            .into(),
            known_consumed_tokens: spent,
            retained_tokens: held,
            unresolved_tokens: held - spent,
            usage_complete: complete,
            budget_revision: budget.as_ref().map(|b| b.budget_revision),
            budget_digest: budget.as_ref().map(|b| b.budget_digest.clone()),
            effective_ceiling_tokens: budget.as_ref().map(|b| b.effective_ceiling_tokens),
            liability_digest: "a".repeat(64),
        },
    }
}
pub(super) async fn record(
    f: &Fixture,
    d: &AgentSettlementDelivery,
    r: AgentSettlementResult,
) -> crate::Result<ManagedAgentSettlementReceipt> {
    f.domain
        .record_agent_settlement(&f.owner.p, &d.command.command_id, r)
        .await
}
async fn get(service: &Service, path: &str, token: &str, status: StatusCode) -> Value {
    let mut r = TestClient::get(format!(
        "https://example.test/hagency/api/v1/management/{path}"
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(token)
    .send(service)
    .await;
    assert_eq!(r.status_code, Some(status));
    r.take_json().await.unwrap()
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn settlement_http_roles_finite_tickets_exact_recovery_and_conservative_balance() {
    let (mut f, r, a, fence) = fenced(true).await;
    for p in [&f.owner.p, &f.admin.p, &f.coordinator.p] {
        assert!(f.domain.managed_agent_settlement(p, &r.id).await.is_err());
    }
    let view = f
        .domain
        .managed_agent_settlement(&f.outsider.p, &r.id)
        .await
        .unwrap();
    assert!(!view.can_request && !view.settlement_current);
    assert_eq!(view.retained_tokens_floor, Some(100));
    let id = view.pending_command_id.unwrap();
    let w = {
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        sql_query("SELECT digest AS payload FROM hagency_agent_v1.management_agent_settlement_commands WHERE id=$1").bind::<Text,_>(&id).get_result::<Payload>(&mut db).await.unwrap().payload
    };
    let historical = f
        .domain
        .recover_agent_settlement(&f.owner.p, &id, ExecutorRecoverInput { command_digest: w })
        .await
        .unwrap();
    assert!(!historical.execution_allowed);
    assert!(
        record(
            &f,
            &historical,
            result(&f, &historical, 120, 40, 40, true).await
        )
        .await
        .is_err()
    );
    let first = poll_settlement(&f).await;
    let second = poll_settlement(&f).await;
    assert_ne!(first.token, second.token);
    let body = result(&f, &first, 120, 40, 40, true).await;
    let first_service = service(&f, true);
    for actor in [&f.outsider, &f.admin, &f.coordinator] {
        post(
            &first_service,
            &format!("agent-settlement-executor/{id}/result"),
            &actor.token,
            serde_json::to_value(&body).unwrap(),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let receipt = record(&f, &first, body.clone()).await.unwrap();
    assert_eq!(receipt.retained_tokens, 40);
    f.policy = f
        .policy
        .with_agent_executor(false)
        .with_agent_admission(false)
        .with_agent_budget_deltas(false);
    let service = service(&f, true);
    let json = get(
        &service,
        &format!("agent-requests/{}/settlement", r.id),
        &a.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(json["settlement"]["settlementCurrent"], true);
    assert_eq!(json["settlement"]["retainedTokensFloor"], 40);
    let cleanup = f.domain.managed_agent_removal(&a.p, &r.id).await.unwrap();
    assert_eq!(cleanup.usage.retained_tokens_floor, Some(40));
    assert_eq!(cleanup.usage.fence_usage_complete, Some(false));
    assert!(!cleanup.usage.requires_reconciliation);
    assert_eq!(
        f.domain
            .agent_budget(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .conservative_commitment_tokens,
        40
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("UPDATE hagency_agent_v1.management_agent_settlement_tickets SET issued_at_ms=1,expires_at_ms=2").await.unwrap();
    drop(db);
    assert_eq!(
        allocation_digest(&record(&f, &first, body.clone()).await.unwrap()).unwrap(),
        allocation_digest(&receipt).unwrap()
    );
    let mut wrong = body;
    wrong.receipt.liability_digest = "b".repeat(64);
    assert!(record(&f, &first, wrong).await.is_err());
    let old = f
        .domain
        .recover_managed_agent_executor(
            &f.owner.p,
            &f.policy.clone().with_agent_executor(true),
            id_of(&fence),
            ExecutorRecoverInput {
                command_digest: fence.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        allocation_digest(old.recorded.as_ref().unwrap()).unwrap(),
        allocation_digest(&fence_result(&fence, true).receipt).unwrap()
    );
    assert!(
        f.domain
            .poll_agent_settlement(&f.owner.p, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}
#[derive(QueryableByName)]
struct Payload {
    #[diesel(sql_type=Text)]
    payload: String,
}
fn id_of(d: &AgentExecutorDelivery) -> &str {
    super::id(d)
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn settlement_unknown_holds_over_ceiling_new_proof_and_human_retry_are_exact() {
    let (f, r, a, _) = fenced(true).await;
    let first = poll_settlement(&f).await;
    let original = record(&f, &first, result(&f, &first, 120, 40, 180, false).await)
        .await
        .unwrap();
    let v = f
        .domain
        .managed_agent_settlement(&a.p, &r.id)
        .await
        .unwrap();
    assert_eq!(v.retained_tokens_floor, Some(180));
    assert_eq!(
        f.domain
            .agent_budget(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .conservative_commitment_tokens,
        180
    );
    assert!(v.can_request && !v.settlement_current);
    let input = RequestAgentSettlement {
        command_id: "settlement_recheck".into(),
        expected_request_revision: r.revision,
        expected_fence_receipt_digest: v.fence_receipt_digest.unwrap(),
    };
    let mut bad = input.clone();
    bad.expected_request_revision += 1;
    assert!(
        f.domain
            .request_agent_settlement(&f.outsider.p, &r.id, bad)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .request_agent_settlement(&f.owner.p, &r.id, input.clone())
            .await
            .is_err()
    );
    let req = f
        .domain
        .request_agent_settlement(&f.outsider.p, &r.id, input.clone())
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .request_agent_settlement(&f.outsider.p, &r.id, input)
            .await
            .unwrap()
            .command_id,
        req.command_id
    );
    let next = poll_settlement(&f).await;
    let mut collision = result(&f, &next, 120, 60, 60, true).await;
    assert!(record(&f, &next, collision.clone()).await.is_err());
    collision.receipt.ledger_revision = 130;
    record(&f, &next, collision).await.unwrap();
    let v = f
        .domain
        .managed_agent_settlement(&a.p, &r.id)
        .await
        .unwrap();
    assert!(v.settlement_current);
    assert_eq!(v.retained_tokens_floor, Some(60));
    assert_eq!(v.latest_receipt.unwrap().ledger_revision, 130);
    let recovered = f
        .domain
        .recover_agent_settlement(
            &f.owner.p,
            &first.command.command_id,
            ExecutorRecoverInput {
                command_digest: first.command_digest,
            },
        )
        .await
        .unwrap();
    assert!(!recovered.execution_allowed);
    assert_eq!(
        allocation_digest(&recovered.recorded.unwrap()).unwrap(),
        allocation_digest(&original).unwrap()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(
        db.batch_execute("DELETE FROM hagency_agent_v1.management_agent_settlement_results")
            .await
            .is_err()
    );
    assert!(
        db.batch_execute(
            "UPDATE hagency_agent_v1.management_agent_settlement_commands SET digest=repeat('f',64)"
        )
        .await
        .is_err()
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn settlement_absent_fence_has_no_budget_or_new_agent_and_never_recovers_as_executable() {
    let (f, r, a, _) = fenced(false).await;
    let d = poll_settlement(&f).await;
    let rr = record(&f, &d, result(&f, &d, 120, 0, 0, true).await)
        .await
        .unwrap();
    assert!(rr.agent_reservation_id.is_none());
    let v = f
        .domain
        .managed_agent_settlement(&a.p, &r.id)
        .await
        .unwrap();
    assert!(v.settlement_current);
    assert_eq!(v.retained_tokens_floor, Some(0));
    assert!(
        f.domain
            .managed_agent_assignment(&f.owner.p, &f.policy, &r.id)
            .await
            .unwrap()
            .is_none()
    );
    let h = f
        .domain
        .recover_agent_settlement(
            &f.owner.p,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest,
            },
        )
        .await
        .unwrap();
    assert!(!h.execution_allowed);
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn settlement_schema20_upgrade_rolls_back_and_does_not_enroll_historical_fences() {
    let (f, r, a, d) = fenced(true).await;
    let original = fence_result(&d, true);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute(
        "CREATE TABLE hagency_agent_v1.management_agent_settlement_results(dummy text)",
    )
    .await
    .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    let check=sql_query("SELECT CASE WHEN version=20 AND to_regclass('hagency_agent_v1.management_agent_settlement_commands') IS NULL THEN 'yes' ELSE 'no' END AS payload FROM hagency_agent_v1.management_deployment").get_result::<Payload>(&mut db).await.unwrap();
    assert_eq!(check.payload, "yes");
    db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_settlement_results")
        .await
        .unwrap();
    for _ in 0..2 {
        let domain = DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap();
        assert!(
            domain
                .poll_agent_settlement(&f.owner.p, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
        // Replaying the old receipt does not enroll the historical agent either.
        domain
            .record_managed_agent_executor_result(
                &f.owner.p,
                &f.policy,
                id_of(&d),
                original.clone(),
            )
            .await
            .unwrap();
        assert!(
            domain
                .poll_agent_settlement(&f.owner.p, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
        let v = domain.managed_agent_settlement(&a.p, &r.id).await.unwrap();
        assert!(v.can_request);
        assert!(v.latest_receipt.is_none());
        assert_eq!(v.retained_tokens_floor, Some(100));
        let old = domain
            .recover_managed_agent_executor(
                &f.owner.p,
                &f.policy,
                id_of(&d),
                ExecutorRecoverInput {
                    command_digest: d.command_digest.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            allocation_digest(&old.recorded.unwrap()).unwrap(),
            allocation_digest(&original.receipt).unwrap()
        );
    }
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn settlement_enqueue_failure_rolls_back_original_fence_receipt_and_can_retry() {
    let (f, r, a) = approved().await;
    let reserved = poll(&f).await;
    submit(&f, &reserved, reservation(&reserved)).await.unwrap();
    f.domain
        .revoke_managed_agent_request(
            &a.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "enqueue_fault_revoke".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
    let d = poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.test_settlement_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test settlement failure'; END; $$;CREATE TRIGGER test_settlement_fault BEFORE INSERT ON hagency_agent_v1.management_agent_settlement_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.test_settlement_fault()").await.unwrap();
    assert!(submit(&f, &d, fence_result(&d, true)).await.is_err());
    let old = f
        .domain
        .recover_managed_agent_executor(
            &f.owner.p,
            &f.policy,
            id_of(&d),
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(old.recorded.is_none());
    assert!(
        f.domain
            .poll_agent_settlement(&f.owner.p, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    db.batch_execute("DROP TRIGGER test_settlement_fault ON hagency_agent_v1.management_agent_settlement_commands;DROP FUNCTION hagency_agent_v1.test_settlement_fault()").await.unwrap();
    // Recovery rotates the single original fence ticket. Upload its original
    // proof with that fresh historical ticket, without requesting new execution.
    let d = old;
    submit(&f, &d, fence_result(&d, true)).await.unwrap();
    let first = poll_settlement(&f).await;
    submit(&f, &d, fence_result(&d, true)).await.unwrap();
    assert_eq!(
        poll_settlement(&f).await.command.command_id,
        first.command.command_id
    );
}
