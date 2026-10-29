#[path = "management_allocation_ui_tests.rs"]
mod ui;
#[path = "management_parent_tests.rs"]
mod parents;
#[path = "management_setup_tests.rs"]
mod setup;
#[path = "management_owner_coordinator_tests.rs"]
mod owner_coordination;
#[path = "management_admission_grant_tests.rs"]
mod admission_grants;
#[path = "management_pool_agents_tests.rs"]
mod pool_agents;
use super::*;
use crate::management_allocation_types::*;
use diesel::QueryableByName;
async fn enabled_fixture() -> Fixture {
    let mut f = fixture().await;
    f.policy = f.policy.with_project_reservations(true);
    f
}

async fn admitted(f: &Fixture) -> Admission {
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(f, "native_capacity"))
        .await
        .unwrap();
    f.domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("admit_native", 1),
        )
        .await
        .unwrap()
}

async fn approved_plan(f: &Fixture, a: &Admission, manager: &Principal) -> ProjectPlan {
    let p = f
        .domain
        .request_management_project(
            manager,
            &f.policy,
            ProjectRequestInput {
                command_id: "request_native_project".into(),
                contribution_id: a.contribution.id.clone(),
                name: "Native project".into(),
                purpose: "Capacity request, no agent yet".into(),
                requested_tokens: 100_000,
            },
        )
        .await
        .unwrap();
    f.domain
        .decide_management_project(
            &f.admin.p,
            &f.policy,
            &p.request.id,
            approve("admit_native_project", 1),
        )
        .await
        .unwrap()
}

#[derive(QueryableByName)]
struct Period {
    #[diesel(sql_type=Text)]
    key: String,
    #[diesel(sql_type=BigInt)]
    start_ms: i64,
    #[diesel(sql_type=BigInt)]
    end_ms: i64,
}

async fn grant_input(f: &Fixture, a: &Admission, manager: &Principal) -> PublishGrantInput {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let period=sql_query("SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM') AS key,(extract(epoch FROM (date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))*1000)::bigint AS start_ms,(extract(epoch FROM ((date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC'))*1000)::bigint AS end_ms").get_result::<Period>(&mut db).await.unwrap();
    PublishGrantInput {
        command_id: "publish_native_grant".into(),
        expected_contribution_revision: a.contribution.revision,
        receipt: NativeGrantReceipt {
            version: 1,
            capability: ALLOCATION_CAPABILITY.into(),
            admission_request_id: None,
            enrollment_id: format!("men_{}", secret_token()),
            grant_id: format!("mgr_{}", secret_token()),
            grant_revision: 1,
            contribution_id: a.contribution.id.clone(),
            resource_id: a.contribution.resource_id.clone(),
            resource_revision: a.contribution.resource_revision.clone(),
            executor_device_id: f.owner.p.device_id.clone().unwrap(),
            executor_generation: f.owner.p.device_generation.unwrap(),
            owner_mxid: f.owner.p.mxid.clone(),
            coordinator_mxid: f.coordinator.p.mxid.clone(),
            delegation_revision: 1,
            delegation_expires_at_ms: period.end_ms,
            allow_self_approval: false,
            eligible_managers: vec![manager.mxid.clone()],
            allocated_tokens: 1_000_000,
            period_key: period.key,
            period_start_ms: period.start_ms,
            period_end_ms: period.end_ms,
            ledger_revision: 10,
            succession: None,
        },
    }
}

async fn allocation(f: &Fixture, p: &ProjectPlan) -> ProjectAllocation {
    f.domain
        .management_project_allocation(&f.coordinator.p, &f.policy, &p.id)
        .await
        .unwrap()
        .unwrap()
}

async fn prepared() -> (
    Fixture,
    Admission,
    ProjectPlan,
    ResourceGrantView,
    ProjectAllocation,
) {
    let f = enabled_fixture().await;
    let a = admitted(&f).await;
    let p = approved_plan(&f, &a, &f.outsider.p).await;
    let input = grant_input(&f, &a, &f.outsider.p).await;
    let g = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input)
        .await
        .unwrap();
    let a2 = allocation(&f, &p).await;
    (f, a, p, g, a2)
}

async fn accepted(f: &Fixture, a: &ProjectAllocation) -> ProjectAllocation {
    f.domain
        .decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &a.request_id,
            approve("accept_native_capacity", 1),
        )
        .await
        .unwrap()
}

