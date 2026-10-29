use crate::{
    api::{App, now_ms},
    domain::DomainStore,
    gateway::Gateway,
    identity::{Identity, Verifier},
    management_types::*,
    secret_token,
    store::{Principal, RegisterDevice, Store},
};
use diesel::{
    sql_query,
    sql_types::{BigInt, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use salvo::{
    Service,
    http::StatusCode,
    test::{ResponseExt, TestClient},
};
use serde_json::{Value, json};
use std::sync::Arc;
use url::Url;
#[path = "management_allocation_tests.rs"]
mod allocations;
#[path = "management_project_tests.rs"]
mod projects;
struct Actor {
    session: String,
    token: String,
    p: Principal,
}
async fn same_actor(auth: &Store, a: &Actor) -> Actor {
    Actor { session: a.session.clone(), token: a.token.clone(), p: auth.authenticate(&a.token, now_ms(), true).await.unwrap() }
}
// The capacity coordinator is the owner, signed in on a second device.
// This keeps account authority separate from custody of the resource executor.
async fn owner_coordinator_device(auth: &Store, owner: &Actor) -> Actor {
    let session = owner.session.clone();
    let token = auth.register_device(&session, RegisterDevice {
        installation_id: "owner_management_device".into(),
        name: "Owner management device".into(),
    }, now_ms()).await.unwrap().token;
    let p = auth.authenticate(&token, now_ms(), true).await.unwrap();
    Actor { session, token, p }
}
struct Fixture {
    auth: Store,
    domain: DomainStore,
    url: String,
    policy: ManagementPolicy,
    owner: Actor,
    admin: Actor,
    coordinator: Actor,
    outsider: Actor,
}
async fn actor(auth: &Store, name: &str) -> Actor {
    let suffix = secret_token();
    let session = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: suffix.clone(),
                mxid: format!("@{name}_{suffix}:example.test"),
                client_id: "management-test".into(),
                valid_until_ms: now_ms() + 1_000_000,
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let token = auth
        .register_device(
            &session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "MacBook".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let p = auth.authenticate(&token, now_ms(), true).await.unwrap();
    Actor { session, token, p }
}
async fn fixture() -> Fixture {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test DB required");
    fixture_at(url).await
}
async fn fixture_at(url: String) -> Fixture {
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let owner = actor(&auth, "contributor").await;
    let admin = actor(&auth, "administrator").await;
    let coordinator = owner_coordinator_device(&auth, &owner).await;
    let outsider = actor(&auth, "manager").await;
    let policy = ManagementPolicy::new(admin.p.mxid.clone(), "example.test").unwrap();
    Fixture {
        auth,
        domain,
        url,
        policy,
        owner,
        admin,
        coordinator,
        outsider,
    }
}
fn offer(f: &Fixture, id: &str) -> ContributionInput {
    ContributionInput {
        command_id: format!("offer_{id}"),
        resource_id: id.into(),
        resource_revision: "a".repeat(64),
        label: "MacBook coding resource".into(),
        runtime: Runtime::Codex,
        capabilities: ["coding".into(), "documentation".into()].into(),
        coordinator_mxid: f.coordinator.p.mxid.clone(),
    }
}
fn approve(key: &str, revision: i64) -> DecisionInput {
    DecisionInput {
        command_id: key.into(),
        expected_revision: revision,
        decision: Decision::Approve,
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn connection_verification_is_executor_scoped_expires_and_replay_never_renews_it() {
    let f = fixture().await;
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "verify_resource"))
        .await
        .unwrap();
    let mut input = VerifyConnectionInput {
        command_id: "verify_once".into(),
        expected_revision: 1,
        resource_id: a.contribution.resource_id.clone(),
        resource_revision: a.contribution.resource_revision.clone(),
    };
    assert_eq!(
        f.domain
            .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
            .await
            .unwrap_err()
            .to_string(),
        "contribution_not_admitted"
    );
    let a = f
        .domain
        .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("admit", 1))
        .await
        .unwrap();
    assert!(a.contribution.verification.is_none());
    input.expected_revision = a.contribution.revision;
    for other in [&f.admin.p, &f.outsider.p] {
        assert_eq!(
            f.domain
                .verify_contribution(other, &f.policy, &a.contribution.id, input.clone())
                .await
                .unwrap_err()
                .to_string(),
            "contribution_executor_required"
        );
    }
    let other_token = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "second_device".into(),
                name: "Second Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let other = f
        .auth
        .authenticate(&other_token, now_ms(), true)
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .verify_contribution(&other, &f.policy, &a.contribution.id, input.clone())
            .await
            .unwrap_err()
            .to_string(),
        "contribution_executor_required"
    );
    let mut changed = input.clone();
    changed.resource_revision = "b".repeat(64);
    assert_eq!(
        f.domain
            .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, changed)
            .await
            .unwrap_err()
            .to_string(),
        "contribution_resource_changed"
    );
    let verified = f
        .domain
        .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(verified.contribution.revision, 3);
    let proof = verified.contribution.verification.unwrap();
    assert_eq!(proof.status, "verified");
    let restarted = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let replay = restarted
        .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    assert_eq!(
        replay.contribution.verification.unwrap().verified_at_ms,
        proof.verified_at_ms
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_verifications SET verified_at_ms=$2,fresh_until_ms=$3 WHERE contribution_id=$1")
        .bind::<Text,_>(&a.contribution.id).bind::<BigInt,_>(now_ms()-2000).bind::<BigInt,_>(now_ms()-1000).execute(&mut db).await.unwrap();
    restarted
        .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    let stale = restarted
        .contribution(&f.owner.p, &f.policy, &a.contribution.id)
        .await
        .unwrap();
    assert_eq!(stale.contribution.verification.unwrap().status, "stale");
    input.command_id = "verify_again".into();
    input.expected_revision = 3;
    let fresh = restarted
        .verify_contribution(&f.owner.p, &f.policy, &a.contribution.id, input)
        .await
        .unwrap();
    assert_eq!(fresh.contribution.verification.unwrap().status, "verified");
    let revoked = restarted
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &a.contribution.id,
            RevokeInput {
                command_id: "withdraw_verified".into(),
                expected_revision: 4,
            },
        )
        .await
        .unwrap();
    assert_eq!(revoked.contribution.verification.unwrap().status, "stale");
    assert_eq!(revoked.contribution.state, "revoked");
    let count =
        sql_query("SELECT count(*) AS count FROM hagency_agent_v1.agents WHERE owner_user_id=$1")
            .bind::<Text, _>(&f.owner.p.user_id)
            .get_result::<Count>(&mut db)
            .await
            .unwrap();
    assert_eq!(
        count.count, 0,
        "connection verification cannot create an agent"
    );
}
fn pending() -> ManagementQuery {
    ManagementQuery {
        needs_my_action: true,
        ..Default::default()
    }
}
#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    count: i64,
}
async fn outbox(f: &Fixture, id: &str, state: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_outbox WHERE request_id=$1 AND state=$2")
        .bind::<Text,_>(id).bind::<Text,_>(state).get_result::<Count>(&mut db).await.unwrap().count
}
#[test]
fn closed_management_contract_rejects_authority_and_secret_injection() {
    let valid = json!({"commandId":"c1","resourceId":"r1","resourceRevision":"a".repeat(64),"label":"Coding","runtime":"codex","capabilities":["coding"],"coordinatorMxid":"@coord:example.test"});
    assert!(
        serde_json::from_value::<ContributionInput>(valid.clone())
            .unwrap()
            .validate("example.test")
            .is_ok()
    );
    for field in [
        "actor",
        "ownerMxid",
        "executorDeviceId",
        "deviceGeneration",
        "isAdmin",
        "token",
        "url",
        "workspace",
        "providerSecret",
    ] {
        let mut input = valid.clone();
        input[field] = json!("injected");
        assert!(
            serde_json::from_value::<ContributionInput>(input).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("resourceId", json!("../private")),
        ("runtime", json!("shell")),
        ("coordinatorMxid", json!("@coord:other.test")),
        ("resourceRevision", json!("A".repeat(64))),
        ("label", json!("\n")),
        ("capabilities", json!([])),
    ] {
        let mut input = valid.clone();
        input[field] = value;
        assert!(
            serde_json::from_value::<ContributionInput>(input)
                .map(|v| v.validate("example.test").is_err())
                .unwrap_or(true)
        );
    }
    assert!(ManagementPolicy::new("@admin:other.test".into(), "example.test").is_err());
    assert!(serde_json::from_value::<DecisionInput>(json!({"commandId":"c","expectedRevision":1,"decision":"approve","approverMxid":"@admin:example.test"})).is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_is_durable_private_and_decidable_only_by_designated_administrator() {
    let f = fixture().await;
    let admission = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
        .await
        .unwrap();
    assert_eq!(admission.contribution.state, "pending_admission");
    assert!(!admission.request.can_decide);
    assert_eq!(outbox(&f, &admission.request.id, "pending").await, 1);
    for actor in [&f.owner, &f.coordinator, &f.outsider] {
        assert!(
            f.domain
                .management_inbox(&actor.p, &f.policy, pending())
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert_eq!(
            f.domain
                .decide_management_request(
                    &actor.p,
                    &f.policy,
                    &admission.request.id,
                    approve("forged", 1)
                )
                .await
                .unwrap_err()
                .to_string(),
            "management_administrator_required"
        );
    }
    assert!(
        f.domain
            .contribution(&f.outsider.p, &f.policy, &admission.contribution.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .management_request(&f.outsider.p, &f.policy, &admission.request.id)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .contributions(&f.outsider.p, &f.policy, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    for _ in 0..2 {
        let inbox = f
            .domain
            .management_inbox(&f.admin.p, &f.policy, pending())
            .await
            .unwrap();
        assert_eq!(inbox.items.len(), 1);
        assert!(inbox.items[0].can_decide);
    }
    // Reading/opening the card has no acknowledgement or completion side effect.
    let result = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &admission.request.id,
            approve("accept", 1),
        )
        .await
        .unwrap();
    assert_eq!(result.contribution.state, "admitted");
    assert_eq!(result.request.state, "approved");
    assert!(!result.request.can_decide);
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, pending())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(outbox(&f, &admission.request.id, "cancelled").await, 1);
    assert_eq!(outbox(&f, &admission.request.id, "pending").await, 1);
    assert_eq!(
        f.domain
            .management_request(&f.owner.p, &f.policy, &admission.request.id)
            .await
            .unwrap()
            .state,
        "approved"
    );
    // Admission alone is neither a connection proof nor permission to execute.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let agents =
        sql_query("SELECT count(*) AS count FROM hagency_agent_v1.agents WHERE owner_user_id=$1")
            .bind::<Text, _>(&f.owner.p.user_id)
            .get_result::<Count>(&mut db)
            .await
            .unwrap();
    assert_eq!(agents.count, 0);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn concurrent_retries_restart_and_old_cards_do_not_duplicate_admissions_or_decisions() {
    let f = fixture().await;
    // Independent connections model different request workers/server instances.
    let second = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.domain
            .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1")),
        second.create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.contribution.id, b.contribution.id);
    assert_eq!(outbox(&f, &a.request.id, "pending").await, 1);
    let mut altered = offer(&f, "r1");
    altered.label = "Different resource".into();
    assert_eq!(
        f.domain
            .create_contribution(&f.owner.p, &f.policy, altered)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
    let (a, b) = tokio::join!(
        f.domain.decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("accept", 1)
        ),
        second.decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("accept", 1)
        )
    );
    let a = a.unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(b.unwrap()).unwrap()
    );
    let restart = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let replay = restart
        .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("accept", 1))
        .await
        .unwrap();
    assert_eq!(replay.request.revision, 2);
    assert_eq!(outbox(&f, &a.request.id, "pending").await, 1);
    assert_eq!(
        restart
            .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("another", 1))
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let revoke = RevokeInput {
        command_id: "withdraw".into(),
        expected_revision: 2,
    };
    f.domain
        .revoke_contribution(&f.owner.p, &f.policy, &a.contribution.id, revoke.clone())
        .await
        .unwrap();
    restart
        .revoke_contribution(&f.owner.p, &f.policy, &a.contribution.id, revoke)
        .await
        .unwrap();
    let old_card = restart
        .management_request(&f.admin.p, &f.policy, &a.request.id)
        .await
        .unwrap();
    assert_eq!(old_card.state, "revoked");
    assert_eq!(old_card.revision, 3);
    assert!(!old_card.can_decide);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn decision_compare_and_swap_rejects_conflicting_races() {
    let f = fixture().await;
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
        .await
        .unwrap();
    let second = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let reject = DecisionInput {
        command_id: "reject".into(),
        expected_revision: 1,
        decision: Decision::Reject,
    };
    let (approve, reject) = tokio::join!(
        f.domain.decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("accept", 1)
        ),
        second.decide_management_request(&f.admin.p, &f.policy, &a.request.id, reject)
    );
    assert_eq!(
        usize::from(approve.is_ok()) + usize::from(reject.is_ok()),
        1
    );
    let latest = f
        .domain
        .management_request(&f.admin.p, &f.policy, &a.request.id)
        .await
        .unwrap();
    assert_eq!(latest.revision, 2);
    assert!(matches!(latest.state.as_str(), "approved" | "rejected"));
    assert_eq!(outbox(&f, &a.request.id, "pending").await, 1);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn stale_executor_revoked_actor_and_expired_actions_cannot_be_approved() {
    let f = fixture().await;
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
        .await
        .unwrap();
    f.auth
        .revoke_device(
            &f.owner.session,
            f.owner.p.device_id.as_ref().unwrap(),
            now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .create_contribution(&f.owner.p, &f.policy, offer(&f, "r2"))
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("accept", 1))
            .await
            .unwrap_err()
            .to_string(),
        "contribution_executor_changed"
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_requests SET created_at_ms=$2,expires_at_ms=$3 WHERE id=$1").bind::<Text,_>(&a.request.id).bind::<BigInt,_>(now_ms()-2000).bind::<BigInt,_>(now_ms()-1000).execute(&mut db).await.unwrap();
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, pending())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let expired = f
        .domain
        .management_request(&f.admin.p, &f.policy, &a.request.id)
        .await
        .unwrap();
    assert_eq!(expired.state, "expired");
    assert!(!expired.can_decide);
    assert_eq!(
        f.domain
            .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("expire", 1))
            .await
            .unwrap_err()
            .to_string(),
        "management_action_closed"
    );
    f.auth.sign_out(&f.admin.session).await.unwrap();
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, pending())
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn failed_outbox_write_rolls_back_resource_request_and_receipt_together() {
    let f = fixture().await;
    let suffix = &secret_token()[..12];
    let function = format!("management_fail_{suffix}");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Target only this test's recipient; no production database is used.
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_outbox_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.admin.p.mxid)).await.unwrap();
    assert!(
        f.domain
            .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
            .await
            .is_err()
    );
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(
        f.domain
            .contributions(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.domain
            .management_inbox(&f.admin.p, &f.policy, pending())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "r1"))
        .await
        .unwrap();
    assert_eq!(outbox(&f, &a.request.id, "pending").await, 1);
}
fn service(f: &Fixture, enabled: bool) -> Service {
    let issuer = Url::parse("https://example.test/auth/").unwrap();
    let origin = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("introspect").unwrap(),
        origin.clone(),
        "unused-test-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let gateway = Gateway::new(
        Arc::new(|_| Box::pin(async { Ok(vec![]) })),
        "@_hagency_service:example.test".into(),
    );
    let app = App::new(
        f.auth.clone(),
        verifier,
        origin,
        issuer,
        "example.test".into(),
    )
    .with_domain(f.domain.clone(), gateway);
    Service::new(
        if enabled {
            app.with_management(f.policy.clone())
        } else {
            app
        }
        .router(),
    )
}
async fn post(s: &Service, path: &str, token: &str, body: Value, status: StatusCode) -> Value {
    let mut r = Box::pin(
        TestClient::post(format!(
            "https://example.test/hagency/api/v1/management/{path}"
        ))
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .json(&body)
        .send(s),
    )
    .await;
    let actual = r.status_code;
    let value: Value = r.take_json().await.unwrap();
    assert_eq!(
        actual,
        Some(status),
        "{path}: {}",
        value.get("code").unwrap_or(&Value::Null)
    );
    value
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn actual_api_requires_enrolled_device_exact_contract_and_explicit_server_configuration() {
    let f = fixture().await;
    let s = service(&f, true);
    post(
        &s,
        "inbox",
        &f.owner.session,
        json!({}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let mut input = serde_json::to_value(offer(&f, "r1")).unwrap();
    input["ownerMxid"] = json!(f.admin.p.mxid);
    post(
        &s,
        "contributions",
        &f.owner.token,
        input,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let result = post(
        &s,
        "contributions",
        &f.owner.token,
        serde_json::to_value(offer(&f, "r1")).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        result["admission"]["contribution"]["ownerMxid"],
        f.owner.p.mxid
    );
    let path = format!(
        "requests/{}/decide",
        result["admission"]["request"]["id"].as_str().unwrap()
    );
    post(
        &s,
        &path,
        &f.outsider.token,
        serde_json::to_value(approve("accept", 1)).unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    post(
        &s,
        &path,
        &f.admin.token,
        serde_json::to_value(approve("accept", 1)).unwrap(),
        StatusCode::OK,
    )
    .await;
    post(
        &s,
        "inbox?actor=admin",
        &f.owner.token,
        json!({}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post(
        &s,
        "unknown",
        &f.owner.token,
        json!({}),
        StatusCode::NOT_FOUND,
    )
    .await;
    post(
        &service(&f, false),
        "inbox",
        &f.admin.token,
        json!({}),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn bounded_pages_preserve_all_visible_records_without_leaking_other_owners() {
    let f = fixture().await;
    for i in 0..35 {
        f.domain
            .create_contribution(&f.owner.p, &f.policy, offer(&f, &format!("r{i}")))
            .await
            .unwrap();
    }
    let first = f
        .domain
        .contributions(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 32);
    assert!(first.next_after.is_some());
    let second = f
        .domain
        .contributions(
            &f.owner.p,
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
    let first = f
        .domain
        .management_inbox(&f.admin.p, &f.policy, pending())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 32);
    let second = f
        .domain
        .management_inbox(
            &f.admin.p,
            &f.policy,
            ManagementQuery {
                after: first.next_after,
                needs_my_action: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 3);
    assert!(
        f.domain
            .management_inbox(&f.outsider.p, &f.policy, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn recovery_reads_prior_generation_receipt_without_reviving_executor_authority() {
    let f = fixture().await;
    let input = offer(&f, "restart_resource");
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, input.clone())
        .await
        .unwrap();
    let a = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("restart_admit", 1),
        )
        .await
        .unwrap();
    let renewed = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Restarted Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let current = f
        .auth
        .authenticate(&renewed.token, now_ms(), true)
        .await
        .unwrap();
    assert!(current.device_generation > f.owner.p.device_generation);
    let recover = || RecoverCommandInput {
        operation: "contribution.create".into(),
    };
    assert!(
        f.domain
            .recover_management_command(&f.owner.p, &f.policy, &input.command_id, recover())
            .await
            .is_err(),
        "old authority remains fenced"
    );
    let replay = f
        .domain
        .create_contribution(&current, &f.policy, input.clone())
        .await
        .unwrap();
    assert_eq!(replay.contribution.id, a.contribution.id);
    let mut changed = input.clone();
    changed.resource_id = "different_resource".into();
    assert!(
        f.domain
            .create_contribution(&current, &f.policy, changed)
            .await
            .is_err(),
        "rotation does not permit reusing an ID with changed arguments"
    );
    let restored = f
        .domain
        .recover_management_command(&current, &f.policy, &input.command_id, recover())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.contribution.id, a.contribution.id);
    assert_eq!(
        restored.contribution.state, "admitted",
        "recover the latest state, not the old pending card"
    );
    assert_eq!(
        restored.contribution.executor_generation,
        f.owner.p.device_generation.unwrap(),
        "recovery cannot transfer execution authority"
    );
    assert!(
        f.domain
            .recover_management_command(&f.outsider.p, &f.policy, &input.command_id, recover())
            .await
            .unwrap()
            .is_none()
    );
    let other = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "other_mac".into(),
                name: "Other Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let other = f
        .auth
        .authenticate(&other.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .recover_management_command(&other, &f.policy, &input.command_id, recover())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .recover_management_command(
                &current,
                &f.policy,
                &input.command_id,
                RecoverCommandInput {
                    operation: "request.decide".into()
                }
            )
            .await
            .is_err()
    );
    let svc = service(&f, true);
    let body = post(
        &svc,
        &format!("commands/{}/recover", input.command_id),
        &renewed.token,
        serde_json::to_value(recover()).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["recorded"], true);
    assert_eq!(body["admission"]["contribution"]["id"], a.contribution.id);
    assert!(body["observedAtMs"].is_number());
    f.auth
        .revoke_device(
            &f.owner.session,
            current.device_id.as_ref().unwrap(),
            now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .recover_management_command(&current, &f.policy, &input.command_id, recover())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn explicit_executor_resume_preserves_admission_and_requires_a_new_verification() {
    let f = fixture().await;
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "resume_resource"))
        .await
        .unwrap();
    let grant = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Restarted Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&grant.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("stale", 1))
            .await
            .is_err()
    );
    let input = VerifyConnectionInput {
        command_id: "resume_first".into(),
        expected_revision: 1,
        resource_id: a.contribution.resource_id.clone(),
        resource_revision: a.contribution.resource_revision.clone(),
    };
    for other in [&f.outsider.p, &f.admin.p] {
        assert!(
            f.domain
                .resume_contribution(other, &f.policy, &a.contribution.id, input.clone())
                .await
                .is_err()
        );
    }
    let mut changed = input.clone();
    changed.resource_revision = "b".repeat(64);
    assert!(
        f.domain
            .resume_contribution(&p, &f.policy, &a.contribution.id, changed)
            .await
            .is_err()
    );
    let wire = post(
        &service(&f, true),
        &format!("contributions/{}/resume", a.contribution.id),
        &grant.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    let resumed: Admission = serde_json::from_value(wire["admission"].clone()).unwrap();
    assert_eq!(resumed.contribution.state, "pending_admission");
    assert_eq!(
        resumed.contribution.executor_generation,
        p.device_generation.unwrap()
    );
    assert_eq!(resumed.request.id, a.request.id);
    assert_eq!(resumed.request.revision, 2);
    assert!(resumed.contribution.verification.is_none());
    assert!(
        f.domain
            .decide_management_request(
                &f.admin.p,
                &f.policy,
                &a.request.id,
                approve("old_review", 1)
            )
            .await
            .is_err()
    );
    let admitted = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("current_review", 2),
        )
        .await
        .unwrap();
    let verified = f
        .domain
        .verify_contribution(
            &p,
            &f.policy,
            &a.contribution.id,
            VerifyConnectionInput {
                command_id: "verify_after_resume".into(),
                expected_revision: admitted.contribution.revision,
                ..input.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        verified.contribution.verification.as_ref().unwrap().status,
        "verified"
    );
    let grant = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Restart again".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let newest = f
        .auth
        .authenticate(&grant.token, now_ms(), true)
        .await
        .unwrap();
    // Replaying an old resume cannot transfer authority to a later generation.
    f.domain
        .resume_contribution(&newest, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    let latest = f
        .domain
        .contribution(&newest, &f.policy, &a.contribution.id)
        .await
        .unwrap();
    assert_eq!(
        latest.contribution.executor_generation,
        p.device_generation.unwrap()
    );
    let current = f
        .domain
        .resume_contribution(
            &newest,
            &f.policy,
            &a.contribution.id,
            VerifyConnectionInput {
                command_id: "resume_second".into(),
                expected_revision: latest.contribution.revision,
                ..input
            },
        )
        .await
        .unwrap();
    assert_eq!(
        current.contribution.executor_generation,
        newest.device_generation.unwrap()
    );
    assert_eq!(current.contribution.state, "admitted");
    assert_eq!(current.request.state, "approved");
    assert_eq!(
        current.contribution.verification.unwrap().status,
        "stale",
        "reconnection must not manufacture a fresh verification"
    );
    assert!(
        f.domain
            .contribution(&p, &f.policy, &a.contribution.id)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn reconnecting_an_expired_request_does_not_reopen_approval() {
    let f = fixture().await;
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "expired_resume"))
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_requests SET created_at_ms=$2,expires_at_ms=$3 WHERE id=$1").bind::<Text,_>(&a.request.id).bind::<BigInt,_>(now_ms()-2000).bind::<BigInt,_>(now_ms()-1000).execute(&mut db).await.unwrap();
    let grant = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Restarted Mac".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&grant.token, now_ms(), true)
        .await
        .unwrap();
    let input = VerifyConnectionInput {
        command_id: "expired_reconnect".into(),
        expected_revision: 1,
        resource_id: a.contribution.resource_id,
        resource_revision: a.contribution.resource_revision,
    };
    assert_eq!(
        f.domain
            .resume_contribution(&p, &f.policy, &a.contribution.id, input)
            .await
            .unwrap_err()
            .to_string(),
        "management_action_closed"
    );
    let latest = f
        .domain
        .contribution(&p, &f.policy, &a.contribution.id)
        .await
        .unwrap();
    assert_eq!(latest.request.state, "expired");
    assert_eq!(
        latest.contribution.executor_generation,
        f.owner.p.device_generation.unwrap()
    );
}
