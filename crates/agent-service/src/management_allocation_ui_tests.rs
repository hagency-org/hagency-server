//! Human read contracts use real PostgreSQL projections, never a local-capacity estimate.
use super::*;

fn query(after: Option<String>, limit: u16) -> ProjectAllocationsQuery {
    ProjectAllocationsQuery {
        after,
        limit: Some(limit),
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn allocation_ui_exact_detail_keeps_request_revision_and_redacts_private_parent() {
    let (f, _, p, g, a) = prepared().await;
    for person in [&f.owner.p, &f.outsider.p, &f.coordinator.p, &f.admin.p] {
        let detail = f
            .domain
            .management_allocation_detail(person, &f.policy, &a.id)
            .await
            .unwrap();
        assert_eq!(detail.project_name, p.name);
        assert_eq!(detail.request.allocation_id.as_deref(), Some(a.id.as_str()));
        assert_eq!(
            detail.request.can_decide,
            person.mxid == f.coordinator.p.mxid
        );
        assert_eq!(detail.grant.total_tokens, g.receipt.allocated_tokens);
        assert!(detail.grant.native_available_tokens.is_none());
        let safe = serde_json::to_value(&detail).unwrap();
        for key in [
            "receipt",
            "token",
            "enrollmentId",
            "executorDeviceId",
            "eligibleManagers",
        ] {
            assert!(safe["grant"].get(key).is_none(), "{key}");
        }
    }
    accepted(&f, &a).await;
    let d = delivered(&f).await;
    record(&f, &d, "reserved", 11).await;
    // The two counters are independent protocol facts. Seed an allocation-only
    // revision advance to prove the read never substitutes it for the request CAS.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_allocations SET revision=revision+5 WHERE id=$1")
        .bind::<Text, _>(&a.id)
        .execute(&mut db)
        .await
        .unwrap();

    let detail = f
        .domain
        .management_allocation_detail(&f.coordinator.p, &f.policy, &a.id)
        .await
        .unwrap();
    assert_eq!(detail.request.revision, 3);
    assert!(detail.allocation.revision > detail.request.revision);
    assert_eq!(
        detail.allocation.balance.as_ref().unwrap().retained_tokens,
        a.requested_tokens
    );
    assert!(!detail.request.can_decide);
    assert!(detail.grant.native_available_tokens.is_none());
    let stranger = actor(&f.auth, "unrelated_allocation_reader").await;
    assert!(
        f.domain
            .management_allocation_detail(&stranger.p, &f.policy, &a.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_project_allocations(&stranger.p, &f.policy, &p.id, query(None, 1))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn allocation_ui_history_pages_exact_ids_and_keeps_owner_authority() {
    let (f, admission, p, grant, original) = prepared().await;
    f.domain
        .revoke_management_grant(
            &f.owner.p,
            &f.policy,
            &grant.id,
            RevokeInput {
                command_id: "ui_revoke_original".into(),
                expected_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    let mut input = grant_input(&f, &admission, &f.outsider.p).await;
    input.command_id = "ui_publish_replacement".into();
    input.receipt.ledger_revision = 20;
    let replacement = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &admission.contribution.id, input)
        .await
        .unwrap();
    let first = f
        .domain
        .management_project_allocations(&f.coordinator.p, &f.policy, &p.id, query(None, 1))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let next = first.next_after.clone().unwrap();
    let second = f
        .domain
        .management_project_allocations(
            &f.coordinator.p,
            &f.policy,
            &p.id,
            query(Some(next.clone()), 1),
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_after.is_none());
    assert!(second.items[0].allocation.id > next);
    let ids = [
        first.items[0].allocation.id.clone(),
        second.items[0].allocation.id.clone(),
    ];
    assert!(ids.contains(&original.id));
    assert!(
        first.items[0].grant.id == replacement.id || second.items[0].grant.id == replacement.id
    );
    let exact = f
        .domain
        .management_allocation_detail(&f.coordinator.p, &f.policy, &original.id)
        .await
        .unwrap();
    assert_eq!(exact.grant.id, grant.id);
    assert_eq!(exact.grant.state, "revoked");
    assert!(!exact.allocation.can_decide);

    // A separate coordinator cannot replace the owner or acquire historical access.
    let newer = actor(&f.auth, "new_allocation_coordinator").await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query(
        "UPDATE hagency_agent_v1.management_contributions SET coordinator_mxid=$1 WHERE id=$2",
    )
    .bind::<Text, _>(&newer.p.mxid)
    .bind::<Text, _>(&admission.contribution.id)
    .execute(&mut db)
    .await
    .is_err());
    let history = f
        .domain
        .management_project_allocations(&f.coordinator.p, &f.policy, &p.id, query(None, 16))
        .await
        .unwrap();
    assert_eq!(history.items.len(), 2);
    assert!(!history.items.iter().find(|x| x.allocation.id == original.id).unwrap().allocation.can_decide);
    assert!(
        f.domain
            .management_allocation_detail(&newer.p, &f.policy, &original.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_project_allocations(&newer.p, &f.policy, &p.id, query(None, 16))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn allocation_ui_actual_http_is_closed_bounded_and_policy_scoped() {
    let (f, _, p, _, a) = prepared().await;
    let s = service(&f, true);
    let mut session = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.coordinator.token)
        .send(&s)
        .await;
    let session: Value = session.take_json().await.unwrap();
    assert!(
        session["features"]
            .as_array()
            .unwrap()
            .contains(&json!(ALLOCATION_REVIEW_FEATURE))
    );

    let mut response = TestClient::get(format!(
        "https://example.test/hagency/api/v1/management/allocations/{}",
        a.id
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(&f.coordinator.token)
    .send(&s)
    .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let reply: Value = response.take_json().await.unwrap();
    assert_eq!(reply["detail"]["allocation"]["id"], a.id);
    assert!(reply["observedAtMs"].as_i64().unwrap() > 0);
    let path = format!("projects/{}/allocations/query", p.id);
    let page = post(
        &s,
        &path,
        &f.outsider.token,
        json!({"limit":16}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(page["allocations"]["items"].as_array().unwrap().len(), 1);
    for body in [
        json!({"limit":0}),
        json!({"limit":17}),
        json!({"limit":16,"ownerMxid":f.owner.p.mxid}),
    ] {
        post(
            &s,
            &path,
            &f.coordinator.token,
            body,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let disabled = f.policy.clone().with_project_reservations(false);
    assert!(
        f.domain
            .management_allocation_detail(&f.owner.p, &disabled, &a.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_project_allocations(&f.owner.p, &disabled, &p.id, query(None, 16))
            .await
            .is_err()
    );
}