async fn delivered(f: &Fixture) -> ExecutorDelivery {
    f.domain
        .poll_management_executor(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap()
        .unwrap()
}

fn result(d: &ExecutorDelivery, state: &str, revision: i64) -> AllocationResultInput {
    AllocationResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: NativeAllocationReceipt {
            version: 1,
            command_id: d.command.command_id.clone(),
            allocation_id: d.command.allocation_id.clone(),
            state: state.into(),
            ledger_revision: revision,
            reserved_tokens: if state == "refused" {
                0
            } else {
                d.command.requested_tokens
            },
            retained_tokens: if state == "reserved" {
                d.command.requested_tokens
            } else {
                0
            },
            period_key: d.command.grant_receipt.period_key.clone(),
            reason: (state == "refused").then(|| "unsettled_usage".into()),
        },
    }
}

async fn record(
    f: &Fixture,
    d: &ExecutorDelivery,
    state: &str,
    revision: i64,
) -> AllocationAcknowledgement {
    f.domain
        .record_management_executor_result(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            result(d, state, revision),
        )
        .await
        .unwrap()
}

async fn release(f: &Fixture, a: &ProjectAllocation, command: &str) -> ProjectAllocation {
    f.domain
        .release_management_allocation(
            &f.outsider.p,
            &f.policy,
            &a.id,
            RevokeInput {
                command_id: command.into(),
                expected_revision: a.revision,
            },
        )
        .await
        .unwrap()
}

async fn fresh_installation(f: &Fixture, installation: &str) -> Principal {
    let t = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: installation.into(),
                name: installation.into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    f.auth.authenticate(&t, now_ms(), true).await.unwrap()
}

