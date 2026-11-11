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

/// The UTC month `offset` months from the database clock's current month.
async fn month(f: &Fixture, offset: i32) -> Period {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT to_char(m,'YYYY-MM') AS key,(extract(epoch FROM (m AT TIME ZONE 'UTC'))*1000)::bigint AS start_ms,(extract(epoch FROM ((m + interval '1 month') AT TIME ZONE 'UTC'))*1000)::bigint AS end_ms FROM (SELECT date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC') + make_interval(months => $1) AS m) AS t")
        .bind::<diesel::sql_types::Integer, _>(offset)
        .get_result::<Period>(&mut db)
        .await
        .unwrap()
}

fn for_month(mut input: PublishGrantInput, m: &Period) -> PublishGrantInput {
    input.receipt.period_key = m.key.clone();
    input.receipt.period_start_ms = m.start_ms;
    input.receipt.period_end_ms = m.end_ms;
    input.receipt.delegation_expires_at_ms = m.end_ms;
    input
}

/// An earlier month's grant on this admission, stored as the server stored it
/// then. Its expiry is that month's end, so it is history today.
async fn seed_earlier_month(
    f: &Fixture,
    a: &Admission,
    offset: i32,
    state: &str,
    ledger_revision: i64,
) -> NativeGrantReceipt {
    let m = month(f, offset).await;
    let mut receipt = for_month(v2_input(f, a).await, &m).receipt;
    receipt.ledger_revision = ledger_revision;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms,chain_root_id,period_start_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$1,$11)")
        .bind::<Text, _>(&receipt.grant_id)
        .bind::<Text, _>(&a.contribution.id)
        .bind::<Text, _>(&f.owner.p.user_id)
        .bind::<Text, _>(f.owner.p.device_id.as_ref().unwrap())
        .bind::<BigInt, _>(f.owner.p.device_generation.unwrap())
        .bind::<Text, _>(serde_json::to_string(&receipt).unwrap())
        .bind::<Text, _>(allocation_digest(&receipt).unwrap())
        .bind::<Text, _>(state)
        .bind::<Text, _>(&m.key)
        .bind::<BigInt, _>(m.end_ms)
        .bind::<BigInt, _>(m.start_ms)
        .execute(&mut db)
        .await
        .unwrap();
    receipt
}

#[derive(QueryableByName)]
struct BoundGrant {
    #[diesel(sql_type = Text)]
    pool_grant_id: String,
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn ended_month_admission_is_granted_again_once_per_month_and_reopens_the_pool() {
    // Pool Agent requests need their own database, as in the pool Agent tests:
    // schema downgrade tests cannot run beside a request without a plan.
    let mut f = super::pool_agents::isolated_fixture().await;
    f.policy = f.policy.with_project_reservations(false);
    assert!(
        !session(&f).await["features"]
            .as_array()
            .unwrap()
            .contains(&json!(POOL_MONTHLY_REGRANT_FEATURE))
    );
    f.policy = f
        .policy
        .with_project_reservations(true)
        .with_agent_admission(true);
    assert!(
        session(&f).await["features"]
            .as_array()
            .unwrap()
            .contains(&json!(POOL_MONTHLY_REGRANT_FEATURE))
    );
    let a = admitted(&f).await;
    // Two earlier months on this same admission: one revoked, one that ended.
    seed_earlier_month(&f, &a, -2, "revoked", 3).await;
    let ended = seed_earlier_month(&f, &a, -1, "active", 5).await;
    assert_eq!(
        f.domain
            .management_resource_grant(&f.owner.p, &f.policy, &ended.grant_id)
            .await
            .unwrap()
            .state,
        "expired"
    );
    // An ended month opens nothing until the owner grants this month.
    assert_eq!(
        f.domain
            .request_pool_agent(
                &f.outsider.p,
                &f.policy,
                &a.contribution.id,
                super::pool_agents::input(&a, "monthly_regrant_before"),
            )
            .await
            .unwrap_err()
            .to_string(),
        "pool_capacity_unavailable"
    );
    // No early scheduling: next month is refused while this month runs.
    let mut early = for_month(v2_input(&f, &a).await, &month(&f, 1).await);
    early.command_id = "publish_next_month_early".into();
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, early)
            .await
            .unwrap_err()
            .to_string(),
        "invalid_grant_period"
    );
    let input = v2_input(&f, &a).await;
    let grant = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(grant.state, "active");
    assert_eq!(
        grant.receipt.admission_request_id.as_ref(),
        Some(&a.request.id)
    );
    assert_eq!(grant.receipt.period_key, month(&f, 0).await.key);
    assert_ne!(grant.receipt.period_key, ended.period_key);
    // The exact retry replays; a second grant in the same month is refused.
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
            .await
            .unwrap()
            .id,
        grant.id
    );
    let mut second = v2_input(&f, &a).await;
    second.command_id = "publish_second_grant_this_month".into();
    second.receipt.ledger_revision = input.receipt.ledger_revision + 1;
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, second.clone())
            .await
            .unwrap_err()
            .to_string(),
        "resource_grant_already_exists"
    );
    // The pool binds new requests to this month's grant, not the ended one.
    let request = f
        .domain
        .request_pool_agent(
            &f.outsider.p,
            &f.policy,
            &a.contribution.id,
            super::pool_agents::input(&a, "monthly_regrant_after"),
        )
        .await
        .unwrap();
    assert_eq!(request.period_key, grant.receipt.period_key);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let bound = sql_query(
        "SELECT pool_grant_id FROM hagency_agent_v1.management_agent_requests WHERE id=$1",
    )
    .bind::<Text, _>(&request.id)
    .get_result::<BoundGrant>(&mut db)
    .await
    .unwrap();
    assert_eq!(bound.pool_grant_id, grant.id);
    // Revocation does not free this month for the admission either.
    f.domain
        .revoke_management_grant(
            &f.owner.p,
            &f.policy,
            &grant.id,
            RevokeInput {
                command_id: "revoke_monthly_regrant".into(),
                expected_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, second)
            .await
            .unwrap_err()
            .to_string(),
        "resource_grant_already_exists"
    );
}
