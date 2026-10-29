use crate::{
    api::{App, now_ms},
    configuration_types::*,
    domain::{CreateAgent, DomainStore, SetExecutionDevice},
    gateway::Gateway,
    identity::{Identity, Verifier},
    secret_token,
    store::{Principal, RegisterDevice, Store},
};
use diesel::{
    sql_query,
    sql_types::{BigInt, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use salvo::{
    Service,
    http::StatusCode,
    test::{ResponseExt, TestClient},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use url::Url;

struct Fixture {
    auth: Store,
    domain: DomainStore,
    session: String,
    a: String,
    b: String,
    pa: Principal,
    pb: Principal,
    agent: String,
    url: String,
}
async fn fixture() -> Fixture {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test DB required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    crate::transport::TransportStore::open(&url, crate::transport::Limits::default())
        .await
        .unwrap();
    let suffix = secret_token();
    let session = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: suffix.clone(),
                mxid: format!("@config_{suffix}:example.test"),
                client_id: "config-test".into(),
                valid_until_ms: now_ms() + 1_000_000,
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let a = auth
        .register_device(
            &session,
            RegisterDevice {
                installation_id: "device-a".into(),
                name: "Execution A".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let b = auth
        .register_device(
            &session,
            RegisterDevice {
                installation_id: "device-b".into(),
                name: "Display B".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let pa = auth.authenticate(&a, now_ms(), true).await.unwrap();
    let pb = auth.authenticate(&b, now_ms(), true).await.unwrap();
    let agent = domain
        .create_agent(
            &pa,
            CreateAgent {
                display_name: "Cross-device".into(),
                idempotency_key: "agent".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .id;
    Fixture {
        auth,
        domain,
        session,
        a,
        b,
        pa,
        pb,
        agent,
        url,
    }
}
fn input(op: ConfigurationOperation, key: &str, value: Value) -> ConfigurationInput {
    ConfigurationInput {
        idempotency_key: key.into(),
        expected_generation: 1,
        operation: op,
        input: value,
    }
}
fn config(agent: &str) -> Value {
    json!({"agentId":agent,"policy":{"revision":0,"policy":{"budget":{"limit":"Unlimited","period":"Lifetime"},"requests":"Allow","high_risk":"Deny"}},"usage":{"spent":0,"held":0},"modelProfile":null})
}
fn provider() -> Value {
    json!({"runtime":"codex","installed":true,"version":"1.0","versionSupported":true,"state":"ready","authenticated":true,"accountVerified":true,"credentialRef":"codex-managed:shared-home:abc","credentialSource":"local_codex","shared":true,"strictTokenCap":false,"nativeTools":true,"models":[],"defaultModel":null})
}
fn success(result: Value) -> ConfigurationComplete {
    ConfigurationComplete {
        state: "succeeded".into(),
        result: Some(result),
        error_code: None,
    }
}
async fn heartbeat(f: &Fixture) {
    assert!(
        f.domain
            .poll_configuration(&f.pa, ConfigurationPoll { accept_work: false }, now_ms())
            .await
            .unwrap()
            .is_none()
    );
}
async fn claim(f: &Fixture) -> ConfigurationRequest {
    f.domain
        .poll_configuration(&f.pa, ConfigurationPoll { accept_work: true }, now_ms())
        .await
        .unwrap()
        .unwrap()
}
async fn expire(f: &Fixture, id: &str) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.configuration_requests SET expires_at_ms=$2 WHERE id=$1")
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(now_ms() - 1)
        .execute(&mut db)
        .await
        .unwrap();
}
fn service(f: &Fixture) -> Service {
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
    Service::new(
        App::new(
            f.auth.clone(),
            verifier,
            origin,
            issuer,
            "example.test".into(),
        )
        .with_domain(f.domain.clone(), gateway)
        .router(),
    )
}
async fn get(s: &Service, path: &str, token: &str, status: StatusCode) -> Value {
    let mut r = TestClient::get(format!("https://example.test/hagency/api/v1/{path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .send(s)
        .await;
    assert_eq!(r.status_code, Some(status), "{path}");
    r.take_json().await.unwrap()
}
async fn post(s: &Service, path: &str, token: &str, body: Value, status: StatusCode) -> Value {
    let mut r = TestClient::post(format!("https://example.test/hagency/api/v1/{path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .json(&body)
        .send(s)
        .await;
    assert_eq!(r.status_code, Some(status), "{path}");
    r.take_json().await.unwrap()
}

#[test]
fn strict_non_secret_dto_rejects_provider_and_profile_injection() {
    for missing in [
        json!({"state":"failed","errorCode":"invalid_arguments"}),
        json!({"state":"succeeded","result":{}}),
        json!({"state":"failed"}),
    ] {
        assert!(serde_json::from_value::<ConfigurationComplete>(missing).is_err());
    }
    assert!(
        serde_json::from_value::<ConfigurationComplete>(
            json!({"state":"failed","result":null,"errorCode":"invalid_arguments"})
        )
        .is_ok()
    );
    let op = ConfigurationOperation::ProviderStatus;
    let p = provider();
    assert!(validate_result(&op, &json!({"runtime":"codex"}), "a", p.clone()).is_ok());
    for secret in [
        "authUrl",
        "accessToken",
        "credentialStore",
        "rawConfig",
        "stderr",
    ] {
        let mut malicious = p.clone();
        malicious[secret] = json!("sentinel-secret");
        assert!(validate_result(&op, &json!({"runtime":"codex"}), "a", malicious).is_err());
    }
    assert!(validate_result(&op, &json!({"runtime":"claude"}), "a", p.clone()).is_err());
    let mut too_big = p;
    too_big["models"] = json!(vec![json!({}); 101]);
    assert!(validate_result(&op, &json!({"runtime":"codex"}), "a", too_big).is_err());
    assert!(
        validate_input(
            &ConfigurationOperation::ProviderConnect,
            json!({"runtime":"codex","action":"logout"})
        )
        .is_err()
    );
    assert!(
        validate_input(
            &ConfigurationOperation::AgentPolicy,
            json!({"path":"/etc/passwd"})
        )
        .is_err()
    );
    assert!(!failure_code("provider said bearer secret"));
    let malicious = json!({"profile":{"model":"m","credential_ref":"Bearer secret","workspace_root":"/a","reasoning_effort":"medium"}});
    assert!(validate_input(&ConfigurationOperation::SaveAgentModel, malicious).is_err());
    let mut policy = config("a");
    policy["policy"]["policy"]["budget"]["secret"] = json!("no");
    assert!(validate_result(&ConfigurationOperation::AgentModel, &json!({}), "a", policy).is_err());
}
#[test]
fn model_configuration_digest_preserves_old_requests_and_checked_preconditions() {
    let legacy = json!({"profile": {
        "model": "gpt-6.1-sol", "credential_ref": "codex-managed:account",
        "workspace_root": "/workspace", "reasoning_effort": "low", "web_search": false
    }});
    let op = ConfigurationOperation::SaveAgentModel;
    assert_eq!(validate_input(&op, legacy.clone()).unwrap(), legacy);

    let mut checked = legacy.clone();
    checked["expectedProfileDigest"] = json!("ab12".repeat(16));
    assert_eq!(validate_input(&op, checked.clone()).unwrap(), checked);

    for invalid in [
        Value::Null,
        json!(42),
        json!(""),
        json!("a".repeat(63)),
        json!("a".repeat(65)),
        json!("A".repeat(64)),
        json!("g".repeat(64)),
    ] {
        let mut input = legacy.clone();
        input["expectedProfileDigest"] = invalid;
        assert!(validate_input(&op, input).is_err());
    }
    let mut wrong_name = legacy;
    wrong_name["expected_profile_digest"] = json!("ab12".repeat(16));
    assert!(validate_input(&op, wrong_name).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_http_two_devices_claim_and_complete_are_idempotent() {
    let f = fixture().await;
    let s = service(&f);
    let path = format!("agents/{}/configuration", f.agent);
    let status = get(&s, &path, &f.b, StatusCode::OK).await;
    assert_eq!(status["configuration"]["online"], false);
    assert_eq!(
        status["configuration"]["executionDeviceId"],
        f.pa.device_id.as_deref().unwrap()
    );
    assert_eq!(status["configuration"]["environment"], Value::Null);
    get(&s, &path, &f.session, StatusCode::UNAUTHORIZED).await;
    post(
        &s,
        "device-configuration/poll",
        &f.a,
        json!({"acceptWork":false}),
        StatusCode::OK,
    )
    .await;
    let requests = format!("{path}/requests");
    let payload = json!({"idempotencyKey":"environment","expectedGeneration":1,"operation":"environment","input":{}});
    let r = post(&s, &requests, &f.b, payload.clone(), StatusCode::OK).await;
    let id = r["request"]["id"].as_str().unwrap();
    assert_eq!(r["request"]["state"], "pending");
    let again = post(&s, &requests, &f.b, payload.clone(), StatusCode::OK).await;
    assert_eq!(again, r);
    let mut conflict = payload;
    conflict["operation"] = json!("agent_model");
    post(&s, &requests, &f.b, conflict, StatusCode::CONFLICT).await;
    assert_eq!(
        post(
            &s,
            "device-configuration/poll",
            &f.b,
            json!({"acceptWork":true}),
            StatusCode::OK
        )
        .await["request"],
        Value::Null
    );
    let claimed = post(
        &s,
        "device-configuration/poll",
        &f.a,
        json!({"acceptWork":true}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(claimed["request"]["id"], id);
    assert_eq!(claimed["request"]["state"], "claimed");
    assert_eq!(
        post(
            &s,
            "device-configuration/poll",
            &f.a,
            json!({"acceptWork":true}),
            StatusCode::OK
        )
        .await["request"],
        Value::Null
    );
    let complete = format!("device-configuration/requests/{id}/complete");
    let result = json!({"providers":[provider()],"configuration":config(&f.agent)});
    let body = json!({"state":"succeeded","result":result,"errorCode":null});
    post(&s, &complete, &f.b, body.clone(), StatusCode::UNAUTHORIZED).await;
    let mut secret = body.clone();
    secret["result"]["providers"][0]["authUrl"] = json!("https://example.test/secret");
    post(&s, &complete, &f.a, secret, StatusCode::BAD_REQUEST).await;
    let done = post(&s, &complete, &f.a, body.clone(), StatusCode::OK).await;
    assert_eq!(done["request"]["state"], "succeeded");
    assert_eq!(
        post(&s, &complete, &f.a, body, StatusCode::OK).await,
        done,
        "lost completion response must not reapply"
    );
    post(
        &s,
        &complete,
        &f.a,
        json!({"state":"failed","result":null,"errorCode":"configuration_model_unavailable"}),
        StatusCode::CONFLICT,
    )
    .await;
    let read = get(&s, &format!("{requests}/{id}"), &f.b, StatusCode::OK).await;
    assert_eq!(read, done);
    let status = get(&s, &path, &f.b, StatusCode::OK).await;
    assert_eq!(status["configuration"]["online"], true);
    assert_eq!(status["configuration"]["cached"], false);
    assert!(status["configuration"]["observedAtMs"].is_number());
    let foreign = fixture().await;
    get(&s, &path, &foreign.b, StatusCode::UNAUTHORIZED).await;
    get(
        &s,
        &format!("{requests}/{id}"),
        &foreign.b,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    post(
        &s,
        "device-configuration/poll",
        &f.a,
        json!({"acceptWork":false,"extra":1}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post(&s,&requests,&f.b,json!({"idempotencyKey":"unsafe","expectedGeneration":1,"operation":"provider_login","input":{}}),StatusCode::BAD_REQUEST).await;
    assert!(
        get(&s, "discovery", &f.b, StatusCode::OK).await["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "execution-configuration-v1")
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_expiry_no_redelivery_limits_and_offline_write() {
    let f = fixture().await;
    assert!(
        f.domain
            .create_configuration_request(
                &f.pb,
                &f.agent,
                input(
                    ConfigurationOperation::ProviderConnect,
                    "offline",
                    json!({"runtime":"codex"})
                ),
                now_ms()
            )
            .await
            .is_err()
    );
    heartbeat(&f).await;
    let r = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(
                ConfigurationOperation::ProviderConnect,
                "mutation",
                json!({"runtime":"codex"}),
            ),
            now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .create_configuration_request(
                &f.pb,
                &f.agent,
                input(
                    ConfigurationOperation::SaveAgentPolicy,
                    "busy",
                    json!({"expectedRevision":0,"policy":config(&f.agent)["policy"]["policy"]})
                ),
                now_ms()
            )
            .await
            .is_err()
    );
    assert_eq!(claim(&f).await.id, r.id);
    expire(&f, &r.id).await;
    assert_eq!(
        f.domain
            .configuration_request(&f.pb, &f.agent, &r.id, now_ms())
            .await
            .unwrap()
            .state,
        "unknown"
    );
    assert!(
        f.domain
            .poll_configuration(&f.pa, ConfigurationPoll { accept_work: true }, now_ms())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .complete_configuration(&f.pa, &r.id, success(provider()), now_ms())
            .await
            .is_err()
    );
    assert_eq!(
        f.domain
            .create_configuration_request(
                &f.pb,
                &f.agent,
                input(
                    ConfigurationOperation::ProviderConnect,
                    "mutation",
                    json!({"runtime":"codex"})
                ),
                now_ms()
            )
            .await
            .unwrap()
            .id,
        r.id
    );
    let pending = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "pending", json!({})),
            now_ms(),
        )
        .await
        .unwrap();
    expire(&f, &pending.id).await;
    assert_eq!(
        f.domain
            .configuration_request(&f.pb, &f.agent, &pending.id, now_ms())
            .await
            .unwrap()
            .state,
        "expired"
    );
    for n in 0..16 {
        f.domain
            .create_configuration_request(
                &f.pb,
                &f.agent,
                input(
                    ConfigurationOperation::AgentModel,
                    &format!("read-{n}"),
                    json!({}),
                ),
                now_ms(),
            )
            .await
            .unwrap();
    }
    assert!(
        f.domain
            .create_configuration_request(
                &f.pb,
                &f.agent,
                input(ConfigurationOperation::AgentModel, "full", json!({})),
                now_ms()
            )
            .await
            .is_err()
    );
    let first = claim(&f).await;
    assert_eq!(first.input, json!({}));
    assert!(
        f.domain
            .poll_configuration(&f.pa, ConfigurationPoll { accept_work: true }, now_ms())
            .await
            .unwrap()
            .is_none()
    );
    f.domain
        .complete_configuration(&f.pa, &first.id, success(config(&f.agent)), now_ms())
        .await
        .unwrap();
    assert_ne!(claim(&f).await.id, first.id);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_reassignment_and_requester_target_revocation_fence_results() {
    let f = fixture().await;
    heartbeat(&f).await;
    let first = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "stale", json!({})),
            now_ms(),
        )
        .await
        .unwrap();
    claim(&f).await;
    f.domain
        .set_execution_device(
            &f.pb,
            &f.agent,
            SetExecutionDevice {
                expected_generation: 1,
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .configuration_request(&f.pb, &f.agent, &first.id, now_ms())
            .await
            .unwrap()
            .state,
        "unknown"
    );
    assert!(
        f.domain
            .complete_configuration(&f.pa, &first.id, success(config(&f.agent)), now_ms())
            .await
            .is_err()
    );
    assert!(
        f.domain
            .configuration_status(&f.pb, &f.agent, now_ms())
            .await
            .unwrap()
            .environment
            .is_none()
    );
    let f = fixture().await;
    heartbeat(&f).await;
    let r = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "requester", json!({})),
            now_ms(),
        )
        .await
        .unwrap();
    claim(&f).await;
    f.auth
        .revoke_device(&f.session, f.pb.device_id.as_deref().unwrap(), now_ms())
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .configuration_request(&f.pa, &f.agent, &r.id, now_ms())
            .await
            .unwrap()
            .state,
        "unknown"
    );
    assert!(
        f.domain
            .complete_configuration(&f.pa, &r.id, success(config(&f.agent)), now_ms())
            .await
            .is_err()
    );
    assert!(
        f.domain
            .configuration_status(&f.pb, &f.agent, now_ms())
            .await
            .is_err()
    );
    let f = fixture().await;
    heartbeat(&f).await;
    let r = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "target", json!({})),
            now_ms(),
        )
        .await
        .unwrap();
    f.auth
        .revoke_device(&f.session, f.pa.device_id.as_deref().unwrap(), now_ms())
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .configuration_request(&f.pb, &f.agent, &r.id, now_ms())
            .await
            .unwrap()
            .state,
        "expired"
    );
    assert!(
        f.domain
            .poll_configuration(&f.pa, ConfigurationPoll { accept_work: true }, now_ms())
            .await
            .is_err()
    );
    assert!(
        !f.domain
            .configuration_status(&f.pb, &f.agent, now_ms())
            .await
            .unwrap()
            .online
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_parallel_connections_rotate_and_cached_observation() {
    let f = fixture().await;
    heartbeat(&f).await;
    let second = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        f.domain.create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "same", json!({})),
            now_ms()
        ),
        second.create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::AgentModel, "same", json!({})),
            now_ms()
        )
    );
    let one = one.unwrap();
    assert_eq!(one.id, two.unwrap().id);
    let (one, two) = tokio::join!(
        f.domain.create_configuration_request(
            &f.pb,
            &f.agent,
            input(
                ConfigurationOperation::ProviderConnect,
                "first-mutation",
                json!({"runtime":"codex"})
            ),
            now_ms()
        ),
        second.create_configuration_request(
            &f.pb,
            &f.agent,
            input(
                ConfigurationOperation::ProviderConnect,
                "second-mutation",
                json!({"runtime":"codex"})
            ),
            now_ms()
        )
    );
    assert_eq!(
        usize::from(one.is_ok()) + usize::from(two.is_ok()),
        1,
        "one outstanding mutation across separate DB connections"
    );
    let first = claim(&f).await;
    f.domain
        .complete_configuration(&f.pa, &first.id, success(config(&f.agent)), now_ms())
        .await
        .unwrap();
    let mutation = claim(&f).await;
    f.domain
        .complete_configuration(&f.pa, &mutation.id, success(provider()), now_ms())
        .await
        .unwrap();
    let r = f
        .domain
        .create_configuration_request(
            &f.pb,
            &f.agent,
            input(ConfigurationOperation::Environment, "observe", json!({})),
            now_ms(),
        )
        .await
        .unwrap();
    claim(&f).await;
    f.domain
        .complete_configuration(
            &f.pa,
            &r.id,
            success(json!({"providers":[provider()],"configuration":config(&f.agent)})),
            now_ms(),
        )
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "UPDATE hagency_agent_v1.configuration_presence SET last_seen_at_ms=$2 WHERE device_id=$1",
    )
    .bind::<Text, _>(f.pa.device_id.as_deref().unwrap())
    .bind::<BigInt, _>(now_ms() - 20_000)
    .execute(&mut db)
    .await
    .unwrap();
    let cached = f
        .domain
        .configuration_status(&f.pb, &f.agent, now_ms())
        .await
        .unwrap();
    assert!(!cached.online);
    assert!(cached.cached);
    assert!(cached.environment.is_some());
    let rotated = f
        .auth
        .register_device(
            &f.session,
            RegisterDevice {
                installation_id: "device-a".into(),
                name: "Rotated A".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(rotated.device_id, f.pa.device_id.as_deref().unwrap());
    assert_eq!(rotated.generation, 2);
    let status = f
        .domain
        .configuration_status(&f.pb, &f.agent, now_ms())
        .await
        .unwrap();
    assert!(!status.online);
    assert!(status.environment.is_none());
    assert!(status.last_seen_at_ms.is_none());
    assert!(
        f.domain
            .complete_configuration(
                &f.pa,
                &r.id,
                success(json!({"providers":[provider()],"configuration":config(&f.agent)})),
                now_ms()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_requester_separate_session_signout_prevents_apply_confirmation() {
    let f = fixture().await;
    heartbeat(&f).await;
    let session = f
        .auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: f.pb.subject.clone(),
                mxid: f.pb.mxid.clone(),
                client_id: "config-test".into(),
                valid_until_ms: now_ms() + 1_000_000,
            },
            now_ms(),
        )
        .await
        .unwrap();
    let b = f
        .auth
        .register_device(
            &session.token,
            RegisterDevice {
                installation_id: "third-device".into(),
                name: "Separate session B".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let pb = f.auth.authenticate(&b.token, now_ms(), true).await.unwrap();
    assert_ne!(pb.session_id, f.pa.session_id);
    let r = f
        .domain
        .create_configuration_request(
            &pb,
            &f.agent,
            input(
                ConfigurationOperation::AgentModel,
                "separate-session",
                json!({}),
            ),
            now_ms(),
        )
        .await
        .unwrap();
    claim(&f).await;
    f.auth.sign_out(&session.token).await.unwrap();
    assert!(
        f.domain
            .complete_configuration(&f.pa, &r.id, success(config(&f.agent)), now_ms())
            .await
            .is_err()
    );
    let read = f
        .domain
        .configuration_request(&f.pa, &f.agent, &r.id, now_ms())
        .await
        .unwrap();
    assert_eq!(read.state, "unknown");
    assert!(read.result.is_none());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_auth_snapshot_and_mutations_do_not_invert_session_owner_locks() {
    use diesel_async::SimpleAsyncConnection;
    let f = fixture().await;
    heartbeat(&f).await;
    // A second live session is essential: the production cycle used B's session
    // held by authentication while configuration already held the common owner.
    let identity = Identity {
        issuer: "https://example.test/auth/".into(),
        subject: f.pb.subject.clone(),
        mxid: f.pb.mxid.clone(),
        client_id: "config-test".into(),
        valid_until_ms: now_ms() + 1_000_000,
    };
    let session = f.auth.sign_in(identity.clone(), now_ms()).await.unwrap();
    let b = f
        .auth
        .register_device(
            &session.token,
            RegisterDevice {
                installation_id: "auth-lock-b".into(),
                name: "Lock B".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let auth = Store::open(&f.url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let mut barrier = AsyncPgConnection::establish(&f.url).await.unwrap();
    barrier
        .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
        .await
        .unwrap();
    sql_query("SELECT true AS matched FROM hagency_agent_v1.users WHERE id=$1 FOR SHARE")
        .bind::<Text, _>(&f.pa.user_id)
        .execute(&mut barrier)
        .await
        .unwrap();
    // Snapshot authentication takes no locks and must complete while the owner
    // is locked. Previously it took B's session then waited for this owner row.
    let principal = tokio::time::timeout(
        Duration::from_secs(2),
        auth.authenticate(&b.token, now_ms(), true),
    )
    .await
    .expect("read-only authentication must not wait for configuration owner locks")
    .unwrap();
    assert_eq!(principal.user_id, f.pa.user_id);
    // Each mutation must wait at the common advisory lock, before holding a row.
    // Acquiring all session share locks remains possible; no retries are used.
    for operation in 0..5 {
        let auth = Store::open(&f.url, "example.test", "https://example.test/auth/")
            .await
            .unwrap();
        let token = session.token.clone();
        let target = b.device_id.clone();
        let identity = identity.clone();
        let work = tokio::spawn(async move {
            match operation {
                0 => auth.renew(&token, identity, now_ms()).await.map(|_| ()),
                1 => auth
                    .register_device(
                        &token,
                        RegisterDevice {
                            installation_id: "mutation-order".into(),
                            name: "Mutation".into(),
                        },
                        now_ms(),
                    )
                    .await
                    .map(|_| ()),
                2 => auth.revoke_device(&token, &target, now_ms()).await,
                3 => auth.sign_in(identity, now_ms()).await.map(|_| ()),
                _ => auth.sign_out(&token).await,
            }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !work.is_finished(),
            "auth mutation must enter the common authority lock before rows"
        );
        tokio::time::timeout(Duration::from_secs(2),sql_query("SELECT true AS matched FROM hagency_agent_v1.sessions WHERE user_id=$1 ORDER BY id FOR SHARE").bind::<Text,_>(&f.pa.user_id).execute(&mut barrier)).await.expect("configuration must acquire session locks without auth inversion").unwrap();
        barrier.batch_execute("COMMIT").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .expect("auth mutation must finish after configuration commit")
            .unwrap()
            .unwrap();
        if operation < 4 {
            barrier
                .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
                .await
                .unwrap();
            sql_query("SELECT true AS matched FROM hagency_agent_v1.users WHERE id=$1 FOR SHARE")
                .bind::<Text, _>(&f.pa.user_id)
                .execute(&mut barrier)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_configuration_http_authentication_races_heartbeats_without_deadlock_or_retries() {
    let f = fixture().await;
    let service = Arc::new(service(&f));
    let path = format!("agents/{}/configuration", f.agent);
    // Real HTTP does Store::authenticate before entering the independently
    // connected DomainStore. This path was absent from domain-only race tests.
    let mut work = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let service = service.clone();
        let a = f.a.clone();
        let b = f.b.clone();
        let path = path.clone();
        work.spawn(async move {
            for _ in 0..20 {
                tokio::join!(
                    get(&service, &path, &b, StatusCode::OK),
                    post(
                        &service,
                        "device-configuration/poll",
                        &a,
                        json!({"acceptWork":false}),
                        StatusCode::OK
                    ),
                );
            }
        });
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(done) = work.join_next().await {
            done.unwrap();
        }
    })
    .await
    .expect("authentication and heartbeat must converge without any transient storage errors");
}
