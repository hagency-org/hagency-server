use super::*;
use crate::management_parent_types::*;
async fn prepared_parent() -> (
    Fixture,
    Admission,
    ProjectPlan,
    ResourceGrantView,
    ProjectAllocation,
) {
    let (mut f, c, p, g, a) = prepared().await;
    f.policy = f.policy.with_parent_settlement(true);
    (f, c, p, g, a)
}

async fn close(f: &Fixture, g: &ResourceGrantView) -> ParentSettlement {
    f.domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            ParentCloseInput {
                command_id: "close_parent".into(),
                expected_grant_revision: g.revision,
            },
        )
        .await
        .unwrap()
}

async fn parent_delivery(f: &Fixture) -> ParentDelivery {
    f.domain
        .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap()
        .unwrap()
}

fn parent_result(
    d: &ParentDelivery,
    g: &ResourceGrantView,
    revision: i64,
    consumed: i64,
    unknown: i64,
) -> ParentResultInput {
    ParentResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: NativeGrantSettlementReceipt {
            version: 1,
            command_id: d.command.command_id.clone(),
            command_digest: d.command_digest.clone(),
            grant_id: g.id.clone(),
            grant_receipt_digest: g.receipt_digest.clone(),
            enrollment_id: g.receipt.enrollment_id.clone(),
            period_key: g.receipt.period_key.clone(),
            fence_revision: 12,
            ledger_revision: revision,
            state: if unknown == 0 { "closed" } else { "retained" }.into(),
            retained_tokens: consumed + unknown,
            known_consumed_tokens: consumed,
            unresolved_tokens: unknown,
            liability_count: i64::from(consumed + unknown > 0),
            liability_digest: "b".repeat(64),
        },
    }
}

async fn report(
    f: &Fixture,
    d: &ParentDelivery,
    g: &ResourceGrantView,
    revision: i64,
    consumed: i64,
    unknown: i64,
) -> ParentAcknowledgement {
    f.domain
        .record_management_parent_result(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            parent_result(d, g, revision, consumed, unknown),
        )
        .await
        .unwrap()
}

