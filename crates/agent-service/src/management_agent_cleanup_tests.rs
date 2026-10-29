use super::*;
use crate::management_agent_cleanup_types::*;
async fn get(service: &Service, path: &str, token: &str, status: StatusCode) -> Value {
    let mut response = Box::pin(
        TestClient::get(format!(
            "https://example.test/hagency/api/v1/management/{path}"
        ))
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .send(service),
    )
    .await;
    assert_eq!(response.status_code, Some(status));
    response.take_json().await.unwrap()
}
fn disabled(f: &Fixture) -> ManagementPolicy {
    f.policy
        .clone()
        .with_agent_admission(false)
        .with_project_setup(false)
        .with_project_reservations(false)
        .with_agent_executor(false)
        .with_agent_matrix_provisioning(false)
        .with_agent_runtime_provisioning(false)
        .with_shared_execution(false)
}
async fn removal(f: &Fixture, p: &Principal, id: &str) -> AgentRemovalProgress {
    f.domain.managed_agent_removal(p, id).await.unwrap()
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cleanup_disabled_issuance_manager_cancel_exact_recovery_no_new_authority() {
    let (mut f, p, s, admin, gw) = ready_agent().await;
    let r = request(&f, &p, &s, &gw).await;
    f.policy = disabled(&f);
    let before = removal(&f, &f.outsider.p, &r.id).await;
    assert!(before.can_cancel && !before.can_revoke && !before.central_fenced);
    assert!(before.native_fence.is_none() && before.usage.known_consumed_tokens.is_none());
    assert_eq!(
        f.domain
            .decide_managed_agent(
                &admin.p,
                &f.policy,
                &r.id,
                decide(&r, "disabled_approve"),
                &gw
            )
            .await
            .unwrap_err()
            .to_string(),
        "agent_admission_unavailable"
    );
    let cmd = RevokeInput {
        command_id: "cleanup_cancel".into(),
        expected_revision: r.revision,
    };
    let result = f
        .domain
        .cancel_managed_agent_request(&f.outsider.p, &f.policy, &r.id, cmd.clone())
        .await
        .unwrap();
    assert_eq!(result.state, "cancelled");
    let replay = f
        .domain
        .cancel_managed_agent_request(&f.outsider.p, &f.policy, &r.id, cmd.clone())
        .await
        .unwrap();
    assert_eq!(replay.revision, result.revision);
    let recovered = f
        .domain
        .recover_managed_agent_command(
            &f.outsider.p,
            &f.policy,
            &cmd.command_id,
            RecoverCommandInput {
                operation: "agent.cancel".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered["agentRequest"]["id"], r.id);
    assert!(recovered["recorded"].as_bool().unwrap());
    assert_eq!(
        f.domain
            .recover_managed_agent_command(
                &f.outsider.p,
                &f.policy,
                &cmd.command_id,
                RecoverCommandInput {
                    operation: "agent.request".into()
                }
            )
            .await
            .unwrap_err()
            .to_string(),
        "agent_admission_unavailable"
    );
    let after = removal(&f, &f.outsider.p, &r.id).await;
    assert!(after.central_fenced && !after.can_cancel && !after.can_revoke);
    assert!(
        after.native_fence.is_none()
            && after.process_observation.is_none()
            && after.matrix.is_none()
    );
    let bad = RevokeInput {
        expected_revision: r.revision + 1,
        ..cmd
    };
    assert_eq!(
        f.domain
            .cancel_managed_agent_request(&f.outsider.p, &f.policy, &r.id, bad)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
    let service = service(&f, true);
    let response = get(&service, "session", &f.outsider.token, StatusCode::OK).await;
    let features = response["features"].as_array().unwrap();
    assert!(
        features.contains(&json!(AGENT_CLEANUP_FEATURE))
            && !features.contains(&json!("agent-admission-v1"))
    );
    let detail = get(
        &service,
        &format!("agent-requests/{}/removal", r.id),
        &f.outsider.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(detail["removal"]["admissionState"], "cancelled");
    assert!(detail["observedAtMs"].as_i64().is_some());
    let index = post(
        &service,
        "agent-removals/list",
        &f.outsider.token,
        json!({"projectId":p.id,"limit":1}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(index["requests"]["items"][0]["requestId"], r.id);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cleanup_current_admin_only_disabled_policy_unknown_hold_and_membership_separate() {
    let (mut f, r, admin) = approved().await;
    let d = poll(&f).await;
    submit(&f, &d, reservation(&d)).await.unwrap();
    let original = f.policy.clone();
    f.policy = disabled(&f);
    let cmd = RevokeInput {
        command_id: "disabled_revoke".into(),
        expected_revision: r.revision,
    };
    for actor in [&f.owner.p, &f.admin.p, &f.coordinator.p] {
        assert_eq!(
            f.domain
                .revoke_managed_agent_request(actor, &f.policy, &r.id, cmd.clone())
                .await
                .unwrap_err()
                .to_string(),
            "assigned_project_administrator_required"
        );
    }
    for actor in [&f.owner.p, &f.admin.p, &f.coordinator.p] {
        assert_eq!(
            f.domain
                .managed_agent_removal(actor, &r.id)
                .await
                .unwrap_err()
                .to_string(),
            "agent_request_not_found"
        );
        assert!(
            f.domain
                .managed_agent_removals(actor, AgentRemovalQuery::default())
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    assert!(removal(&f, &admin.p, &r.id).await.can_revoke);
    let wrong = RevokeInput {
        command_id: "wrong_revoke".into(),
        expected_revision: r.revision + 1,
    };
    assert_eq!(
        f.domain
            .revoke_managed_agent_request(&admin.p, &f.policy, &r.id, wrong)
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let result = f
        .domain
        .revoke_managed_agent_request(&admin.p, &f.policy, &r.id, cmd.clone())
        .await
        .unwrap();
    assert_eq!(result.state, "revoked");
    let current = removal(&f, &admin.p, &r.id).await;
    assert!(current.central_fenced && !current.can_revoke);
    assert_eq!(current.native_fence.unwrap().delivery_state, "pending");
    assert_eq!(
        current.usage.retained_tokens_floor,
        Some(r.requested_tokens)
    );
    let recovered = f
        .domain
        .recover_managed_agent_command(
            &admin.p,
            &f.policy,
            &cmd.command_id,
            RecoverCommandInput {
                operation: "agent.revoke".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered["agentRequest"]["state"], "revoked");
    // Explicitly restore only this isolated fixture's executor policy to deliver
    // the original cleanup; the human restriction route did not enable it.
    f.policy = original;
    let fence = poll(&f).await;
    submit(&f, &fence, fence_result(&fence, true))
        .await
        .unwrap();
    let after = removal(&f, &f.outsider.p, &r.id).await;
    let native = after.native_fence.unwrap();
    assert_eq!(native.delivery_state, "recorded");
    assert_eq!(native.process_state.as_deref(), Some("unknown"));
    assert_eq!(after.usage.known_consumed_tokens, Some(0));
    assert_eq!(after.usage.retained_tokens_floor, Some(r.requested_tokens));
    assert_eq!(after.usage.fence_usage_complete, Some(false));
    assert!(after.process_observation.is_none());
    assert!(
        after
            .matrix
            .as_ref()
            .is_none_or(|m| m.cleanup_observed_at_ms.is_none())
    );
    let wire = serde_json::to_string(&after.usage).unwrap();
    assert!(!wire.contains("released") && !wire.contains("available"));
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cleanup_projection_paging_manager_authority_and_inactive_manager() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let r = request(&f, &p, &s, &gw).await;
    let r = f
        .domain
        .decide_managed_agent(
            &admin.p,
            &f.policy,
            &r.id,
            decide(&r, "cleanup_approve"),
            &gw,
        )
        .await
        .unwrap();
    for n in 0..2 {
        f.domain
            .request_managed_agent(
                &f.outsider.p,
                &f.policy,
                &p.id,
                input(&f, &p, &s, &format!("cleanup_paging_{n}")).await,
                &gw,
            )
            .await
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = f
            .domain
            .managed_agent_removals(
                &admin.p,
                AgentRemovalQuery {
                    project_id: Some(p.id.clone()),
                    after,
                    limit: Some(1),
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        seen.push(page.items[0].request_id.clone());
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 3);
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
    assert!(
        f.domain
            .managed_agent_removals(
                &admin.p,
                AgentRemovalQuery {
                    limit: Some(17),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let other = actor(&f.auth, "unrelated_cleanup_reader").await;
    assert!(f.domain.managed_agent_removal(&other.p, &r.id).await.is_err());
    invalidate_authority(&f, &p.id).await;
    assert!(removal(&f, &admin.p, &r.id).await.central_fenced);
    let (g, q, current) = approved().await;
    let mut db = AsyncPgConnection::establish(&g.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.users SET active=false WHERE id=$1").bind::<Text,_>(&g.outsider.p.user_id).execute(&mut db).await.unwrap();
    assert!(g.domain.managed_agent_removal(&current.p, &q.id).await.is_err());
    // Disabling the manager still fences execution and queues device cleanup.
    assert!(sql_query("SELECT state='fenced' AS matched FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&q.id).get_result::<Check>(&mut db).await.unwrap().matched);

}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cleanup_disabled_policy_atomic_failure_and_late_reservation_race_keep_one_fence() {
    let (f, r, _) = approved().await;
    let d = poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fail_disabled_cleanup_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.operation='agent.fence' THEN RAISE EXCEPTION 'rollback_fixture'; END IF; RETURN NEW; END $$;CREATE TRIGGER fail_disabled_cleanup_test BEFORE INSERT ON hagency_agent_v1.management_agent_executor_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_disabled_cleanup_test()").await.unwrap();
    let command = RevokeInput {
        command_id: "disabled_atomic_cancel".into(),
        expected_revision: r.revision,
    };
    let policy = disabled(&f);
    assert!(
        f.domain
            .cancel_managed_agent_request(&f.outsider.p, &policy, &r.id, command.clone())
            .await
            .is_err()
    );
    let before = removal(&f, &f.outsider.p, &r.id).await;
    assert_eq!(before.request_revision, r.revision);
    assert!(!before.central_fenced && before.native_fence.is_none());
    let recover = f
        .domain
        .recover_managed_agent_command(
            &f.outsider.p,
            &policy,
            &command.command_id,
            RecoverCommandInput {
                operation: "agent.cancel".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recover["recorded"], false);
    db.batch_execute("DROP TRIGGER fail_disabled_cleanup_test ON hagency_agent_v1.management_agent_executor_commands;DROP FUNCTION hagency_agent_v1.fail_disabled_cleanup_test()").await.unwrap();
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let result = reservation(&d);
    let (accepted, cancelled) = tokio::join!(
        f.domain
            .record_managed_agent_executor_result(&f.owner.p, &f.policy, id(&d), result),
        other.cancel_managed_agent_request(&f.outsider.p, &policy, &r.id, command.clone())
    );
    accepted.unwrap();
    cancelled.unwrap();
    let after = removal(&f, &f.outsider.p, &r.id).await;
    assert!(after.central_fenced && !after.can_cancel);
    assert_eq!(after.admission_state, "cancelled");
    assert_eq!(after.native_fence.unwrap().delivery_state, "pending");
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=$1 AND state='active') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1) AND (SELECT count(*) FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$2 AND w.operation='agent.fence')=1 AS matched").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
    let replay = other
        .cancel_managed_agent_request(&f.outsider.p, &policy, &r.id, command)
        .await
        .unwrap();
    assert_eq!(replay.revision, after.request_revision);
}
