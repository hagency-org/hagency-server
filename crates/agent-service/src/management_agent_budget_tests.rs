use super::*;
use crate::management_agent_budget_types::*;
use crate::management_allocation_types::{ExecutorRecoverInput, allocation_digest};
pub(crate) async fn remove_migration(db: &mut AsyncPgConnection) {
    super::settlement::remove_migration(db).await;
    db.batch_execute(include_str!("management_agent_budget_remove_test.sql"))
        .await
        .unwrap();
}
async fn ready() -> (Fixture, ManagedAgentRequest, Actor) {
    let (mut f, r, a) = approved().await;
    let d = poll(&f).await;
    submit(&f, &d, reservation(&d)).await.unwrap();
    f.policy = f.policy.with_agent_budget_deltas(true);
    (f, r, a)
}
async fn budget(f: &Fixture, r: &ManagedAgentRequest) -> AgentBudget {
    f.domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
}
async fn request_delta(
    f: &Fixture,
    r: &ManagedAgentRequest,
    key: &str,
    add: i64,
) -> AgentTopUpReply {
    let b = budget(f, r).await;
    f.domain
        .request_agent_topup(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RequestAgentTopUp { approve_immediately: false, owner_funded: false,
                command_id: key.into(),
                expected_request_revision: r.revision,
                expected_assignment_generation: 1,
                expected_budget_revision: b.budget_revision,
                expected_budget_digest: b.budget_digest,
                add_tokens: add,
                purpose: "Continue the approved project".into(),
            },
        )
        .await
        .unwrap()
}
async fn approve_delta(f: &Fixture, a: &Actor, t: &AgentTopUp) -> AgentTopUpReply {
    f.domain
        .decide_agent_topup(
            &a.p,
            &f.policy,
            &t.id,
            DecideAgentTopUp {
                command_id: format!("decide_{}", t.id),
                expected_revision: t.revision,
                expected_authority_revision: 1,
                decision: Decision::Approve,
            },
        )
        .await
        .unwrap()
}
async fn delivered(f: &Fixture) -> AgentBudgetDelivery {
    f.domain
        .poll_agent_budget(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
fn applied(d: &AgentBudgetDelivery, n: i64) -> AgentBudgetResult {
    let c = &d.command;
    AgentBudgetResult {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentBudgetReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            top_up_id: c.top_up_id.clone(),
            agent_reservation_id: c.reserve_receipt.agent_reservation_id.clone().unwrap(),
            assignment_id: c.assignment_id.clone(),
            original_reserve_receipt_digest: c.reserve_receipt_digest.clone(),
            state: "applied".into(),
            expected_budget_revision: c.expected_budget_revision,
            previous_budget_digest: c.expected_budget_digest.clone(),
            budget_revision: c.expected_budget_revision + 1,
            budget_digest: next_budget_digest(c, n).unwrap(),
            previous_ceiling_tokens: c.expected_ceiling_tokens,
            applied_delta_tokens: c.add_tokens,
            effective_ceiling_tokens: c.expected_ceiling_tokens + c.add_tokens,
            ledger_revision: n,
            period_key: c.reserve_command.period_key.clone(),
            reason: None,
        },
    }
}
async fn record(
    f: &Fixture,
    d: &AgentBudgetDelivery,
    r: AgentBudgetResult,
) -> crate::Result<AgentBudgetReceipt> {
    f.domain
        .record_agent_budget(&f.owner.p, &d.command.command_id, r)
        .await
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_roles_http_replay_and_applied_delta_keeps_original_reserve() {
    let (f, r, a) = ready().await;
    let b = budget(&f, &r).await;
    assert_eq!(b.original_ceiling_tokens, 100);
    assert!(!b.execution_supported);
    assert_eq!(b.native_remaining_tokens, None);
    let i = RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id: "topup_http".into(),
        expected_request_revision: r.revision,
        expected_assignment_generation: 1,
        expected_budget_revision: b.budget_revision,
        expected_budget_digest: b.budget_digest,
        add_tokens: 25,
        purpose: "More approved work".into(),
    };
    let svc = service(&f, true);
    let v = post(
        &svc,
        &format!("agent-requests/{}/topups", r.id),
        &f.outsider.token,
        json!(i),
        StatusCode::OK,
    )
    .await;
    let t: AgentTopUpReply =
        serde_json::from_value(json!({"receipt":v["receipt"],"topUp":v["topUp"]})).unwrap();
    for actor in [&f.owner, &f.admin, &f.coordinator] {
        assert!(
            f.domain
                .decide_agent_topup(
                    &actor.p,
                    &f.policy,
                    &t.top_up.id,
                    DecideAgentTopUp {
                        command_id: "wrong_decider".into(),
                        expected_revision: 1,
                        expected_authority_revision: 1,
                        decision: Decision::Approve
                    }
                )
                .await
                .is_err()
        );
    }
    let approved = approve_delta(&f, &a, &t.top_up).await;
    let replay = post(
        &svc,
        &format!("agent-requests/{}/topups", r.id),
        &f.outsider.token,
        json!(i),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v["receipt"], replay["receipt"]);
    assert_eq!(replay["topUp"]["state"], "approved");
    let d = delivered(&f).await;
    assert!(d.application_allowed);
    assert_eq!(d.command.top_up_id, approved.top_up.id);
    let raw = json!(d.command.reserve_command);
    let result = applied(&d, 101);
    let first = record(&f, &d, result.clone()).await.unwrap();
    let second = record(&f, &d, result).await.unwrap();
    assert_eq!(json!(first), json!(second));
    let b = budget(&f, &r).await;
    assert_eq!(
        (
            b.original_ceiling_tokens,
            b.applied_delta_tokens,
            b.effective_ceiling_tokens,
            b.budget_revision
        ),
        (100, 25, 125, 2)
    );
    assert!(!b.execution_supported);
    assert_eq!(json!(d.command.reserve_command), raw);
    let receipt = f
        .domain
        .recover_agent_topup(
            &f.outsider.p,
            &f.policy,
            "topup_http",
            RecoverCommandInput {
                operation: "agent.topup.request".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt["result"]["receipt"], v["receipt"]);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_independent_tickets_recovery_disabled_and_late_cancel_preserve_effect() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "ticket_request", 20).await;
    let t = approve_delta(&f, &a, &t.top_up).await;
    let d = delivered(&f).await;
    let original = applied(&d, 101);
    let disabled = f.policy.clone().with_agent_budget_deltas(false);
    let historical = f
        .domain
        .recover_agent_budget(
            &f.owner.p,
            &disabled,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!historical.application_allowed);
    let another = delivered(&f).await;
    assert_ne!(d.token, another.token);
    let cancelled = f
        .domain
        .cancel_agent_topup(
            &f.outsider.p,
            &disabled,
            &t.top_up.id,
            RevokeInput {
                command_id: "cancel_delta".into(),
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(cancelled.top_up.state, "cancellation_requested");
    record(&f, &d, original).await.unwrap();
    let b = budget(&f, &r).await;
    assert_eq!(b.effective_ceiling_tokens, 120);
    let recovered = f
        .domain
        .recover_agent_budget(
            &f.owner.p,
            &disabled,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest,
            },
        )
        .await
        .unwrap();
    assert!(recovered.recorded.is_some());
    assert!(!recovered.application_allowed);
    assert!(recovered.cancellation_requested);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_observation_only_cannot_fabricate_unissued_application() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "observe_request", 20).await;
    let t = approve_delta(&f, &a, &t.top_up).await;
    let id = t.top_up.executor_command_id.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let payload = sql_query(
        "SELECT payload FROM hagency_agent_v1.management_agent_budget_commands WHERE id=$1",
    )
    .bind::<Text, _>(&id)
    .get_result::<Payload>(&mut db)
    .await
    .unwrap();
    let c: AgentBudgetCommand = serde_json::from_str(&payload.payload).unwrap();
    let d = f
        .domain
        .recover_agent_budget(
            &f.owner.p,
            &f.policy,
            &id,
            ExecutorRecoverInput {
                command_digest: allocation_digest(&c).unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(!d.application_allowed);
    assert_eq!(
        record(&f, &d, applied(&d, 101))
            .await
            .unwrap_err()
            .to_string(),
        "agent_budget_not_issued"
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_stale_head_wrong_owner_and_excess_capacity_refused() {
    let (f, r, _) = ready().await;
    let b = budget(&f, &r).await;
    let i = RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id: "wrong_head".into(),
        expected_request_revision: r.revision,
        expected_assignment_generation: 1,
        expected_budget_revision: 2,
        expected_budget_digest: b.budget_digest,
        add_tokens: 1,
        purpose: "purpose".into(),
    };
    assert_eq!(
        f.domain
            .request_agent_topup(&f.outsider.p, &f.policy, &r.id, i.clone())
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let mut large = i;
    large.expected_budget_revision = 1;
    large.add_tokens = 9007199254740891;
    assert_eq!(
        f.domain
            .request_agent_topup(&f.outsider.p, &f.policy, &r.id, large)
            .await
            .unwrap_err()
            .to_string(),
        "agent_budget_exceeds_project"
    );
    for actor in [&f.owner, &f.admin, &f.coordinator] {
        assert!(
            f.domain
                .agent_budget(&actor.p, &f.policy, &r.id)
                .await
                .is_err()
        );
    }
}
#[test]
fn agent_budget_closed_canonical_fixture_and_original_v2_boundary() {
    let f: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-budget-v1.json"
    ))
    .unwrap();
    let c: AgentBudgetCommand = serde_json::from_value(f["command"].clone()).unwrap();
    c.validate("example.test").unwrap();
    let r: AgentBudgetReceipt = serde_json::from_value(f["receipt"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    for (field, value) in [
        ("command", json!(c)),
        ("receipt", json!(r)),
        ("genesis", f["genesis"].clone()),
        ("nextHead", f["nextHead"].clone()),
    ] {
        assert_eq!(
            allocation_digest(&value).unwrap(),
            f[format!("{field}Digest")]
        );
    }
    assert_eq!(
        next_budget_digest(&c, r.ledger_revision).unwrap(),
        r.budget_digest
    );
    let mut expired = c.clone();
    expired.expires_at_ms = c.reserve_command.expires_at_ms + 1;
    assert!(expired.validate("example.test").is_err());
    let mut unknown = json!(c);
    unknown["extra"] = json!(true);
    assert!(serde_json::from_value::<AgentBudgetCommand>(unknown).is_err());
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_schema18_rollback_reopen_retains_original_and_immutable_delta() {
    let (f, r, a) = ready().await;
    let before = budget(&f, &r).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_agent_budget_results(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=18 AND to_regclass('hagency_agent_v1.management_agent_topups') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_budget_results")
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let after = reopened
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert_eq!(json!(before), json!(after));
    let t = request_delta(&f, &r, "migration_topup", 15).await;
    approve_delta(&f, &a, &t.top_up).await;
    let d = delivered(&f).await;
    let value = record(&f, &d, applied(&d, 101)).await.unwrap();
    for table in [
        "management_agent_budget_commands",
        "management_agent_budget_results",
        "management_agent_budget_issuance",
    ] {
        assert!(
            db.batch_execute(&format!("DELETE FROM hagency_agent_v1.{table}"))
                .await
                .is_err()
        );
    }
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        reopened
            .agent_budget(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
            .effective_ceiling_tokens,
        115
    );
    assert_eq!(
        json!(
            reopened
                .record_agent_budget(&f.owner.p, &d.command.command_id, applied(&d, 101))
                .await
                .unwrap()
        ),
        json!(value)
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_ticket_expiry_cap_and_generation_do_not_weaken_old_evidence() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "finite_topup", 10).await;
    approve_delta(&f, &a, &t.top_up).await;
    let d = delivered(&f).await;
    let mut other = f
        .auth
        .authenticate(&f.owner.token, now_ms(), true)
        .await
        .unwrap();
    other.device_generation = Some(other.device_generation.unwrap() + 1);
    assert!(
        f.domain
            .record_agent_budget(&other, &d.command.command_id, applied(&d, 101))
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_agent_budget_tickets SET issued_at_ms=1,expires_at_ms=2 WHERE command_id=$1").bind::<Text,_>(&d.command.command_id).execute(&mut db).await.unwrap();
    assert_eq!(
        record(&f, &d, applied(&d, 101))
            .await
            .unwrap_err()
            .to_string(),
        "agent_budget_ticket_changed"
    );
    let fresh = delivered(&f).await;
    for _ in 0..63 {
        delivered(&f).await;
    }
    assert_eq!(
        f.domain
            .poll_agent_budget(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap_err()
            .to_string(),
        "agent_budget_ticket_capacity"
    );
    record(&f, &fresh, applied(&fresh, 101)).await.unwrap();
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_cancel_before_delivery_and_refusal_never_credit() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "cancel_before", 10).await;
    let t = approve_delta(&f, &a, &t.top_up).await;
    let disabled = f.policy.clone().with_agent_budget_deltas(false);
    f.domain
        .cancel_agent_topup(
            &f.outsider.p,
            &disabled,
            &t.top_up.id,
            RevokeInput {
                command_id: "cancel_before_issue".into(),
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    let d = delivered(&f).await;
    assert!(!d.application_allowed && d.cancellation_requested);
    let mut v = applied(&d, 101);
    v.receipt.state = "cancelled_before_apply".into();
    v.receipt.budget_revision = 1;
    v.receipt.budget_digest = d.command.expected_budget_digest.clone();
    v.receipt.applied_delta_tokens = 0;
    v.receipt.effective_ceiling_tokens = 100;
    v.receipt.reason = Some("cancelled".into());
    record(&f, &d, v).await.unwrap();
    assert_eq!(budget(&f, &r).await.effective_ceiling_tokens, 100);
    let next = request_delta(&f, &r, "next_after_cancel", 5).await;
    assert_eq!(next.top_up.expected_budget_revision, 1);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_current_authority_removed_cleanup_remains_but_never_apply() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "authority_topup", 10).await;
    approve_delta(&f, &a, &t.top_up).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_projects SET state='revoked',revision=revision+1 WHERE id=$1").bind::<Text,_>(&r.project_id).execute(&mut db).await.unwrap();
    let d = delivered(&f).await;
    assert!(!d.application_allowed);
    assert!(d.cancellation_requested);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_replacement_generation_only_recovers_original_issued_evidence() {
    let (f, r, a) = ready().await;
    let t = request_delta(&f, &r, "replacement_topup", 10).await;
    approve_delta(&f, &a, &t.top_up).await;
    let d = delivered(&f).await;
    let old = applied(&d, 101);
    let device = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "same original installation".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&device.token, now_ms(), true)
        .await
        .unwrap();
    assert_eq!(p.device_id, f.owner.p.device_id);
    assert_eq!(
        p.device_generation,
        Some(f.owner.p.device_generation.unwrap() + 1)
    );
    assert!(record(&f, &d, old.clone()).await.is_err());
    let recovery = f
        .domain
        .recover_agent_budget(
            &p,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!recovery.application_allowed);
    assert!(recovery.cancellation_requested);
    let mut late = old;
    late.token = recovery.token;
    f.domain
        .record_agent_budget(&p, &d.command.command_id, late)
        .await
        .unwrap();
    assert_eq!(budget(&f, &r).await.effective_ceiling_tokens, 110);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_two_domain_instances_serialize_one_unknown_delta_and_global_revision() {
    let (f, r, a) = ready().await;
    let b = budget(&f, &r).await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let i = RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id: "parallel_original".into(),
        expected_request_revision: r.revision,
        expected_assignment_generation: 1,
        expected_budget_revision: 1,
        expected_budget_digest: b.budget_digest,
        add_tokens: 10,
        purpose: "Exact one request".into(),
    };
    let (one, two) = tokio::join!(
        f.domain
            .request_agent_topup(&f.outsider.p, &f.policy, &r.id, i.clone()),
        other.request_agent_topup(&f.outsider.p, &f.policy, &r.id, i.clone())
    );
    let one = one.unwrap();
    assert_eq!(json!(one.receipt), json!(two.unwrap().receipt));
    let mut changed = i;
    changed.command_id = "another_pending".into();
    assert_eq!(
        other
            .request_agent_topup(&f.outsider.p, &f.policy, &r.id, changed)
            .await
            .unwrap_err()
            .to_string(),
        "agent_topup_pending"
    );
    approve_delta(&f, &a, &one.top_up).await;
    let d = delivered(&f).await;
    assert_eq!(
        record(&f, &d, applied(&d, 100))
            .await
            .unwrap_err()
            .to_string(),
        "invalid_agent_budget"
    );
    record(&f, &d, applied(&d, 101)).await.unwrap();
    let t = request_delta(&f, &r, "second_delta", 5).await;
    approve_delta(&f, &a, &t.top_up).await;
    let d = delivered(&f).await;
    assert_eq!(
        record(&f, &d, applied(&d, 101))
            .await
            .unwrap_err()
            .to_string(),
        "agent_budget_changed"
    );
    record(&f, &d, applied(&d, 102)).await.unwrap();
    assert_eq!(budget(&f, &r).await.effective_ceiling_tokens, 115);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_history_pagination_disabled_replay_and_exact_body_conflict() {
    let (f, r, _) = ready().await;
    let mut ids = Vec::new();
    for n in 0..3 {
        let t = request_delta(&f, &r, &format!("page_{n}"), 1).await;
        ids.push(t.top_up.id.clone());
        f.domain
            .cancel_agent_topup(
                &f.outsider.p,
                &f.policy,
                &t.top_up.id,
                RevokeInput {
                    command_id: format!("page_cancel_{n}"),
                    expected_revision: 1,
                },
            )
            .await
            .unwrap();
    }
    ids.sort();
    let disabled = f.policy.clone().with_agent_budget_deltas(false);
    let one = f
        .domain
        .query_agent_topups(
            &f.outsider.p,
            &disabled,
            &r.id,
            AgentTopUpQuery {
                after: None,
                limit: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(one.items[0].id, ids[0]);
    assert_eq!(one.next_after, Some(ids[0].clone()));
    let two = f
        .domain
        .query_agent_topups(
            &f.outsider.p,
            &disabled,
            &r.id,
            AgentTopUpQuery {
                after: one.next_after,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        two.items.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ids[1..]
    );
    assert!(two.next_after.is_none());
    assert!(
        f.domain
            .query_agent_topups(&f.owner.p, &disabled, &r.id, AgentTopUpQuery::default())
            .await
            .is_err()
    );
    let b = budget(&f, &r).await;
    let changed = RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id: "page_0".into(),
        expected_request_revision: r.revision,
        expected_assignment_generation: 1,
        expected_budget_revision: 1,
        expected_budget_digest: b.budget_digest,
        add_tokens: 2,
        purpose: "changed original body".into(),
    };
    assert_eq!(
        f.domain
            .request_agent_topup(&f.outsider.p, &disabled, &r.id, changed)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
    assert!(
        !f.domain
            .agent_budget(&f.outsider.p, &disabled, &r.id)
            .await
            .unwrap()
            .can_request
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_discovery_is_protocol_only_and_disabled_has_no_fresh_apply() {
    let (mut f, r, a) = ready().await;
    let t = request_delta(&f, &r, "disabled_delivery", 10).await;
    approve_delta(&f, &a, &t.top_up).await;
    f.policy = f.policy.with_agent_budget_deltas(false);
    let svc = service(&f, true);
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&svc)
        .await;
    let v = response.take_json::<Value>().await.unwrap();
    assert!(
        v["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "managed-agent-budget-delta-v1")
    );
    let d = delivered(&f).await;
    assert!(!d.application_allowed);
    let b = budget(&f, &r).await;
    assert!(!b.execution_supported && !b.can_request);
    assert_eq!(b.effective_ceiling_tokens, 100);
    assert_eq!(b.native_remaining_tokens, None);
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn cleanup_budget_applied_and_unacknowledged_increases_survive_fencing() {
    let (mut f, r, admin) = ready().await;
    let topup = request_delta(&f, &r, "cleanup_first_delta", 25).await;
    // A request alone never changes the original held amount.
    assert_eq!(f.domain.managed_agent_removal(&f.outsider.p, &r.id).await.unwrap().usage.retained_tokens_floor, Some(100));
    approve_delta(&f, &admin, &topup.top_up).await;
    assert_eq!(f.domain.managed_agent_removal(&admin.p, &r.id).await.unwrap().usage.retained_tokens_floor, Some(125));
    let delivery = delivered(&f).await;
    record(&f, &delivery, applied(&delivery, 101)).await.unwrap();
    let second = request_delta(&f, &r, "cleanup_second_delta", 15).await;
    approve_delta(&f, &admin, &second.top_up).await;
    // The second command can have applied with its reply lost. It is not free
    // capacity, and fencing cannot silently erase its possible native hold.
    f.domain.revoke_managed_agent_request(&admin.p, &f.policy, &r.id, RevokeInput {
        command_id: "cleanup_budget_revoke".into(), expected_revision: r.revision,
    }).await.unwrap();
    let fence = poll(&f).await;
    let mut result = fence_result(&fence, true);
    let AgentExecutorReceipt::Fence(receipt) = &mut result.receipt else { panic!("fence") };
    receipt.ledger_revision = 102;
    receipt.fence_revision = 102;
    submit(&f, &fence, result).await.unwrap();
    f.policy = f.policy.with_agent_budget_deltas(false).with_agent_admission(false);
    let after = f.domain.managed_agent_removal(&f.outsider.p, &r.id).await.unwrap();
    assert!(after.central_fenced);
    assert_eq!(after.usage.retained_tokens_floor, Some(140));
    assert_eq!(after.usage.fence_usage_complete, Some(false));
    assert_eq!(after.usage.known_consumed_tokens, Some(0));
    assert!(!after.can_cancel && !after.can_revoke);
    assert!(f.domain.managed_agent_removal(&f.owner.p, &r.id).await.is_err());
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn cleanup_budget_complete_native_fence_still_can_report_known_unused() {
    let (f, r, admin) = ready().await;
    let topup = request_delta(&f, &r, "unused_delta", 25).await;
    approve_delta(&f, &admin, &topup.top_up).await;
    let delta = delivered(&f).await;
    record(&f, &delta, applied(&delta, 101)).await.unwrap();
    f.domain.revoke_managed_agent_request(&admin.p, &f.policy, &r.id, RevokeInput {
        command_id: "unused_budget_revoke".into(), expected_revision: r.revision,
    }).await.unwrap();
    let fence = poll(&f).await;
    let mut result = fence_result(&fence, true);
    let AgentExecutorReceipt::Fence(receipt) = &mut result.receipt else { panic!("fence") };
    receipt.ledger_revision = 102;
    receipt.fence_revision = 102;
    receipt.retained_tokens = Some(0);
    receipt.usage_complete = true;
    receipt.process_state = "not_started".into();
    submit(&f, &fence, result).await.unwrap();
    let after = f.domain.managed_agent_removal(&admin.p, &r.id).await.unwrap();
    assert_eq!(after.usage.retained_tokens_floor, Some(0));
    assert_eq!(after.usage.fence_usage_complete, Some(true));
    assert_eq!(after.usage.known_consumed_tokens, Some(0));
    let matrix = after.matrix.unwrap();
    assert_eq!(matrix.state, "fenced");
    assert!(matrix.cleanup_observed_at_ms.is_none());
}

#[tokio::test]
#[ignore="dedicated PostgreSQL"]
async fn settlement_pending_delta_cannot_lower_hold_and_late_result_makes_exact_head_current() {
    let (f,r,a)=ready().await;
    let topup=request_delta(&f,&r,"settlement_lost_delta",25).await;
    approve_delta(&f,&a,&topup.top_up).await;
    let delta=delivered(&f).await;
    let applied_result=applied(&delta,101);
    f.domain.revoke_managed_agent_request(&a.p,&f.policy,&r.id,RevokeInput{command_id:"settlement_pending_revoke".into(),expected_revision:r.revision}).await.unwrap();
    let d=poll(&f).await;let mut fenced=fence_result(&d,true);
    let AgentExecutorReceipt::Fence(fr)=&mut fenced.receipt else{panic!("fence")};fr.fence_revision=110;fr.ledger_revision=110;
    submit(&f,&d,fenced).await.unwrap();
    let delivery=super::settlement::poll_settlement(&f).await;
    let mut proof=super::settlement::result(&f,&delivery,120,40,40,true).await;
    proof.receipt.fence_revision=110;
    proof.receipt.budget_revision=Some(2);
    proof.receipt.budget_digest=Some(applied_result.receipt.budget_digest.clone());
    proof.receipt.effective_ceiling_tokens=Some(125);
    super::settlement::record(&f,&delivery,proof).await.unwrap();
    let v=f.domain.managed_agent_settlement(&a.p,&r.id).await.unwrap();assert!(!v.settlement_current);assert_eq!(v.retained_tokens_floor,Some(125));
    assert_eq!(budget(&f,&r).await.conservative_commitment_tokens,125);
    // The earlier native delta was applied before the fence; only its HTTP
    // acknowledgement was lost. Its arrival matches the already saved proof.
    record(&f,&delta,applied_result).await.unwrap();
    let v=f.domain.managed_agent_settlement(&a.p,&r.id).await.unwrap();assert!(v.settlement_current);assert_eq!(v.retained_tokens_floor,Some(40));
    assert_eq!(budget(&f,&r).await.conservative_commitment_tokens,40);
    assert!(f.domain.poll_agent_settlement(&f.owner.p,ExecutorPollInput::default()).await.unwrap().is_none());
}

#[tokio::test]
#[ignore="dedicated PostgreSQL"]
async fn settlement_later_cancellation_evidence_queues_new_observation_and_cannot_share_ledger_revision() {
    let (f,r,a)=ready().await;
    let topup=request_delta(&f,&r,"settlement_cancel_delta",25).await;
    approve_delta(&f,&a,&topup.top_up).await;
    f.domain.revoke_managed_agent_request(&a.p,&f.policy,&r.id,RevokeInput{command_id:"settlement_cancel_revoke".into(),expected_revision:r.revision}).await.unwrap();
    let d=poll(&f).await;submit(&f,&d,fence_result(&d,true)).await.unwrap();
    let delivery=super::settlement::poll_settlement(&f).await;
    super::settlement::record(&f,&delivery,super::settlement::result(&f,&delivery,120,40,40,true).await).await.unwrap();
    assert_eq!(budget(&f,&r).await.conservative_commitment_tokens,125);
    let delta=delivered(&f).await;assert!(delta.cancellation_requested);
    let mut cancelled=applied(&delta,120);
    cancelled.receipt.state="cancelled_before_apply".into();
    cancelled.receipt.budget_revision=1;cancelled.receipt.budget_digest=delta.command.expected_budget_digest.clone();
    cancelled.receipt.applied_delta_tokens=0;cancelled.receipt.effective_ceiling_tokens=100;cancelled.receipt.reason=Some("cancelled".into());
    assert!(record(&f,&delta,cancelled.clone()).await.is_err());
    cancelled.receipt.ledger_revision=130;record(&f,&delta,cancelled).await.unwrap();
    let v=f.domain.managed_agent_settlement(&a.p,&r.id).await.unwrap();assert!(!v.settlement_current);assert_eq!(v.retained_tokens_floor,Some(100));
    let next=super::settlement::poll_settlement(&f).await;assert_ne!(next.command.command_id,delivery.command.command_id);
    assert_eq!(super::settlement::poll_settlement(&f).await.command.command_id,next.command.command_id);
    let new=super::settlement::result(&f,&next,140,40,40,true).await;super::settlement::record(&f,&next,new).await.unwrap();
    assert!(f.domain.managed_agent_settlement(&a.p,&r.id).await.unwrap().settlement_current);
    assert_eq!(budget(&f,&r).await.conservative_commitment_tokens,40);
    assert!(f.domain.poll_agent_settlement(&f.owner.p,ExecutorPollInput::default()).await.unwrap().is_none());
}
