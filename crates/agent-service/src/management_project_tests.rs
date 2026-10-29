use super::*;
use diesel::{QueryableByName, sql_types::Bool};

async fn admitted(f: &Fixture, name: &str) -> Admission {
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(f, name))
        .await
        .unwrap();
    f.domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve(&format!("approve_{name}"), 1),
        )
        .await
        .unwrap()
}
fn plan(a: &Admission, command: &str) -> ProjectRequestInput {
    ProjectRequestInput {
        command_id: command.into(),
        contribution_id: a.contribution.id.clone(),
        name: "OctoSense development".into(),
        purpose: "Develop and document the project".into(),
        requested_tokens: 100_000,
    }
}
fn project_query(needs_my_action: bool) -> ManagementQuery {
    ManagementQuery {
        include_projects: true,
        needs_my_action,
        ..Default::default()
    }
}
#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    count: i64,
}
async fn count(f: &Fixture, sql: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(sql)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_intent_requires_admin_and_never_creates_execution_or_capacity() {
    let f = fixture().await;
    let pending = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "project_resource"))
        .await
        .unwrap();
    let input = plan(&pending, "request_project");
    assert_eq!(
        f.domain
            .request_management_project(&f.outsider.p, &f.policy, input.clone())
            .await
            .unwrap_err()
            .to_string(),
        "contribution_not_admitted"
    );
    let a = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &pending.request.id,
            approve("admit_project_resource", 1),
        )
        .await
        .unwrap();
    let domain_before=count(&f,"SELECT (SELECT count(*) FROM hagency_agent_v1.projects)+(SELECT count(*) FROM hagency_agent_v1.agents)+(SELECT count(*) FROM hagency_agent_v1.bindings) AS count").await;
    let p = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, input.clone())
        .await
        .unwrap();
    assert_eq!(p.state, "pending_approval");
    assert_eq!(p.allocation_status, "awaiting_approval");
    assert!(p.can_revoke);
    assert!(!p.request.can_decide);
    assert_eq!(p.request.project_id.as_deref(), Some(p.id.as_str()));
    for user in [&f.owner.p, &f.coordinator.p, &f.outsider.p] {
        assert_eq!(
            f.domain
                .decide_management_project(
                    user,
                    &f.policy,
                    &p.request.id,
                    approve("project_decision", 1)
                )
                .await
                .unwrap_err()
                .to_string(),
            "management_administrator_required"
        );
        let view = f
            .domain
            .management_project(user, &f.policy, &p.id)
            .await
            .unwrap();
        assert!(!view.request.can_decide);
    }
    let stranger = actor(&f.auth, "stranger").await;
    assert!(
        f.domain
            .management_project(&stranger.p, &f.policy, &p.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_projects(&stranger.p, &f.policy, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let admin_view = f
        .domain
        .management_project(&f.admin.p, &f.policy, &p.id)
        .await
        .unwrap();
    assert!(admin_view.request.can_decide);
    assert!(admin_view.can_revoke);
    assert_eq!(outbox(&f, &p.request.id, "pending").await, 1);
    assert_eq!(
        f.domain
            .decide_management_request(
                &f.admin.p,
                &f.policy,
                &p.request.id,
                approve("wrong_resource_route", 1)
            )
            .await
            .unwrap_err()
            .to_string(),
        "management_operation_mismatch"
    );
    assert_eq!(
        f.domain
            .decide_management_project(
                &f.admin.p,
                &f.policy,
                &a.request.id,
                approve("wrong_project_route", 2)
            )
            .await
            .unwrap_err()
            .to_string(),
        "management_operation_mismatch"
    );
    let accepted = f
        .domain
        .decide_management_project(
            &f.admin.p,
            &f.policy,
            &p.request.id,
            approve("project_decision", 1),
        )
        .await
        .unwrap();
    assert_eq!(accepted.state, "approved");
    assert_eq!(accepted.allocation_status, "awaiting_allocation");
    assert!(!accepted.request.can_decide);
    assert_eq!(accepted.revision, 2);
    assert_eq!(outbox(&f, &p.request.id, "cancelled").await, 1);
    assert_eq!(outbox(&f, &p.request.id, "pending").await, 2);
    assert_eq!(domain_before,count(&f,"SELECT (SELECT count(*) FROM hagency_agent_v1.projects)+(SELECT count(*) FROM hagency_agent_v1.agents)+(SELECT count(*) FROM hagency_agent_v1.bindings) AS count").await);
    // Retrying a previously saved create returns the latest state, not pending.
    let replay = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, input.clone())
        .await
        .unwrap();
    assert_eq!(replay.id, p.id);
    assert_eq!(replay.state, "approved");
    let mut changed = input;
    changed.requested_tokens += 1;
    assert_eq!(
        f.domain
            .request_management_project(&f.outsider.p, &f.policy, changed)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_catalog_filters_public_metadata_and_never_exposes_executor_identifiers() {
    let f = fixture().await;
    let a = admitted(&f, "catalog_resource").await;
    let pending = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "catalog_pending"))
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Give this fixture a unique capability so pagination isn't influenced by
    // other fixtures in the dedicated database.
    let cap = format!("catalog.{}", a.contribution.id);
    sql_query("UPDATE hagency_agent_v1.management_contributions SET capabilities=$2 WHERE id=$1")
        .bind::<Text, _>(&a.contribution.id)
        .bind::<Text, _>(serde_json::to_string(&[&cap]).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    let result = f
        .domain
        .management_catalog(
            &f.outsider.p,
            CatalogQuery {
                capability: Some(cap.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let c = result
        .items
        .iter()
        .find(|c| c.id == a.contribution.id)
        .unwrap();
    assert!(result.items.iter().all(|c| c.id != pending.contribution.id));
    let value = serde_json::to_value(c).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 6);
    for hidden in [
        "ownerMxid",
        "executorDeviceId",
        "executorGeneration",
        "resourceId",
        "resourceRevision",
        "tokens",
        "availableTokens",
    ] {
        assert!(value.get(hidden).is_none(), "{hidden}");
    }
    let result = f
        .domain
        .management_catalog(
            &f.outsider.p,
            CatalogQuery {
                capability: Some("missing-capability".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.items.is_empty());
    assert!(
        f.domain
            .management_catalog(
                &f.outsider.p,
                CatalogQuery {
                    capability: Some("Coding OR true".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let page = f
        .domain
        .management_catalog(
            &f.outsider.p,
            CatalogQuery {
                after: Some(a.contribution.id.clone()),
                capability: Some(cap),
            },
        )
        .await
        .unwrap();
    assert!(page.items.is_empty());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn resource_withdrawal_closes_all_plans_and_recovery_returns_current_result() {
    let f = fixture().await;
    let a = admitted(&f, "withdraw_resource").await;
    let p = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "pending_plan"))
        .await
        .unwrap();
    let q = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "approved_plan"))
        .await
        .unwrap();
    f.domain
        .decide_management_project(
            &f.admin.p,
            &f.policy,
            &q.request.id,
            approve("approve_plan", 1),
        )
        .await
        .unwrap();
    let r = f
        .domain
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &a.contribution.id,
            RevokeInput {
                command_id: "withdraw".into(),
                expected_revision: a.contribution.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.contribution.state, "revoked");
    for id in [&p.id, &q.id] {
        let current = f
            .domain
            .management_project(&f.outsider.p, &f.policy, id)
            .await
            .unwrap();
        assert_eq!(current.state, "revoked");
        assert_eq!(current.allocation_status, "resource_unavailable");
        assert_eq!(current.request.state, "revoked");
        assert!(!current.can_revoke);
        assert!(!current.request.can_decide);
    }
    let recovery = f
        .domain
        .recover_management_project_command(
            &f.outsider.p,
            &f.policy,
            "pending_plan",
            RecoverCommandInput {
                operation: "project.request".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovery.state, "revoked");
    let action = f
        .domain
        .management_inbox(&f.admin.p, &f.policy, project_query(true))
        .await
        .unwrap();
    assert!(
        action
            .items
            .iter()
            .all(|r| r.id != p.request.id && r.id != q.request.id)
    );
    assert_eq!(outbox(&f, &p.request.id, "cancelled").await, 1);
    assert_eq!(outbox(&f, &p.request.id, "pending").await, 3);
    assert!(
        f.domain
            .decide_management_project(
                &f.admin.p,
                &f.policy,
                &p.request.id,
                approve("late_approve", 2)
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_decisions_are_revision_checked_expire_and_are_concurrency_safe() {
    let f = fixture().await;
    let a = admitted(&f, "concurrent_resource").await;
    let p = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "race_plan"))
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .decide_management_project(
                &f.admin.p,
                &f.policy,
                &p.request.id,
                approve("bad_revision", 2)
            )
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        f.domain.decide_management_project(
            &f.admin.p,
            &f.policy,
            &p.request.id,
            approve("race_left", 1)
        ),
        other.decide_management_project(
            &f.admin.p,
            &f.policy,
            &p.request.id,
            DecisionInput {
                command_id: "race_right".into(),
                expected_revision: 1,
                decision: Decision::Reject
            }
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let closed = f
        .domain
        .management_project(&f.admin.p, &f.policy, &p.id)
        .await
        .unwrap();
    assert_eq!(closed.revision, 2);
    assert!(!closed.request.can_decide);
    let q = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "expired_plan"))
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_requests SET created_at_ms=$2,expires_at_ms=$3 WHERE id=$1").bind::<Text,_>(&q.request.id).bind::<BigInt,_>(now_ms()-5000).bind::<BigInt,_>(now_ms()-1000).execute(&mut db).await.unwrap();
    let expired = f
        .domain
        .management_project(&f.admin.p, &f.policy, &q.id)
        .await
        .unwrap();
    assert_eq!(expired.state, "expired");
    assert_eq!(expired.allocation_status, "none");
    assert!(!expired.request.can_decide);
    assert!(!expired.can_revoke);
    assert_eq!(
        f.domain
            .decide_management_project(&f.admin.p, &f.policy, &q.request.id, approve("late", 1))
            .await
            .unwrap_err()
            .to_string(),
        "management_action_closed"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_recovery_survives_reauthorization_but_stays_bound_to_installation() {
    let f = fixture().await;
    let a = admitted(&f, "recovery_resource").await;
    let p = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "durable_plan"))
        .await
        .unwrap();
    let grant = f
        .auth
        .register_device(
            &f.outsider.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "MacBook".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let current = f
        .auth
        .authenticate(&grant.token, now_ms(), true)
        .await
        .unwrap();
    assert!(current.device_generation > f.outsider.p.device_generation);
    assert!(
        f.domain
            .management_project(&f.outsider.p, &f.policy, &p.id)
            .await
            .is_err()
    );
    let recovery = f
        .domain
        .recover_management_project_command(
            &current,
            &f.policy,
            "durable_plan",
            RecoverCommandInput {
                operation: "project.request".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovery.id, p.id);
    let foreign = f
        .auth
        .register_device(
            &f.outsider.session,
            RegisterDevice {
                installation_id: "different_macbook".into(),
                name: "Other Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let foreign = f
        .auth
        .authenticate(&foreign.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .recover_management_project_command(
                &foreign,
                &f.policy,
                "durable_plan",
                RecoverCommandInput {
                    operation: "project.request".into()
                }
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .recover_management_project_command(
                &current,
                &f.policy,
                "durable_plan",
                RecoverCommandInput {
                    operation: "project.decide".into()
                }
            )
            .await
            .is_err()
    );
    for user in [&f.coordinator.p, &f.owner.p] {
        assert_eq!(
            f.domain
                .revoke_management_project(
                    user,
                    &f.policy,
                    &p.id,
                    RevokeInput {
                        command_id: "deny_revoke".into(),
                        expected_revision: 1
                    }
                )
                .await
                .unwrap_err()
                .to_string(),
            "project_manager_required"
        );
    }
    let revoked = f
        .domain
        .revoke_management_project(
            &current,
            &f.policy,
            &p.id,
            RevokeInput {
                command_id: "cancel_plan".into(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(revoked.state, "revoked");
    assert_eq!(revoked.allocation_status, "none");
    let latest = f
        .domain
        .recover_management_project_command(
            &current,
            &f.policy,
            "durable_plan",
            RecoverCommandInput {
                operation: "project.request".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.state, "revoked");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_inbox_is_opt_in_and_pages_preserve_role_visibility() {
    let f = fixture().await;
    let a = admitted(&f, "pagination_resource").await;
    for i in 0..35 {
        f.domain
            .request_management_project(&f.outsider.p, &f.policy, plan(&a, &format!("page_{i}")))
            .await
            .unwrap();
    }
    let legacy = f
        .domain
        .management_inbox(&f.admin.p, &f.policy, Default::default())
        .await
        .unwrap();
    assert!(legacy.items.iter().all(|r| r.kind == "resource_admission"));
    let pending = f
        .domain
        .management_inbox(&f.admin.p, &f.policy, project_query(true))
        .await
        .unwrap();
    assert_eq!(pending.items.len(), 32);
    assert!(
        pending
            .items
            .iter()
            .all(|r| r.kind == "project_capacity" && r.can_decide)
    );
    for user in [&f.owner.p, &f.coordinator.p, &f.outsider.p] {
        let first = f
            .domain
            .management_projects(user, &f.policy, Default::default())
            .await
            .unwrap();
        assert_eq!(first.items.len(), 32);
        let second = f
            .domain
            .management_projects(
                user,
                &f.policy,
                ManagementQuery {
                    after: first.next_after,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(second.items.len(), 3);
        assert!(second.next_after.is_none());
        assert!(first.items.last().unwrap().id < second.items[0].id);
        let inbox = f
            .domain
            .management_inbox(user, &f.policy, project_query(false))
            .await
            .unwrap();
        assert!(inbox.items.iter().any(|r| r.kind == "project_capacity"));
        assert!(
            f.domain
                .management_projects(user, &f.policy, project_query(true))
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_creation_is_removed_while_historical_access_remains() {
    let f = fixture().await;
    let a = admitted(&f, "http_project_resource").await;
    let s = service(&f, true);
    let mut response = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.admin.token)
        .send(&s)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let session: Value = response.take_json().await.unwrap();
    assert_eq!(
        session["features"],
        json!([
            "resource-owner-coordinator-v1",
            crate::management_agent_cleanup_types::AGENT_CLEANUP_FEATURE,
            crate::management_agent_settlement_types::AGENT_SETTLEMENT_FEATURE,
            crate::management_shared_intake_types::SHARED_RUN_CONTROL_FEATURE
        ])
    );
    assert_eq!(session["canApproveProjects"], false);
    post(
        &s,
        "projects",
        &f.outsider.session,
        serde_json::to_value(plan(&a, "http_plan")).unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let mut forged = serde_json::to_value(plan(&a, "http_plan")).unwrap();
    forged["managerMxid"] = json!(f.admin.p.mxid);
    post(
        &s,
        "projects",
        &f.outsider.token,
        forged,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let mut invalid = plan(&a, "http_plan");
    invalid.requested_tokens = 9_007_199_254_740_992;
    post(
        &s,
        "projects",
        &f.outsider.token,
        serde_json::to_value(invalid).unwrap(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let removed = post(
        &s,
        "projects",
        &f.outsider.token,
        serde_json::to_value(plan(&a, "http_plan")).unwrap(),
        StatusCode::BAD_REQUEST,
    ).await;
    assert!(removed.to_string().contains("projects_removed_use_resource_pools"));
    // Seed a historical plan through the retained internal reader fixture.
    // The public API must not issue a new project.
    let historical = f.domain.request_management_project(&f.outsider.p, &f.policy, plan(&a, "http_plan")).await.unwrap();
    let id = historical.id.as_str();
    let request = historical.request.id.as_str();
    post(
        &s,
        &format!("project-requests/{request}/decide"),
        &f.coordinator.token,
        json!({"commandId":"http_decide","expectedRevision":1,"decision":"approve"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let approved = post(
        &s,
        &format!("project-requests/{request}/decide"),
        &f.admin.token,
        json!({"commandId":"http_decide","expectedRevision":1,"decision":"approve"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        approved["project"]["allocationStatus"],
        "awaiting_allocation"
    );
    let latest = post(
        &s,
        "project-commands/http_plan/recover",
        &f.outsider.token,
        json!({"operation":"project.request"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(latest["recorded"], true);
    assert_eq!(latest["project"]["state"], "approved");
    let revoked = post(
        &s,
        &format!("projects/{id}/revoke"),
        &f.outsider.token,
        json!({"commandId":"http_cancel","expectedRevision":2}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(revoked["project"]["state"], "revoked");
    post(
        &s,
        "catalog",
        &f.outsider.token,
        json!({"capability":"coding"}),
        StatusCode::OK,
    )
    .await;
    post(
        &s,
        "projects/list",
        &f.outsider.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    post(
        &s,
        "projects?actor=admin",
        &f.outsider.token,
        json!({}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    // The project schema version is actually installed, not only advertised.
    #[derive(QueryableByName)]
    struct Schema {
        #[diesel(sql_type=Bool)]
        matched: bool,
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT version=28 AS matched FROM hagency_agent_v1.management_deployment WHERE singleton").get_result::<Schema>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn escaped_project_pages_fit_native_transport_without_skipping_boundary_record() {
    let f = fixture().await;
    let a = admitted(&f, "large_project_resource").await;
    let mut expected = std::collections::BTreeSet::new();
    for i in 0..35 {
        let mut input = plan(&a, &format!("escaped_{i}"));
        input.purpose = "\"".repeat(2000);
        input.name = "\"".repeat(160);
        expected.insert(
            f.domain
                .request_management_project(&f.outsider.p, &f.policy, input)
                .await
                .unwrap()
                .id,
        );
    }
    let mut after = None;
    let mut observed = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = f
            .domain
            .management_projects(
                &f.outsider.p,
                &f.policy,
                ManagementQuery {
                    after,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 96 * 1024);
        assert!(!page.items.is_empty());
        if pages == 0 {
            assert!(
                page.items.len() < 32,
                "test must exercise byte limit rather than item limit"
            );
        }
        for project in &page.items {
            assert!(
                observed.insert(project.id.clone()),
                "repeated boundary item"
            );
            assert_eq!(
                project.request.project_name.as_deref(),
                Some(project.name.as_str())
            );
            assert_eq!(
                project.request.resource_label.as_deref(),
                Some(project.resource_label.as_str())
            );
        }
        pages += 1;
        assert!(pages < 5);
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(observed, expected);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn project_outbox_failure_rolls_back_plan_request_and_receipt_as_one_transaction() {
    let f = fixture().await;
    let a = admitted(&f, "atomic_project_resource").await;
    let function = format!("project_fail_{}", &secret_token()[..12]);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_project_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.admin.p.mxid)).await.unwrap();
    let failed = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "atomic_plan"))
        .await;
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(failed.is_err());
    assert!(
        f.domain
            .management_projects(&f.outsider.p, &f.policy, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, project_query(true))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.domain
            .recover_management_project_command(
                &f.outsider.p,
                &f.policy,
                "atomic_plan",
                RecoverCommandInput {
                    operation: "project.request".into()
                }
            )
            .await
            .unwrap()
            .is_none()
    );
    let applied = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "atomic_plan"))
        .await
        .unwrap();
    assert_eq!(outbox(&f, &applied.request.id, "pending").await, 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn concurrent_resource_withdrawal_and_project_approval_cannot_leave_live_plan() {
    let f = fixture().await;
    let a = admitted(&f, "withdraw_race_resource").await;
    let p = f
        .domain
        .request_management_project(&f.outsider.p, &f.policy, plan(&a, "withdraw_race_plan"))
        .await
        .unwrap();
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (withdraw, decision) = tokio::join!(
        f.domain.revoke_contribution(
            &f.owner.p,
            &f.policy,
            &a.contribution.id,
            RevokeInput {
                command_id: "race_withdraw".into(),
                expected_revision: a.contribution.revision
            }
        ),
        other.decide_management_project(
            &f.admin.p,
            &f.policy,
            &p.request.id,
            approve("race_accept", 1)
        )
    );
    withdraw.unwrap();
    if let Ok(accepted) = decision {
        assert_eq!(accepted.state, "approved");
    }
    let latest = f
        .domain
        .management_project(&f.admin.p, &f.policy, &p.id)
        .await
        .unwrap();
    assert_eq!(latest.state, "revoked");
    assert_eq!(latest.allocation_status, "resource_unavailable");
    assert!(!latest.request.can_decide);
    let request = f
        .domain
        .management_request(&f.admin.p, &f.policy, &p.request.id)
        .await
        .unwrap();
    assert!(!request.can_decide);
    assert_eq!(request.state, "revoked");
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, project_query(true))
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn management_v2_upgrade_preserves_admission_verification_and_legacy_command() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_project_upgrade_{}",
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
        let auth=Store::open(url.as_str(),"example.test","https://example.test/auth/").await.unwrap();
        let owner=actor(&auth,"legacy_owner").await;
        let admin=actor(&auth,"legacy_admin").await;
        let policy=ManagementPolicy::new(admin.p.mxid.clone(),"example.test").unwrap();
        let now=now_ms();
        let mut db=AsyncPgConnection::establish(url.as_str()).await.unwrap();
        db.batch_execute(include_str!("management_schema.sql")).await.unwrap();
        db.batch_execute(include_str!("management_verification_schema.sql")).await.unwrap();
        db.transaction::<_,diesel::result::Error,_>(async |db:&mut AsyncPgConnection| {
            sql_query("INSERT INTO hagency_agent_v1.management_contributions(id,owner_user_id,executor_device_id,executor_generation,resource_id,resource_revision,label,runtime,capabilities,coordinator_mxid,state,revision,request_id,created_at_ms,updated_at_ms) VALUES('legacy_contribution',$1,$2,$3,'legacy_resource',$4,'Legacy MacBook','codex','[\"coding\"]',$5,'admitted',2,'legacy_request',$6,$6)")
                .bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(owner.p.device_id.as_ref().unwrap()).bind::<BigInt,_>(owner.p.device_generation.unwrap()).bind::<Text,_>("a".repeat(64)).bind::<Text,_>(&owner.p.mxid).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) VALUES('legacy_request','resource_admission','legacy_contribution',$1,$2,'approved',2,$3,$4,$3)")
                .bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&admin.p.mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(now+600000).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_verifications(contribution_id,contribution_revision,executor_generation,resource_revision,verified_at_ms,fresh_until_ms) VALUES('legacy_contribution',2,$1,$2,$3,$4)")
                .bind::<BigInt,_>(owner.p.device_generation.unwrap()).bind::<Text,_>("a".repeat(64)).bind::<BigInt,_>(now).bind::<BigInt,_>(now+300000).execute(db).await?;
            // Receipt deliberately has the v2 wire shape, without project fields.
            let saved=json!({"contribution":{"id":"legacy_contribution","ownerMxid":owner.p.mxid,"executorDeviceId":owner.p.device_id,"executorGeneration":owner.p.device_generation,"resourceId":"legacy_resource","resourceRevision":"a".repeat(64),"label":"Legacy MacBook","runtime":"codex","capabilities":["coding"],"coordinatorMxid":owner.p.mxid,"state":"admitted","revision":2,"requestId":"legacy_request","createdAtMs":now,"updatedAtMs":now},"request":{"id":"legacy_request","kind":"resource_admission","contributionId":"legacy_contribution","requesterMxid":owner.p.mxid,"approverMxid":admin.p.mxid,"state":"approved","revision":2,"createdAtMs":now,"expiresAtMs":now+600000,"decidedAtMs":now,"canDecide":false}});
            sql_query("INSERT INTO hagency_agent_v1.management_commands(actor_user_id,device_id,device_generation,command_id,operation,digest,result) VALUES($1,$2,$3,'legacy_command','contribution.create','legacy_fixture',$4)")
                .bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(owner.p.device_id.as_ref().unwrap()).bind::<BigInt,_>(owner.p.device_generation.unwrap()).bind::<Text,_>(saved.to_string()).execute(db).await?;
            Ok(())
        }).await.unwrap();
        let domain=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();
        let latest=domain.recover_management_command(&owner.p,&policy,"legacy_command",RecoverCommandInput{operation:"contribution.create".into()}).await.unwrap().unwrap();
        assert_eq!(latest.contribution.id,"legacy_contribution");assert_eq!(latest.contribution.state,"admitted");assert_eq!(latest.contribution.revision,2);assert_eq!(latest.contribution.verification.as_ref().unwrap().status,"verified");assert_eq!(latest.request.project_id,None);assert_eq!(latest.request.resource_label.as_deref(),Some("Legacy MacBook"));
        let p=domain.request_management_project(&owner.p,&policy,plan(&latest,"post_upgrade_project")).await.unwrap();assert_eq!(p.state,"pending_approval");
        // A second startup is idempotent and retains both old and new records.
        let again=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();assert_eq!(again.management_project(&owner.p,&policy,&p.id).await.unwrap().id,p.id);
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}