#[test]
fn canonical_parent_contract_is_closed_and_matches_shared_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-parent-settlement-v1.json"
    ))
    .unwrap();
    let c: GrantClosureCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
    let r: NativeGrantSettlementReceipt =
        serde_json::from_value(fixture["receipt"].clone()).unwrap();
    assert_eq!(allocation_digest(&c).unwrap(), fixture["commandDigest"]);
    assert_eq!(allocation_digest(&r).unwrap(), fixture["receiptDigest"]);
    let mut bad = fixture["receipt"].clone();
    bad["ownerApproved"] = json!(true);
    assert!(serde_json::from_value::<NativeGrantSettlementReceipt>(bad).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_gate_roles_and_missing_proof_never_release_capacity() {
    let (mut f, _, p, g, _) = prepared_parent().await;
    f.policy = f.policy.with_parent_settlement(false);
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap_err()
            .to_string(),
        "parent_settlement_unavailable"
    );
    f.policy = f.policy.with_parent_settlement(true);
    let held = f
        .domain
        .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
        .await
        .unwrap();
    assert_eq!(held.held_tokens, 1_000_000);
    assert_eq!(held.known_unused_released_tokens, 0);
    assert!(held.receipt.is_none());
    for actor in [&f.outsider.p] {
        assert!(
            f.domain
                .close_management_parent(
                    actor,
                    &f.policy,
                    &g.id,
                    ParentCloseInput {
                        command_id: "wrong_close".into(),
                        expected_grant_revision: g.revision
                    }
                )
                .await
                .is_err()
        );
    }
    let closing = close(&f, &g).await;
    assert_eq!(closing.state, "closing");
    assert_eq!(closing.held_tokens, 1_000_000);
    assert_eq!(closing.known_unused_released_tokens, 0);
    assert_eq!(allocation(&f, &p).await.state, "released");
    assert!(!allocation(&f, &p).await.can_decide);
    assert!(
        f.domain
            .settle_management_parent(
                &f.owner.p,
                &f.policy,
                &g.id,
                RevokeInput {
                    command_id: "unsafe_retry".into(),
                    expected_revision: closing.revision
                }
            )
            .await
            .is_err()
    );
    let d = parent_delivery(&f).await;
    assert!(d.execution_allowed);
    assert_eq!(d.command.operation, "grant.close");
    let other = fresh_installation(&f, "other_device").await;
    assert!(
        f.domain
            .recover_management_parent_executor(
                &other,
                &f.policy,
                &d.command.command_id,
                ParentRecoverInput {
                    command_digest: d.command_digest.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .record_management_parent_result(
                &f.admin.p,
                &f.policy,
                &d.command.command_id,
                parent_result(&d, &g, 12, 0, 0)
            )
            .await
            .is_err()
    );
    let mut invalid = parent_result(&d, &g, 12, 0, 0);
    invalid.receipt.fence_revision = 10;
    assert!(
        f.domain
            .record_management_parent_result(&f.owner.p, &f.policy, &d.command.command_id, invalid)
            .await
            .is_err()
    );
    let mut invalid = parent_result(&d, &g, 12, 1, 0);
    invalid.receipt.retained_tokens = 0;
    assert!(
        f.domain
            .record_management_parent_result(&f.owner.p, &f.policy, &d.command.command_id, invalid)
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .held_tokens,
        1_000_000
    );
    let done = report(&f, &d, &g, 12, 0, 0).await;
    assert_eq!(done.settlement.state, "closed");
    assert_eq!(done.settlement.held_tokens, 0);
    assert_eq!(done.settlement.known_unused_released_tokens, 1_000_000);
    assert_eq!(
        report(&f, &d, &g, 12, 0, 0).await.settlement.revision,
        done.settlement.revision
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_retained_settlement_and_overrun_preserve_immutable_original_and_old_ack() {
    let (f, _, _, g, _) = prepared_parent().await;
    close(&f, &g).await;
    let d = parent_delivery(&f).await;
    let retained = report(&f, &d, &g, 12, 100, 500).await;
    assert_eq!(retained.settlement.state, "retained");
    assert_eq!(retained.settlement.held_tokens, 600);
    assert_eq!(retained.settlement.known_unused_released_tokens, 999400);
    f.domain
        .settle_management_parent(
            &f.admin.p,
            &f.policy,
            &g.id,
            RevokeInput {
                command_id: "settle_progress".into(),
                expected_revision: retained.settlement.revision,
            },
        )
        .await
        .unwrap();
    let later = parent_delivery(&f).await;
    assert_eq!(later.command.operation, "grant.settle");
    assert_eq!(
        later.command.previous_settlement_digest.as_deref(),
        Some(retained.receipt_digest.as_str())
    );
    for invalid in [
        parent_result(&later, &g, 12, 100, 0),
        parent_result(&later, &g, 13, 99, 0),
    ] {
        assert!(
            f.domain
                .record_management_parent_result(
                    &f.owner.p,
                    &f.policy,
                    &later.command.command_id,
                    invalid
                )
                .await
                .is_err()
        );
    }
    let settled = report(&f, &later, &g, 13, 100, 0).await;
    assert_eq!(settled.settlement.held_tokens, 100);
    let old = report(&f, &d, &g, 12, 100, 500).await;
    assert_eq!(old.settlement.held_tokens, 100);
    assert_eq!(old.receipt_digest, retained.receipt_digest);
    f.domain
        .settle_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            RevokeInput {
                command_id: "settle_late_usage".into(),
                expected_revision: settled.settlement.revision,
            },
        )
        .await
        .unwrap();
    let correction = parent_delivery(&f).await;
    let overrun = report(&f, &correction, &g, 14, 1_000_123, 0).await;
    assert_eq!(overrun.settlement.overrun_tokens, 123);
    assert_eq!(overrun.settlement.known_unused_released_tokens, 0);
    assert_eq!(overrun.settlement.original_granted_tokens, 1_000_000);
    assert_eq!(
        f.domain
            .management_resource_grant(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .receipt_digest,
        g.receipt_digest
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_receipt_recovers_after_lost_reply_rotation_and_expiry_without_reexecution() {
    let (f, _, _, g, _) = prepared_parent().await;
    close(&f, &g).await;
    let d = parent_delivery(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "UPDATE hagency_agent_v1.management_parent_commands SET lease_until_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1",
    )
    .bind::<Text, _>(&d.command.command_id)
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .state,
        "outcome_unknown"
    );
    assert!(
        f.domain
            .record_management_parent_result(
                &f.owner.p,
                &f.policy,
                &d.command.command_id,
                parent_result(&d, &g, 12, 0, 0)
            )
            .await
            .is_err()
    );
    let fresh = fresh_installation(&f, "macbook").await;
    let r = f
        .domain
        .recover_management_parent_executor(
            &fresh,
            &f.policy,
            &d.command.command_id,
            ParentRecoverInput {
                command_digest: d.command_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(!r.execution_allowed);
    assert!(r.recorded.is_none());
    let done = f
        .domain
        .record_management_parent_result(
            &fresh,
            &f.policy,
            &r.command.command_id,
            parent_result(&r, &g, 12, 0, 0),
        )
        .await
        .unwrap();
    assert_eq!(done.settlement.state, "closed");
    let latest = f
        .domain
        .recover_management_parent_command(
            &fresh,
            &f.policy,
            "close_parent",
            RecoverCommandInput {
                operation: "parent.close".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(latest["settlement"]["state"], "closed");
    assert!(
        f.domain
            .poll_management_parent_executor(&fresh, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn revoke_races_enqueue_only_one_logical_parent_fence() {
    let (f, c, _, g, a) = prepared_parent().await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (withdraw, decision) = tokio::join!(
        f.domain.revoke_contribution(
            &f.admin.p,
            &f.policy,
            &c.contribution.id,
            RevokeInput {
                command_id: "admin_withdraw_parent".into(),
                expected_revision: c.contribution.revision
            }
        ),
        other.decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &a.request_id,
            approve("race_child", 1)
        )
    );
    withdraw.unwrap();
    let _ = decision;
    let updated = f
        .domain
        .management_resource_grant(&f.owner.p, &f.policy, &g.id)
        .await
        .unwrap();
    f.domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            ParentCloseInput {
                command_id: "close_already_fenced".into(),
                expected_grant_revision: updated.revision,
            },
        )
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type=BigInt)]
        count: i64,
    }
    assert_eq!(sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_parent_commands WHERE grant_id=$1 AND operation='grant.close'").bind::<Text,_>(&g.id).get_result::<Count>(&mut db).await.unwrap().count,1);
    let d = parent_delivery(&f).await;
    assert_eq!(d.command.reason, "contribution_revoked");
    assert!(d.execution_allowed);
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .held_tokens,
        1_000_000
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_fence_orders_child_receipts_and_keeps_later_positive_liability() {
    let (f, _, _, g, a) = prepared_parent().await;
    accepted(&f, &a).await;
    let child = delivered(&f).await;
    record(&f, &child, "reserved", 11).await;
    close(&f, &g).await;
    let parent = parent_delivery(&f).await;
    let mut bad = parent_result(&parent, &g, 12, 0, 0);
    bad.receipt.fence_revision = 11;
    assert!(
        f.domain
            .record_management_parent_result(&f.owner.p, &f.policy, &parent.command.command_id, bad)
            .await
            .is_err()
    );
    report(&f, &parent, &g, 12, 0, 0).await;
    let release_delivery = delivered(&f).await;
    let mut late = result(&release_delivery, "released", 13);
    late.receipt.retained_tokens = 123;
    f.domain
        .record_management_executor_result(
            &f.owner.p,
            &f.policy,
            &release_delivery.command.command_id,
            late,
        )
        .await
        .unwrap();
    let projection = f
        .domain
        .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
        .await
        .unwrap();
    assert_eq!(projection.state, "awaiting_reconciliation");
    assert_eq!(projection.held_tokens, 123);
    assert_eq!(projection.unreconciled_child_tokens, 123);
    f.domain
        .settle_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            RevokeInput {
                command_id: "reconcile_child_usage".into(),
                expected_revision: projection.revision,
            },
        )
        .await
        .unwrap();
    let next = parent_delivery(&f).await;
    // Equal revision cannot mean the independent parent mutation incorporated
    // a child receipt: native commands each advance the shared owner ledger.
    assert_eq!(
        f.domain
            .record_management_parent_result(
                &f.owner.p,
                &f.policy,
                &next.command.command_id,
                parent_result(&next, &g, 13, 0, 0),
            )
            .await
            .unwrap_err()
            .to_string(),
        "parent_ledger_revision_conflict"
    );
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .held_tokens,
        123
    );
    let done = report(&f, &next, &g, 14, 123, 0).await;
    assert_eq!(done.settlement.state, "closed");
    assert_eq!(done.settlement.held_tokens, 123);
    assert_eq!(done.settlement.unreconciled_child_tokens, 0);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn late_child_receipt_cannot_share_a_historical_parent_mutation_revision() {
    let (f, _, p, g, a) = prepared_parent().await;
    accepted(&f, &a).await;
    let child = delivered(&f).await;
    record(&f, &child, "reserved", 11).await;
    close(&f, &g).await;
    let parent = parent_delivery(&f).await;
    let first = report(&f, &parent, &g, 12, 0, 0).await;
    f.domain
        .settle_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            RevokeInput {
                command_id: "later_parent_proof".into(),
                expected_revision: first.settlement.revision,
            },
        )
        .await
        .unwrap();
    let settlement = parent_delivery(&f).await;
    report(&f, &settlement, &g, 14, 0, 0).await;
    let child_release = delivered(&f).await;
    for revision in [12, 14] {
        let mut invalid = result(&child_release, "released", revision);
        invalid.receipt.retained_tokens = 123;
        assert_eq!(
            f.domain
                .record_management_executor_result(
                    &f.owner.p,
                    &f.policy,
                    &child_release.command.command_id,
                    invalid,
                )
                .await
                .unwrap_err()
                .to_string(),
            "parent_ledger_revision_conflict"
        );
    }
    assert_eq!(allocation(&f, &p).await.state, "releasing");
    let mut current = result(&child_release, "released", 15);
    current.receipt.retained_tokens = 123;
    f.domain
        .record_management_executor_result(
            &f.owner.p,
            &f.policy,
            &child_release.command.command_id,
            current,
        )
        .await
        .unwrap();
    let parent = f
        .domain
        .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
        .await
        .unwrap();
    assert_eq!(parent.state, "awaiting_reconciliation");
    assert_eq!(parent.held_tokens, 123);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn late_child_reserve_must_precede_the_permanent_parent_fence() {
    let (f, _, p, g, a) = prepared_parent().await;
    accepted(&f, &a).await;
    let child = delivered(&f).await;
    close(&f, &g).await;
    let parent = parent_delivery(&f).await;
    report(&f, &parent, &g, 12, 0, 0).await;
    // The central side has not yet seen the native reserve. A receipt asserting
    // it happened after the native fence contradicts the durable parent proof.
    assert_eq!(
        f.domain
            .record_management_executor_result(
                &f.owner.p,
                &f.policy,
                &child.command.command_id,
                result(&child, "reserved", 13),
            )
            .await
            .unwrap_err()
            .to_string(),
        "parent_fence_revision_conflict"
    );
    let historical = record(&f, &child, "reserved", 11).await;
    assert_eq!(historical.allocation.state, "releasing");
    assert_eq!(
        historical.allocation.balance.unwrap().retained_tokens,
        100_000
    );
    // The later complete parent proof covers that historical balance without
    // forging an individual child release or resurrecting execution authority.
    assert!(!allocation(&f, &p).await.can_decide);
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .held_tokens,
        0
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_enqueue_failure_rolls_back_grant_fence_and_child_cleanup() {
    let (f, _, p, g, a) = prepared_parent().await;
    accepted(&f, &a).await;
    let child = delivered(&f).await;
    record(&f, &child, "reserved", 11).await;
    let function = format!("parent_enqueue_fail_{}", &secret_token()[..12]);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected_parent_enqueue_failure'; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_parent_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    let failed = f
        .domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            ParentCloseInput {
                command_id: "failed_parent_enqueue".into(),
                expected_grant_revision: g.revision,
            },
        )
        .await;
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_parent_commands; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(failed.is_err());
    let grant = f
        .domain
        .management_resource_grant(&f.owner.p, &f.policy, &g.id)
        .await
        .unwrap();
    assert_eq!(grant.state, "active");
    assert_eq!(grant.revision, g.revision);
    let allocation = allocation(&f, &p).await;
    assert_eq!(allocation.state, "reserved");
    assert_eq!(allocation.balance.unwrap().retained_tokens, 100_000);
    assert!(
        f.domain
            .poll_management_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(close(&f, &g).await.state, "closing");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v4_parent_migration_is_atomic_and_preserves_native_grant_evidence() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_parent_upgrade_{}",
        &crate::hash(&secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move {
        let auth = Store::open(url.as_str(), "example.test", "https://example.test/auth/").await.unwrap();
        let owner = actor(&auth, "legacy_grant_owner").await;
        let policy = ManagementPolicy::new(owner.p.mxid.clone(), "example.test").unwrap().with_project_reservations(true).with_parent_settlement(true);
        let mut db = AsyncPgConnection::establish(url.as_str()).await.unwrap();
        for schema in [
            include_str!("management_schema.sql"),
            include_str!("management_verification_schema.sql"),
            include_str!("management_projects_schema.sql"),
            include_str!("management_allocations_schema.sql"),
        ] {
            db.batch_execute(schema).await.unwrap();
        }
        let fixture: Value = serde_json::from_str(include_str!("../tests/fixtures/management-allocation-v1.json")).unwrap();
        let mut native: NativeGrantReceipt = serde_json::from_value(fixture["grant"].clone()).unwrap();
        native.contribution_id = "legacy_parent_contribution".into();
        native.owner_mxid = owner.p.mxid.clone();
        native.coordinator_mxid = owner.p.mxid.clone();
        native.executor_device_id = owner.p.device_id.clone().unwrap();
        native.executor_generation = owner.p.device_generation.unwrap();
        let bytes = serde_json::to_string(&native).unwrap();
        let digest = allocation_digest(&native).unwrap();
        let now = now_ms();
        db.transaction::<_, diesel::result::Error, _>(async |db: &mut AsyncPgConnection| {
            sql_query("INSERT INTO hagency_agent_v1.management_contributions(id,owner_user_id,executor_device_id,executor_generation,resource_id,resource_revision,label,runtime,capabilities,coordinator_mxid,state,revision,request_id,created_at_ms,updated_at_ms) VALUES($1,$2,$3,$4,$5,$6,'Original native grant','codex','[\"coding\"]',$7,'admitted',2,'legacy_parent_request',$8,$8)")
                .bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).bind::<BigInt,_>(native.executor_generation).bind::<Text,_>(&native.resource_id).bind::<Text,_>(&native.resource_revision).bind::<Text,_>(&native.coordinator_mxid).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) VALUES('legacy_parent_request','resource_admission',$1,$2,$3,'approved',2,$4,$5,$4)")
                .bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&owner.p.mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(now+600000).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,'revoked',$8,$9,$10)")
                .bind::<Text,_>(&native.grant_id).bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).bind::<BigInt,_>(native.executor_generation).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).bind::<Text,_>(&native.period_key).bind::<BigInt,_>(native.period_end_ms).bind::<BigInt,_>(now).execute(db).await?;
            Ok(())
        }).await.unwrap();
        // Conflict on the second CREATE proves the first table and version
        // update roll back while the old native receipt stays byte-identical.
        db.batch_execute("CREATE TABLE hagency_agent_v1.management_parent_commands (id text)").await.unwrap();
        assert!(DomainStore::open(url.as_str(), "example.test", "_hagency_test_").await.is_err());
        #[derive(QueryableByName)]
        struct State { #[diesel(sql_type=diesel::sql_types::Bool)] matched: bool }
        assert!(sql_query("SELECT version=4 AND to_regclass('hagency_agent_v1.management_parent_settlements') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
        assert!(sql_query("SELECT receipt=$2 AND receipt_digest=$3 AS matched FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).get_result::<State>(&mut db).await.unwrap().matched);
        db.batch_execute("DROP TABLE hagency_agent_v1.management_parent_commands").await.unwrap();
        for _ in 0..2 {
            let domain = DomainStore::open(url.as_str(), "example.test", "_hagency_test_").await.unwrap();
            let parent = domain.management_parent_settlement(&owner.p, &policy, &native.grant_id).await.unwrap();
            assert_eq!(parent.state, "awaiting_native_close");
            assert_eq!(parent.held_tokens, native.allocated_tokens);
            assert_eq!(parent.known_unused_released_tokens, 0);
            assert!(parent.receipt.is_none());
            assert!(sql_query("SELECT version=29 AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
            assert!(sql_query("SELECT receipt=$2 AND receipt_digest=$3 AS matched FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).get_result::<State>(&mut db).await.unwrap().matched);
        }
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn parent_outbox_failures_rollback_revoke_command_and_result_without_false_refund() {
    let (f, _, p, g, _) = prepared_parent().await;
    let function = format!("parent_fail_{}", &secret_token()[..12]);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_parent_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.owner.p.mxid)).await.unwrap();
    let failed = f
        .domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &g.id,
            ParentCloseInput {
                command_id: "atomic_parent_close".into(),
                expected_grant_revision: g.revision,
            },
        )
        .await;
    db.batch_execute(&format!(
        "DROP TRIGGER {function} ON hagency_agent_v1.management_outbox"
    ))
    .await
    .unwrap();
    assert!(failed.is_err());
    assert_eq!(
        f.domain
            .management_resource_grant(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .state,
        "active"
    );
    assert_eq!(allocation(&f, &p).await.state, "pending_coordinator");
    assert!(
        f.domain
            .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    close(&f, &g).await;
    let d = parent_delivery(&f).await;
    db.batch_execute(&format!("CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    let failed = f
        .domain
        .record_management_parent_result(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            parent_result(&d, &g, 12, 0, 0),
        )
        .await;
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(failed.is_err());
    assert_eq!(
        f.domain
            .management_parent_settlement(&f.owner.p, &f.policy, &g.id)
            .await
            .unwrap()
            .held_tokens,
        1_000_000
    );
    assert_eq!(report(&f, &d, &g, 12, 0, 0).await.settlement.held_tokens, 0);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn actual_parent_http_is_closed_and_unadvertised_even_with_test_gate_enabled() {
    let (f, _, _, g, _) = prepared_parent().await;
    let s = service(&f, true);
    let mut r = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.owner.token)
        .send(&s)
        .await;
    let session: Value = r.take_json().await.unwrap();
    assert!(!session.to_string().contains("parent_settlement"));
    assert!(!session.to_string().contains("parent-settlement"));
    post(
        &s,
        &format!("grants/{}/close", g.id),
        &f.outsider.token,
        json!({"commandId":"http_parent_wrong","expectedGrantRevision":g.revision}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    post(&s,&format!("grants/{}/close",g.id),&f.owner.token,json!({"commandId":"http_parent_injected","expectedGrantRevision":g.revision,"retainedTokens":0}),StatusCode::BAD_REQUEST).await;
    let closing = post(
        &s,
        &format!("grants/{}/close", g.id),
        &f.owner.token,
        json!({"commandId":"http_parent_close","expectedGrantRevision":g.revision}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(closing["settlement"]["knownUnusedReleasedTokens"], 0);
    let d = parent_delivery(&f).await;
    let body = serde_json::json!({"commandDigest":d.command_digest,"token":d.token,"receipt":parent_result(&d,&g,12,0,0).receipt});
    let done = post(
        &s,
        &format!("grant-executor-commands/{}/result", d.command.command_id),
        &f.owner.token,
        body,
        StatusCode::OK,
    )
    .await;
    assert_eq!(done["settlement"]["heldTokens"], 0);
    post(
        &s,
        &format!("grants/{}/renewals", g.id),
        &f.owner.token,
        json!({}),
        StatusCode::NOT_FOUND,
    )
    .await;
}