#[test]
fn native_canonical_vectors_are_exact_closed_and_safe_integers() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-allocation-v1.json"
    ))
    .unwrap();
    let g: NativeGrantReceipt = serde_json::from_value(fixture["grant"].clone()).unwrap();
    let c: AllocationCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
    let r: NativeAllocationReceipt = serde_json::from_value(fixture["receipt"].clone()).unwrap();
    assert_eq!(allocation_digest(&g).unwrap(), fixture["grantDigest"]);
    assert_eq!(allocation_digest(&c).unwrap(), fixture["commandDigest"]);
    assert_eq!(allocation_digest(&r).unwrap(), fixture["receiptDigest"]);
    assert_eq!(
        allocation_digest(&fixture["projectDefinition"]).unwrap(),
        fixture["projectDefinitionDigest"]
    );
    let mut injected = fixture["command"].clone();
    injected["ownerApproved"] = json!(true);
    assert!(serde_json::from_value::<AllocationCommand>(injected).is_err());
    assert!(allocation_digest(&json!(MAX_EXACT + 1)).is_err());
    assert!(allocation_digest(&json!({"__proto__":{}})).is_err());
    let mut oversized = g.clone();
    oversized.eligible_managers = (0..64)
        .map(|i| format!("@m{i:02}{}:example.test", "x".repeat(237)))
        .collect();
    assert_eq!(
        oversized.validate("example.test").unwrap_err().to_string(),
        "resource_grant_too_large"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn owner_native_grant_is_exactly_bound_and_protocol_is_not_advertised_early() {
    let mut f = fixture().await;
    let a = admitted(&f).await;
    let input = grant_input(&f, &a, &f.outsider.p).await;
    assert!(
        !f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
            .await
            .unwrap_err()
            .to_string(),
        "project_reservations_unavailable"
    );
    f.policy = f.policy.with_project_reservations(true);
    assert!(
        !f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
    for p in [&f.admin.p, &f.outsider.p] {
        assert!(
            f.domain
                .publish_management_grant(p, &f.policy, &a.contribution.id, input.clone())
                .await
                .is_err()
        );
    }
    let other = fresh_installation(&f, "other_device").await;
    assert!(
        f.domain
            .publish_management_grant(&other, &f.policy, &a.contribution.id, input.clone())
            .await
            .is_err()
    );
    for field in [
        "ownerMxid",
        "coordinatorMxid",
        "executorDeviceId",
        "resourceId",
        "contributionId",
    ] {
        let mut value = serde_json::to_value(&input).unwrap();
        value["receipt"][field] = json!(if field.ends_with("Mxid") {
            f.admin.p.mxid.as_str()
        } else {
            "wrong_binding"
        });
        assert!(
            f.domain
                .publish_management_grant(
                    &f.owner.p,
                    &f.policy,
                    &a.contribution.id,
                    serde_json::from_value(value).unwrap()
                )
                .await
                .is_err(),
            "{field}"
        );
    }
    let mut invalid = input.clone();
    invalid
        .receipt
        .eligible_managers
        .push(invalid.receipt.eligible_managers[0].clone());
    assert!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, invalid)
            .await
            .is_err()
    );
    let mut invalid = input.clone();
    invalid.receipt.allocated_tokens = MAX_EXACT + 1;
    assert!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, invalid)
            .await
            .is_err()
    );
    let mut invalid = input.clone();
    invalid.receipt.period_end_ms -= 1;
    assert!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, invalid)
            .await
            .is_err()
    );
    let g = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(g.receipt_digest, allocation_digest(&input.receipt).unwrap());
    assert!(
        f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
    let replay = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(replay.id, g.id);
    let p = approved_plan(&f, &a, &f.outsider.p).await;
    assert_eq!(allocation(&f, &p).await.state, "pending_coordinator");
    let catalog = f
        .domain
        .management_catalog(&f.outsider.p, Default::default())
        .await
        .unwrap();
    let serialized = serde_json::to_string(&catalog).unwrap();
    assert!(!serialized.contains(&g.id));
    assert!(!serialized.contains("enrollmentId"));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn coordinator_decision_is_separate_from_admission_and_exact_executor_receipt() {
    let (f, _, p, _, a) = prepared().await;
    assert!(a.can_decide);
    assert_eq!(a.state, "pending_coordinator");
    for actor in [&f.admin.p, &f.outsider.p] {
        assert!(
            !f.domain
                .management_project_allocation(actor, &f.policy, &p.id)
                .await
                .unwrap()
                .unwrap()
                .can_decide
        );
        assert_eq!(
            f.domain
                .decide_management_allocation(
                    actor,
                    &f.policy,
                    &a.request_id,
                    approve("forbidden", 1)
                )
                .await
                .unwrap_err()
                .to_string(),
            "allocation_coordinator_required"
        );
    }
    let old = f
        .domain
        .management_inbox(
            &f.coordinator.p,
            &f.policy,
            ManagementQuery {
                needs_my_action: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(old.items.is_empty());
    let inbox = f
        .domain
        .management_inbox(
            &f.coordinator.p,
            &f.policy,
            ManagementQuery {
                needs_my_action: true,
                include_allocations: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(inbox.items.len(), 1);
    assert!(inbox.items[0].can_decide);
    let admin_history = f
        .domain
        .management_inbox(
            &f.admin.p,
            &f.policy,
            ManagementQuery {
                include_allocations: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        admin_history
            .items
            .iter()
            .any(|r| r.allocation_id.as_deref() == Some(a.id.as_str()) && !r.can_decide)
    );
    let approved = accepted(&f, &a).await;
    assert_eq!(approved.state, "approved_waiting_executor");
    assert!(approved.result.is_none());
    assert!(approved.balance.is_none());
    assert!(
        f.domain
            .poll_management_executor(&f.admin.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    let d = delivered(&f).await;
    assert!(d.execution_allowed);
    assert_eq!(d.command.manager_mxid, f.outsider.p.mxid);
    assert_eq!(d.command.request_revision, 2);
    assert_eq!(d.command_digest, allocation_digest(&d.command).unwrap());
    let wrong = fresh_installation(&f, "other_device").await;
    assert!(
        f.domain
            .recover_management_executor(
                &wrong,
                &f.policy,
                &d.command.command_id,
                ExecutorRecoverInput {
                    command_digest: d.command_digest.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .record_management_executor_result(
                &wrong,
                &f.policy,
                &d.command.command_id,
                result(&d, "reserved", 11)
            )
            .await
            .is_err()
    );
    let mut bad = result(&d, "reserved", 11);
    bad.receipt.reserved_tokens -= 1;
    assert!(
        f.domain
            .record_management_executor_result(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .is_err()
    );
    let mut bad = result(&d, "reserved", 11);
    bad.token = "bad".into();
    assert!(
        f.domain
            .record_management_executor_result(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .is_err()
    );
    let ack = record(&f, &d, "reserved", 11).await;
    assert_eq!(ack.allocation.state, "reserved");
    assert_eq!(
        ack.allocation.balance.as_ref().unwrap().retained_tokens,
        100_000
    );
    assert_eq!(
        ack.receipt_digest,
        allocation_digest(&result(&d, "reserved", 11).receipt).unwrap()
    );
    let replay = record(&f, &d, "reserved", 11).await;
    assert_eq!(replay.allocation.revision, ack.allocation.revision);
    assert!(
        f.domain
            .record_management_executor_result(
                &f.owner.p,
                &f.policy,
                &d.command.command_id,
                result(&d, "reserved", 12)
            )
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type=BigInt)]
        count: i64,
    }
    let count=sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_executor_commands WHERE allocation_id=$1").bind::<Text,_>(&a.id).get_result::<Count>(&mut db).await.unwrap().count;
    assert_eq!(count, 1);
    assert!(outbox(&f, &a.request_id, "pending").await >= 3);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn self_approval_and_expiry_are_not_actionable_or_hidden_as_pending_work() {
    let f = enabled_fixture().await;
    let a = admitted(&f).await;
    let p = approved_plan(&f, &a, &f.coordinator.p).await;
    let g = grant_input(&f, &a, &f.coordinator.p).await;
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g)
        .await
        .unwrap();
    let a = allocation(&f, &p).await;
    assert!(!a.can_decide);
    assert_eq!(
        a.blocking_reason.as_deref(),
        Some("self_approval_forbidden")
    );
    assert!(
        f.domain
            .decide_management_allocation(
                &f.coordinator.p,
                &f.policy,
                &a.request_id,
                approve("self", 1)
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_inbox(
                &f.coordinator.p,
                &f.policy,
                ManagementQuery {
                    needs_my_action: true,
                    include_allocations: true,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_allocations SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1")
        .bind::<Text, _>(&a.id)
        .execute(&mut db)
        .await
        .unwrap();
    let a = allocation(&f, &p).await;
    assert_eq!(a.state, "expired");
    assert!(!a.can_decide);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn lost_result_recovers_without_reexecution_and_release_preserves_known_balance() {
    let (f, _, p, _, a) = prepared().await;
    accepted(&f, &a).await;
    let d = delivered(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "UPDATE hagency_agent_v1.management_executor_commands SET lease_until_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1",
    )
    .bind::<Text, _>(&d.command.command_id)
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(allocation(&f, &p).await.state, "outcome_unknown");
    assert!(
        f.domain
            .record_management_executor_result(
                &f.owner.p,
                &f.policy,
                &d.command.command_id,
                result(&d, "reserved", 12)
            )
            .await
            .is_err()
    );
    let recovered = f
        .domain
        .recover_management_executor(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recovered.execution_allowed);
    let ack = record(&f, &recovered, "reserved", 12).await;
    let released = release(&f, &ack.allocation, "release_capacity").await;
    assert_eq!(released.state, "releasing");
    assert_eq!(released.balance.unwrap().retained_tokens, 100_000);
    let release_delivery = delivered(&f).await;
    assert_eq!(release_delivery.command.operation, "project.release");
    assert_eq!(release_delivery.command.request_revision, 4);
    assert!(
        f.domain
            .record_management_executor_result(
                &f.owner.p,
                &f.policy,
                &release_delivery.command.command_id,
                result(&release_delivery, "released", 11)
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .record_management_executor_result(
                &f.owner.p,
                &f.policy,
                &release_delivery.command.command_id,
                result(&release_delivery, "released", 12)
            )
            .await
            .is_err()
    );
    let blocked = record(&f, &release_delivery, "refused", 13).await;
    assert_eq!(blocked.allocation.state, "release_blocked");
    assert_eq!(
        blocked.allocation.balance.as_ref().unwrap().retained_tokens,
        100_000
    );
    let retry = release(&f, &blocked.allocation, "retry_release_capacity").await;
    assert_eq!(retry.state, "releasing");
    let d2 = delivered(&f).await;
    let mut report = result(&d2, "released", 14);
    report.receipt.retained_tokens = 123;
    let final_ack = f
        .domain
        .record_management_executor_result(&f.owner.p, &f.policy, &d2.command.command_id, report)
        .await
        .unwrap();
    assert_eq!(final_ack.allocation.state, "released");
    assert_eq!(final_ack.allocation.balance.unwrap().retained_tokens, 123);
    let historical = record(&f, &recovered, "reserved", 12).await;
    assert_eq!(historical.allocation.state, "released");
    assert_eq!(historical.allocation.balance.unwrap().retained_tokens, 123);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cancellation_and_device_generation_fence_new_work_but_keep_exact_receipt_recovery() {
    let (f, c, p, _, a) = prepared().await;
    accepted(&f, &a).await;
    let d = delivered(&f).await;
    f.domain
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &c.contribution.id,
            RevokeInput {
                command_id: "revoke_capacity".into(),
                expected_revision: c.contribution.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(allocation(&f, &p).await.state, "releasing");
    let fresh = fresh_installation(&f, "macbook").await;
    let recovery = f
        .domain
        .recover_management_executor(
            &fresh,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recovery.execution_allowed);
    assert!(recovery.recorded.is_none());
    let late = f
        .domain
        .record_management_executor_result(
            &fresh,
            &f.policy,
            &recovery.command.command_id,
            result(&recovery, "reserved", 11),
        )
        .await
        .unwrap();
    assert_eq!(late.allocation.state, "releasing");
    assert_eq!(late.allocation.balance.unwrap().retained_tokens, 100_000);
    assert!(
        f.domain
            .management_allocation_supported(&fresh, &f.policy)
            .await
            .unwrap()
    );
    let release_delivery = f
        .domain
        .poll_management_executor(&fresh, &f.policy, Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release_delivery.command.operation, "project.release");
    assert!(release_delivery.execution_allowed);
    let final_ack = f
        .domain
        .record_management_executor_result(
            &fresh,
            &f.policy,
            &release_delivery.command.command_id,
            result(&release_delivery, "released", 12),
        )
        .await
        .unwrap();
    assert_eq!(final_ack.allocation.state, "released");
    assert!(
        !f.domain
            .management_project(&f.admin.p, &f.policy, &p.id)
            .await
            .unwrap()
            .request
            .can_decide
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn concurrent_resource_withdrawal_and_capacity_decision_never_leave_executable_hold() {
    let (f, c, p, _, a) = prepared().await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (withdraw, decision) = tokio::join!(
        f.domain.revoke_contribution(
            &f.owner.p,
            &f.policy,
            &c.contribution.id,
            RevokeInput {
                command_id: "withdraw_race".into(),
                expected_revision: c.contribution.revision
            }
        ),
        other.decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &a.request_id,
            approve("capacity_race", 1)
        )
    );
    withdraw.unwrap();
    if let Ok(result) = decision {
        assert_eq!(result.state, "approved_waiting_executor");
    }
    let a = allocation(&f, &p).await;
    assert!(matches!(a.state.as_str(), "releasing" | "released"));
    assert!(!a.can_decide);
    if let Some(d) = f
        .domain
        .poll_management_executor(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap()
    {
        assert!(d.command.operation == "project.release" || !d.execution_allowed);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn actual_http_allocation_contract_is_closed_and_only_owner_decides()
 {
    let (f, _, p, _, a) = prepared().await;
    let s = service(&f, true);
    let mut r = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.owner.token)
        .send(&s)
        .await;
    let session: Value = r.take_json().await.unwrap();
    assert!(
        session["features"]
            .as_array()
            .unwrap()
            .contains(&json!(ALLOCATION_FEATURE))
    );
    post(
        &s,
        &format!("allocation-requests/{}/decide", a.request_id),
        &f.outsider.token,
        json!({"commandId":"http_forbidden","expectedRevision":1,"decision":"approve"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    post(&s,&format!("allocation-requests/{}/decide",a.request_id),&f.coordinator.token,json!({"commandId":"http_injected","expectedRevision":1,"decision":"approve","managerMxid":f.admin.p.mxid}),StatusCode::BAD_REQUEST).await;
    let approved = post(
        &s,
        &format!("allocation-requests/{}/decide", a.request_id),
        &f.coordinator.token,
        json!({"commandId":"http_allocation","expectedRevision":1,"decision":"approve"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved["allocation"]["state"], "approved_waiting_executor");
    let recovery = post(
        &s,
        "allocation-commands/http_allocation/recover",
        &f.coordinator.token,
        json!({"operation":"allocation.decide"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovery["allocation"]["projectId"], p.id);
    post(
        &s,
        "allocation-commands/http_allocation/recover",
        &f.coordinator.token,
        json!({"operation":"project.decide"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let d = post(
        &s,
        "executor-commands/poll",
        &f.owner.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(d["delivery"]["executionAllowed"], true);
    post(
        &s,
        "executor-commands/poll?owner=admin",
        &f.owner.token,
        json!({}),
        StatusCode::BAD_REQUEST,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn grant_decision_and_executor_result_outbox_failures_rollback_every_related_record() {
    let f = enabled_fixture().await;
    let c = admitted(&f).await;
    let p = approved_plan(&f, &c, &f.outsider.p).await;
    let input = grant_input(&f, &c, &f.outsider.p).await;
    let function = format!("allocation_fail_{}", &secret_token()[..12]);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid IN ('{}','{}') THEN RAISE EXCEPTION 'injected_allocation_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.coordinator.p.mxid,f.outsider.p.mxid)).await.unwrap();
    let publication = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, input.clone())
        .await;
    db.batch_execute(&format!(
        "DROP TRIGGER {function} ON hagency_agent_v1.management_outbox;"
    ))
    .await
    .unwrap();
    assert!(publication.is_err());
    assert!(
        f.domain
            .management_project_allocation(&f.owner.p, &f.policy, &p.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !f.domain
            .recover_management_allocation_command(
                &f.owner.p,
                &f.policy,
                &input.command_id,
                RecoverCommandInput {
                    operation: "grant.publish".into()
                }
            )
            .await
            .unwrap()["recorded"]
            .as_bool()
            .unwrap()
    );
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, input)
        .await
        .unwrap();
    let a = allocation(&f, &p).await;
    db.batch_execute(&format!("CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    let decision = f
        .domain
        .decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &a.request_id,
            approve("atomic_decision", 1),
        )
        .await;
    db.batch_execute(&format!(
        "DROP TRIGGER {function} ON hagency_agent_v1.management_outbox;"
    ))
    .await
    .unwrap();
    assert!(decision.is_err());
    assert_eq!(allocation(&f, &p).await.state, "pending_coordinator");
    assert!(
        f.domain
            .poll_management_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    f.domain
        .decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &a.request_id,
            approve("atomic_decision", 1),
        )
        .await
        .unwrap();
    let d = delivered(&f).await;
    db.batch_execute(&format!("CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    let report = f
        .domain
        .record_management_executor_result(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            result(&d, "reserved", 11),
        )
        .await;
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(report.is_err());
    assert_eq!(allocation(&f, &p).await.state, "approved_waiting_executor");
    let recovered = f
        .domain
        .recover_management_executor(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(recovered.recorded.is_none());
    assert_eq!(
        record(&f, &recovered, "reserved", 11)
            .await
            .allocation
            .state,
        "reserved"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn late_reserve_receipt_preserves_hold_during_release_but_never_overwrites_settlement() {
    for late_after_release in [false, true] {
        let (f, _, p, _, a) = prepared().await;
        let approved = accepted(&f, &a).await;
        let reserve = delivered(&f).await;
        release(&f, &approved, "cancel_before_result").await;
        let release_delivery = delivered(&f).await;
        if late_after_release {
            let done = record(&f, &release_delivery, "released", 12).await;
            assert_eq!(done.allocation.state, "released");
            assert!(
                f.domain
                    .record_management_executor_result(
                        &f.owner.p,
                        &f.policy,
                        &reserve.command.command_id,
                        result(&reserve, "reserved", 13)
                    )
                    .await
                    .is_err()
            );
        }
        let late = record(&f, &reserve, "reserved", 11).await;
        assert_eq!(
            late.allocation.state,
            if late_after_release {
                "released"
            } else {
                "releasing"
            }
        );
        assert_eq!(
            late.allocation.balance.unwrap().retained_tokens,
            if late_after_release { 0 } else { 100_000 }
        );
        if !late_after_release {
            let blocked = record(&f, &release_delivery, "refused", 12).await;
            assert_eq!(blocked.allocation.state, "release_blocked");
            assert_eq!(blocked.allocation.balance.unwrap().retained_tokens, 100_000);
        }
        assert!(!allocation(&f, &p).await.can_decide);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v3_allocation_migration_is_atomic_and_preserves_existing_project_intent() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_allocation_upgrade_{}",
        &crate::hash(&secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
        let auth=Store::open(url.as_str(),"example.test","https://example.test/auth/").await.unwrap();let owner=actor(&auth,"legacy_owner").await;let admin=actor(&auth,"legacy_admin").await;
        let policy=ManagementPolicy::new(admin.p.mxid.clone(),"example.test").unwrap();let now=now_ms();
        let mut db=AsyncPgConnection::establish(url.as_str()).await.unwrap();
        for schema in [include_str!("management_schema.sql"),include_str!("management_verification_schema.sql"),include_str!("management_projects_schema.sql")] {db.batch_execute(schema).await.unwrap();}
        db.transaction::<_,diesel::result::Error,_>(async |db:&mut AsyncPgConnection| {
            sql_query("INSERT INTO hagency_agent_v1.management_contributions(id,owner_user_id,executor_device_id,executor_generation,resource_id,resource_revision,label,runtime,capabilities,coordinator_mxid,state,revision,request_id,created_at_ms,updated_at_ms) VALUES('legacy_contribution',$1,$2,$3,'legacy_resource',$4,'Legacy MacBook','codex','[\"coding\"]',$5,'admitted',2,'legacy_request',$6,$6)").bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(owner.p.device_id.as_ref().unwrap()).bind::<BigInt,_>(owner.p.device_generation.unwrap()).bind::<Text,_>("a".repeat(64)).bind::<Text,_>(&owner.p.mxid).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) VALUES('legacy_request','resource_admission','legacy_contribution',$1,$2,'approved',2,$3,$4,$3)").bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&admin.p.mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(now+600000).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_projects(id,manager_user_id,contribution_id,name,purpose,requested_tokens,state,revision,request_id,created_at_ms,updated_at_ms) VALUES('legacy_project',$1,'legacy_contribution','Legacy project','Existing approved intent',1234,'approved',2,'legacy_project_request',$2,$2)").bind::<Text,_>(&owner.p.user_id).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,project_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) VALUES('legacy_project_request','project_capacity','legacy_contribution','legacy_project',$1,$2,'approved',2,$3,$4,$3)").bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&admin.p.mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(now+600000).execute(db).await?;Ok(())
        }).await.unwrap();
        // Force migration failure after its first CREATE; that table and version
        // change must roll back together without losing the existing intent.
        db.batch_execute("CREATE TABLE hagency_agent_v1.management_allocations (id text)").await.unwrap();
        assert!(DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.is_err());
        #[derive(QueryableByName)]struct State{#[diesel(sql_type=diesel::sql_types::Bool)] matched:bool}
        assert!(sql_query("SELECT version=3 AND to_regclass('hagency_agent_v1.management_resource_grants') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
        db.batch_execute("DROP TABLE hagency_agent_v1.management_allocations").await.unwrap();
        let domain=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();
        let p=domain.management_project(&owner.p,&policy,"legacy_project").await.unwrap();assert_eq!(p.state,"approved");assert_eq!(p.requested_tokens,1234);assert_eq!(p.request.revision,2);assert!(p.request.allocation_id.is_none());
        assert!(sql_query("SELECT version=29 AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
        let again=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();assert_eq!(again.management_project(&owner.p,&policy,"legacy_project").await.unwrap().revision,2);
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn changed_executor_authority_blocks_pending_capacity_without_hiding_recovery_protocol() {
    let (f, _, p, _, a) = prepared().await;
    let fresh = fresh_installation(&f, "macbook").await;
    let current = allocation(&f, &p).await;
    assert!(!current.can_decide);
    assert_eq!(
        current.blocking_reason.as_deref(),
        Some("executor_unavailable")
    );
    assert!(
        f.domain
            .decide_management_allocation(
                &f.coordinator.p,
                &f.policy,
                &a.request_id,
                approve("stale_capacity", 1)
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_inbox(
                &f.coordinator.p,
                &f.policy,
                ManagementQuery {
                    needs_my_action: true,
                    include_allocations: true,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.domain
            .management_allocation_supported(&fresh, &f.policy)
            .await
            .unwrap()
    );
    assert!(
        f.domain
            .poll_management_executor(&fresh, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    let cancelled = release(&f, &current, "cancel_stale_pending").await;
    assert_eq!(cancelled.state, "released");
    assert!(cancelled.balance.is_none());
}
