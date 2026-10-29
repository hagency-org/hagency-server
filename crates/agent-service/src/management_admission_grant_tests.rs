use super::*;

async fn v2_input(f: &Fixture, a: &Admission) -> PublishGrantInput {
    let mut input = grant_input(f, a, &f.outsider.p).await;
    input.receipt.version = 2;
    input.receipt.admission_request_id = Some(a.request.id.clone());
    input
}

async fn session(f: &Fixture) -> Value {
    let mut response = TestClient::get("https://example.test/hagency/api/v1/management/session")
        .add_header("host", "example.test", true)
        .bearer_auth(&f.owner.token)
        .send(&service(f, true))
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    response.take_json().await.unwrap()
}

#[test]
fn v2_native_canonical_vectors_and_version_binding_are_exact() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-allocation-v2.json"
    ))
    .unwrap();
    let grant: NativeGrantReceipt = serde_json::from_value(fixture["grant"].clone()).unwrap();
    grant.validate("example.test").unwrap();
    let command: AllocationCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
    assert_eq!(allocation_digest(&grant).unwrap(), fixture["grantDigest"]);
    assert_eq!(
        allocation_digest(&command).unwrap(),
        fixture["commandDigest"]
    );
    for (version, admission) in [
        (1, Some("req_fixture")),
        (2, None),
        (2, Some("")),
        (3, Some("req_fixture")),
    ] {
        let mut invalid = grant.clone();
        invalid.version = version;
        invalid.admission_request_id = admission.map(String::from);
        assert!(invalid.validate("example.test").is_err());
    }
    let mut injected = fixture["grant"].clone();
    injected["admissionApproved"] = json!(true);
    assert!(serde_json::from_value::<NativeGrantReceipt>(injected).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn first_v2_grant_is_discoverable_and_creates_owner_capacity_approval_over_http() {
    let mut f = fixture().await;
    assert!(
        !session(&f).await["features"]
            .as_array()
            .unwrap()
            .contains(&json!(ADMISSION_BOUND_GRANTS_FEATURE))
    );
    f.policy = f.policy.with_project_reservations(true);
    let features = session(&f).await["features"].as_array().unwrap().clone();
    assert!(features.contains(&json!(ADMISSION_BOUND_GRANTS_FEATURE)));
    assert!(!features.contains(&json!(ALLOCATION_FEATURE)));
    let a = admitted(&f).await;
    let project = approved_plan(&f, &a, &f.outsider.p).await;
    assert!(
        f.domain
            .management_project_allocation(&f.outsider.p, &f.policy, &project.id)
            .await
            .unwrap()
            .is_none()
    );
    let input = v2_input(&f, &a).await;
    let s = service(&f, true);
    let path = format!("contributions/{}/grants", a.contribution.id);
    let grant = post(
        &s,
        &path,
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        grant["grant"]["receipt"]["admissionRequestId"],
        a.request.id
    );
    assert_eq!(
        grant["grant"]["receiptDigest"],
        allocation_digest(&input.receipt).unwrap()
    );
    let replay = post(
        &s,
        &path,
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay["grant"]["id"], grant["grant"]["id"]);
    let allocation = allocation(&f, &project).await;
    assert_eq!(allocation.state, "pending_coordinator");
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
    post(
        &s,
        &format!("allocation-requests/{}/decide", allocation.request_id),
        &f.coordinator.token,
        json!({"commandId":"approve_v2_allocation","expectedRevision":1,"decision":"approve"}),
        StatusCode::OK,
    )
    .await;
    let delivery = delivered(&f).await;
    assert!(delivery.execution_allowed);
    assert_eq!(
        delivery.command.grant_receipt.admission_request_id,
        Some(a.request.id)
    );
    assert_eq!(delivery.command.grant_receipt.version, 2);
    record(&f, &delivery, "reserved", 11).await;
    assert_eq!(super::allocation(&f, &project).await.state, "reserved");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v2_grant_rejects_wrong_admission_and_non_executor_before_creating_capacity() {
    let f = enabled_fixture().await;
    let a = admitted(&f).await;
    let input = v2_input(&f, &a).await;
    let mut invalid = input.clone();
    invalid.receipt.admission_request_id = Some("req_wrong_admission".into());
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, invalid)
            .await
            .unwrap_err()
            .to_string(),
        "resource_grant_binding_mismatch"
    );
    for actor in [&f.coordinator.p, &f.admin.p, &f.outsider.p] {
        assert!(
            f.domain
                .publish_management_grant(actor, &f.policy, &a.contribution.id, input.clone())
                .await
                .is_err()
        );
    }
    assert!(
        !f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn revoked_v2_admission_cannot_issue_another_grant_but_exact_retry_recovers() {
    let f = enabled_fixture().await;
    let a = admitted(&f).await;
    let input = v2_input(&f, &a).await;
    let grant = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    let revoked = f
        .domain
        .revoke_management_grant(
            &f.owner.p,
            &f.policy,
            &grant.id,
            RevokeInput {
                command_id: "revoke_v2_grant".into(),
                expected_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    assert_ne!(revoked.state, "active");
    let recovered = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(recovered.id, grant.id);
    assert_eq!(recovered.state, revoked.state);
    let mut replacement = input;
    replacement.command_id = "publish_v2_replacement".into();
    replacement.receipt.grant_id = "mgr_v2_replacement".into();
    replacement.receipt.ledger_revision += 1;
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, replacement)
            .await
            .unwrap_err()
            .to_string(),
        "resource_grant_already_exists"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn changed_admission_fences_v2_grant_readiness() {
    let f = enabled_fixture().await;
    let a = admitted(&f).await;
    let input = v2_input(&f, &a).await;
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input)
        .await
        .unwrap();
    assert!(
        f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Model a future admission replacement without mutating the original receipt.
    sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) SELECT 'req_replacement',kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms FROM hagency_agent_v1.management_requests WHERE id=$1")
        .bind::<Text,_>(&a.request.id).execute(&mut db).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_contributions SET request_id=$2 WHERE id=$1")
        .bind::<Text, _>(&a.contribution.id)
        .bind::<Text, _>("req_replacement")
        .execute(&mut db)
        .await
        .unwrap();
    assert!(
        !f.domain
            .management_allocation_supported(&f.owner.p, &f.policy)
            .await
            .unwrap()
    );
}
