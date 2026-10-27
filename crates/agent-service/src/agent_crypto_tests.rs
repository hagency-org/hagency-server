use super::*;
use crate::{
    api::{App, now_ms},
    gateway::Gateway,
    identity::{Identity, Verifier},
    store::{RegisterDevice, Store},
};
use diesel::sql_types::Text;
use salvo::{
    prelude::*,
    test::{ResponseExt, TestClient},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn device_keys(user: &str, device: &str, byte: u8) -> Value {
    let key = STANDARD_NO_PAD.encode([byte; 32]);
    json!({"user_id":user,"device_id":device,"algorithms":["m.olm.v1.curve25519-aes-sha2","m.megolm.v1.aes-sha2"],"keys":{format!("ed25519:{device}"):key,format!("curve25519:{device}"):key},"signatures":{user:{format!("ed25519:{device}"):"signature"}}})
}
#[test]
fn identity_broker_has_no_message_login_backup_or_peer_authority() {
    for (method, path) in [
        ("GET", "/_matrix/client/v3/sync"),
        ("POST", "/_matrix/client/v3/login"),
        (
            "PUT",
            "/_matrix/client/v3/sendToDevice/m.room.encrypted/txn",
        ),
        (
            "PUT",
            "/_matrix/client/v3/rooms/room/send/m.room.encrypted/txn",
        ),
        ("POST", "/_matrix/client/v3/keys/claim"),
        ("POST", "/_matrix/client/v3/room_keys/keys"),
        ("DELETE", "/_matrix/client/v3/devices/device"),
        ("POST", "/_matrix/client/v3/keys%2Fupload"),
    ] {
        assert!(Operation::route(method, path).is_err());
    }
    let user = "@_hagency_test:example.test";
    let device = "device";
    assert!(
        fingerprints(
            Operation::Query,
            &json!({"device_keys":{"@foreign:example.test":[]}}),
            user,
            device
        )
        .is_err()
    );
    assert!(
        fingerprints(
            Operation::Query,
            &json!({"device_keys":{user:["foreign"]}}),
            user,
            device
        )
        .is_err()
    );
    assert!(
        fingerprints(
            Operation::Signing,
            &json!({"auth":{},"master_key":{}}),
            user,
            device
        )
        .is_err()
    );
    let mut keys = device_keys(user, device, 7);
    let expected = fingerprints(
        Operation::Upload,
        &json!({"device_keys":keys}),
        user,
        device,
    )
    .unwrap();
    keys["algorithms"].as_array_mut().unwrap().reverse();
    keys["unsigned"] = json!({"device_display_name":"different"});
    assert_eq!(
        expected,
        fingerprints(
            Operation::Upload,
            &json!({"device_keys":keys}),
            user,
            device
        )
        .unwrap()
    );
    keys["device_id"] = json!("other");
    assert!(
        fingerprints(
            Operation::Upload,
            &json!({"device_keys":keys}),
            user,
            device
        )
        .is_err()
    );
}

type Calls = Arc<Mutex<Vec<(String, Value)>>>;
#[derive(Clone)]
struct Matrix {
    calls: Calls,
    fail: Arc<AtomicBool>,
}
#[handler]
impl Matrix {
    async fn handle(&self, req: &mut Request, res: &mut Response) {
        assert_eq!(
            req.headers().get("authorization").unwrap(),
            "Bearer fixture-AS-only"
        );
        let url = url::Url::parse(&format!("http://example.test{}", req.uri())).unwrap();
        let params = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        let raw = req.payload().await.unwrap();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap()
        };
        if url.path().ends_with("/register") {
            assert_eq!(body["inhibit_login"], true);
            assert_eq!(body["type"], "m.login.application_service");
            assert!(
                body["username"]
                    .as_str()
                    .unwrap()
                    .starts_with("_hagency_test_agt_")
            );
            self.calls
                .lock()
                .unwrap()
                .push((req.uri().to_string(), body.clone()));
            res.render(Json(
                json!({"user_id":format!("@{}:example.test",body["username"].as_str().unwrap())}),
            ));
            return;
        }
        if url.path().ends_with("/versions") {
            assert!(params.is_empty());
            self.calls
                .lock()
                .unwrap()
                .push((req.uri().to_string(), body.clone()));
            res.render(Json(json!({"versions":["v1.15"]})));
            return;
        }
        assert!(params["user_id"].starts_with("@_hagency_test_agt_"));
        self.calls
            .lock()
            .unwrap()
            .push((req.uri().to_string(), body.clone()));
        if self.fail.as_ref().load(Ordering::Acquire) {
            res.status_code(StatusCode::SERVICE_UNAVAILABLE);
            res.render(Json(
                json!({"errcode":"M_UNAVAILABLE","error":"test-original-failure"}),
            ));
            return;
        }
        if url.path().ends_with("/versions") {
            res.render(Json(json!({"versions":["v1.15"]})));
        } else if url.path().ends_with("/sync") {
            assert_eq!(req.method(), salvo::http::Method::GET);
            assert_eq!(params["timeout"], "0");
            assert_eq!(params["set_presence"], "offline");
            assert_eq!(params["filter"], receive::FILTER);
            assert!(params["device_id"].starts_with("HAGENCY_"));
            assert_eq!(params["since"], "opaque-test-token");
            assert!(body.is_null());
            res.render(Json(json!({"next_batch":"next-test-token","to_device":{"events":[{"sender":"@owner:example.test","type":"m.room.encrypted","content":{"algorithm":"m.olm.v1.curve25519-aes-sha2","ciphertext":{}}}]}})));
        } else if url.path().ends_with("/whoami") {
            res.render(Json(json!({"user_id":params["user_id"],"device_id":params["device_id"],"is_guest":false})));
        } else {
            res.render(Json(json!({})));
        }
    }
}
struct Fixture {
    auth: Store,
    domain: DomainStore,
    session: String,
    token: String,
    p: Principal,
    second: Principal,
    second_token: String,
    agent: String,
    matrix: MatrixClient,
    calls: Calls,
    fail: Arc<AtomicBool>,
    http: tokio::task::JoinHandle<()>,
    url: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.http.abort();
    }
}
async fn fixture() -> Fixture {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated DB required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    crate::transport::TransportStore::open(&url, Default::default())
        .await
        .unwrap();
    let suffix = crate::secret_token();
    let session = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: suffix.clone(),
                mxid: format!("@crypto_{suffix}:example.test"),
                client_id: "crypto-test".into(),
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
                installation_id: "a".into(),
                name: "Execution A".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let second_token = auth
        .register_device(
            &session,
            RegisterDevice {
                installation_id: "b".into(),
                name: "Execution B".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let p = auth.authenticate(&token, now_ms(), true).await.unwrap();
    let second = auth
        .authenticate(&second_token, now_ms(), true)
        .await
        .unwrap();
    let agent = domain
        .create_agent(
            &p,
            CreateAgent {
                visibility: AgentVisibility::Private,
                display_name: "Crypto Agent".into(),
                idempotency_key: "crypto-create".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .id;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(AtomicBool::new(false));
    let acceptor = salvo::conn::TcpListener::new("127.0.0.1:0")
        .try_bind()
        .await
        .unwrap();
    let address = acceptor.local_addr().unwrap();
    let router = Router::with_path("{**path}").goal(Matrix {
        calls: calls.clone(),
        fail: fail.clone(),
    });
    let http = tokio::spawn(async move {
        Server::new(acceptor).serve(router).await;
    });
    let matrix = MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "fixture-AS-only".into(),
        "example.test".into(),
    )
    .unwrap();
    Fixture {
        auth,
        domain,
        session,
        token,
        p,
        second,
        second_token,
        agent,
        matrix,
        calls,
        fail,
        http,
        url,
    }
}
async fn provision(f: &Fixture) -> String {
    let (status, _, device) = f
        .domain
        .crypto_request(
            &f.matrix,
            &f.p,
            &f.agent,
            1,
            Operation::Provision,
            Value::Null,
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    device
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn original_identity_pins_survive_http_failure_reopen_and_device_reassignment() {
    let mut f = fixture().await;
    let device = provision(&f).await;
    assert_eq!(device, provision(&f).await);
    let agent = f.domain.agent(&f.p, &f.agent, now_ms()).await.unwrap();
    let body = json!({"device_keys":device_keys(&agent.puppet_mxid,&device,5)});
    f.fail.store(true, Ordering::Release);
    assert_eq!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::Upload,
                body.clone()
            )
            .await
            .unwrap()
            .0,
        503
    );
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let count = f.calls.lock().unwrap().len();
    assert!(matches!(
        reopened
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::Upload,
                json!({"device_keys":device_keys(&agent.puppet_mxid,&device,6)})
            )
            .await,
        Err(Error::Conflict("agent_crypto_identity_changed"))
    ));
    assert_eq!(count, f.calls.lock().unwrap().len());
    f.fail.store(false, Ordering::Release);
    assert_eq!(
        reopened
            .crypto_request(&f.matrix, &f.p, &f.agent, 1, Operation::Upload, body)
            .await
            .unwrap()
            .0,
        200
    );
    let rotated = f
        .auth
        .register_device(
            &f.session,
            RegisterDevice {
                installation_id: "a".into(),
                name: "Same execution host".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let new_principal = f
        .auth
        .authenticate(&rotated.token, now_ms(), true)
        .await
        .unwrap();
    let old = std::mem::replace(&mut f.p, new_principal);
    assert_eq!(old.device_id, f.p.device_id);
    assert!(f.p.device_generation > old.device_generation);
    assert!(
        f.domain
            .crypto_request(&f.matrix, &old, &f.agent, 1, Operation::Whoami, Value::Null)
            .await
            .is_err()
    );
    assert_eq!(provision(&f).await, device);
    let signing_key = STANDARD_NO_PAD.encode([8u8; 32]);
    let signing = json!({"master_key":{"user_id":agent.puppet_mxid,"usage":["master"],"keys":{format!("ed25519:{signing_key}"):signing_key}}});
    reopened
        .crypto_request(&f.matrix, &f.p, &f.agent, 1, Operation::Signing, signing)
        .await
        .unwrap();
    let changed = STANDARD_NO_PAD.encode([9u8; 32]);
    assert!(matches!(reopened.crypto_request(&f.matrix,&f.p,&f.agent,1,Operation::Signing,json!({"master_key":{"user_id":agent.puppet_mxid,"usage":["master"],"keys":{format!("ed25519:{changed}"):changed}}})).await,Err(Error::Conflict("agent_crypto_identity_changed"))));
    assert!(matches!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.second,
                &f.agent,
                1,
                Operation::Provision,
                Value::Null
            )
            .await,
        Err(Error::Unauthorized("execution_device_required"))
    ));
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            SetExecutionDevice {
                expected_generation: 1,
            },
            now_ms(),
        )
        .await
        .unwrap();
    let count = f.calls.lock().unwrap().len();
    assert!(
        f.domain
            .crypto_request(&f.matrix, &f.p, &f.agent, 1, Operation::Whoami, Value::Null)
            .await
            .is_err()
    );
    assert_eq!(count, f.calls.lock().unwrap().len());
    let (_, _, new) = f
        .domain
        .crypto_request(
            &f.matrix,
            &f.second,
            &f.agent,
            2,
            Operation::Provision,
            Value::Null,
        )
        .await
        .unwrap();
    assert_ne!(new, device);
    let key = STANDARD_NO_PAD.encode([10u8; 32]);
    assert!(matches!(f.domain.crypto_request(&f.matrix,&f.second,&f.agent,2,Operation::Signing,json!({"master_key":{"user_id":agent.puppet_mxid,"usage":["master"],"keys":{format!("ed25519:{key}"):key}}})).await,Err(Error::Conflict("agent_crypto_identity_changed"))));

    f.auth
        .revoke_device(&f.session, f.second.device_id.as_ref().unwrap(), now_ms())
        .await
        .unwrap();
    let count = f.calls.lock().unwrap().len();
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.second,
                &f.agent,
                2,
                Operation::Whoami,
                Value::Null
            )
            .await
            .is_err()
    );
    assert_eq!(count, f.calls.lock().unwrap().len());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn native_identity_proxy_rejects_scope_injection_and_message_endpoints_before_io() {
    let f = fixture().await;
    let gateway = Gateway::new(
        Arc::new(|_| Box::pin(async { Err(Error::Unauthorized("unused")) })),
        "@_hagency_service:example.test".into(),
    );
    let app = App::new(
        f.auth.clone(),
        Verifier::new(
            "https://example.test/auth/".parse().unwrap(),
            "https://example.test/auth/introspection".parse().unwrap(),
            "https://example.test/".parse().unwrap(),
            "x".into(),
            "example.test".into(),
        )
        .unwrap(),
        "https://example.test/".parse().unwrap(),
        "https://example.test/auth/".parse().unwrap(),
        "example.test".into(),
    )
    .with_domain(f.domain.clone(), gateway)
    .with_matrix_client(f.matrix.clone());
    let service = Service::new(app.router());
    let root = format!("https://example.test/hagency/agent-crypto/{}/1", f.agent);
    let mut response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let manifest: Value = response.take_json().await.unwrap();
    assert_eq!(
        manifest["executionDeviceId"],
        f.p.device_id.as_ref().unwrap().as_str()
    );
    let mut who = TestClient::get(format!("{root}/_matrix/client/v3/account/whoami"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.token)
        .send(&service)
        .await;
    let who: Value = who.take_json().await.unwrap();
    assert_eq!(who["device_id"], manifest["matrixDeviceId"]);
    assert_eq!(who["user_id"], manifest["matrixUserId"]);
    let mut received = TestClient::post(format!("{root}/receive"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.token)
        .send(&service)
        .await;
    assert_eq!(received.status_code, Some(StatusCode::OK));
    let receive_manifest: Value = received.take_json().await.unwrap();
    assert_eq!(receive_manifest["homeserver"], format!("{root}/receive"));
    assert_eq!(
        receive_manifest["matrixDeviceId"],
        manifest["matrixDeviceId"]
    );
    let mut synced = TestClient::get(format!("{root}/receive/_matrix/client/v3/sync?since=opaque-test-token&timeout=5000&set_presence=online&full_state=false&use_state_after=true"))
        .add_header("host", "example.test", true).bearer_auth(&f.token).send(&service).await;
    assert_eq!(synced.status_code, Some(StatusCode::OK));
    let synced: Value = synced.take_json().await.unwrap();
    assert_eq!(synced["next_batch"], "next-test-token");
    assert_eq!(synced["to_device"]["events"].as_array().unwrap().len(), 1);
    let count = f.calls.lock().unwrap().len();
    for query in [
        "filter={}",
        "user_id=@foreign:example.test",
        "device_id=other",
        "access_token=injected",
        "since=a&since=b",
        "full_state=true",
        "timeout=5001",
    ] {
        let response = TestClient::get(format!("{root}/receive/_matrix/client/v3/sync?{query}"))
            .add_header("host", "example.test", true)
            .bearer_auth(&f.token)
            .send(&service)
            .await;
        assert!(
            !response.status_code.unwrap_or(StatusCode::OK).is_success(),
            "{query}"
        );
    }
    for tail in [
        "/receive/_matrix/client/v3/sync",
        "/receive/_matrix/client/v3/rooms/r/messages",
    ] {
        let response = TestClient::get(format!("{root}{tail}"))
            .add_header("host", "example.test", true)
            .bearer_auth(&f.second_token)
            .send(&service)
            .await;
        assert!(!response.status_code.unwrap_or(StatusCode::OK).is_success());
    }
    assert_eq!(count, f.calls.lock().unwrap().len());
    for suffix in [
        "/_matrix/client/v3/login",
        "/_matrix/client/v3/keys/claim",
        "/_matrix/client/v3/keys/upload?user_id=@foreign:example.test",
        "/_matrix/client/v3/keys/upload?device_id=other",
        "/_matrix/client/v3/rooms/room/send/m.room.encrypted/txn",
        "/_matrix/client/v3/keys%2Fupload",
    ] {
        let response = TestClient::post(format!("{root}{suffix}"))
            .add_header("host", "example.test", true)
            .bearer_auth(&f.token)
            .json(&json!({}))
            .send(&service)
            .await;
        assert!(
            !response.status_code.unwrap_or(StatusCode::OK).is_success(),
            "{suffix}"
        );
    }
    for header in ["origin", "cookie"] {
        let response = TestClient::post(format!("{root}/provision"))
            .add_header("host", "example.test", true)
            .add_header(header, "forbidden", true)
            .bearer_auth(&f.token)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    }
    let response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.second_token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.session)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let foreign_id = crate::secret_token();
    let foreign = f
        .auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: foreign_id.clone(),
                mxid: format!("@foreign_{foreign_id}:example.test"),
                client_id: "crypto-test".into(),
                valid_until_ms: now_ms() + 30_000,
            },
            now_ms(),
        )
        .await
        .unwrap();
    let foreign = f
        .auth
        .register_device(
            &foreign.token,
            RegisterDevice {
                installation_id: "foreign".into(),
                name: "Foreign owner".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "example.test", true)
        .bearer_auth(&foreign.token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "wrong.example.test", true)
        .bearer_auth(&f.token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let response = TestClient::post(format!("{root}/provision"))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.token)
        .add_header("authorization", "Bearer duplicate", false)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    assert_eq!(count, f.calls.lock().unwrap().len());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn identity_expiry_is_resampled_after_a_real_agent_row_lock_wait() {
    let f = fixture().await;
    let until = now_ms() + 4000;
    let mut blocker = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=$2 WHERE id=$1")
        .bind::<Text, _>(&f.p.session_id)
        .bind::<BigInt, _>(until)
        .execute(&mut blocker)
        .await
        .unwrap();
    blocker.batch_execute("BEGIN").await.unwrap();
    sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 FOR UPDATE")
        .bind::<Text, _>(&f.agent)
        .get_result::<Flag>(&mut blocker)
        .await
        .unwrap();
    let p = f.auth.authenticate(&f.token, now_ms(), true).await.unwrap();
    let domain = f.domain.clone();
    let matrix = f.matrix.clone();
    let agent = f.agent.clone();
    let pending = tokio::spawn(async move {
        domain
            .crypto_request(&matrix, &p, &agent, 1, Operation::Provision, Value::Null)
            .await
    });
    let mut observer = AsyncPgConnection::establish(&f.url).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        loop {
            let waiting=sql_query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=%FOR SHARE') AS matched").get_result::<Flag>(&mut observer).await.unwrap();
            if waiting.matched {break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("observe actual Agent row wait, not just a spawned task");
    tokio::time::sleep(std::time::Duration::from_millis(
        (until - now_ms()).max(0) as u64 + 50,
    ))
    .await;
    blocker.batch_execute("COMMIT").await.unwrap();
    assert!(matches!(
        pending.await.unwrap(),
        Err(Error::Unauthorized("authorization_expired"))
    ));
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn owner_receive_signatures_require_original_agent_user_signer_before_http() {
    let f = fixture().await;
    let device = provision(&f).await;
    let a = f.domain.agent(&f.p, &f.agent, now_ms()).await.unwrap();
    let signer = STANDARD_NO_PAD.encode([41u8; 32]);
    let master = STANDARD_NO_PAD.encode([42u8; 32]);
    let signature = STANDARD_NO_PAD.encode([43u8; 64]);
    let body = json!({&f.p.mxid:{&master:{"user_id":&f.p.mxid,"usage":["master"],"keys":{format!("ed25519:{master}"):&master},"signatures":{&a.puppet_mxid:{format!("ed25519:{signer}"):&signature},&f.p.mxid:{"ed25519:owner":"must-not-forward"}}}}});
    let count = f.calls.lock().unwrap().len();
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::OwnerSignatures,
                body.clone()
            )
            .await
            .is_err()
    );
    assert_eq!(count, f.calls.lock().unwrap().len());
    f.domain.crypto_request(&f.matrix,&f.p,&f.agent,1,Operation::Signing,json!({"user_signing_key":{"user_id":&a.puppet_mxid,"usage":["user_signing"],"keys":{format!("ed25519:{signer}"):&signer}}})).await.unwrap();
    let count = f.calls.lock().unwrap().len();
    let mut wrong = body.clone();
    wrong[&f.p.mxid][&master]["signatures"][&a.puppet_mxid] =
        json!({format!("ed25519:{master}"):&signature});
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::OwnerSignatures,
                wrong
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.second,
                &f.agent,
                1,
                Operation::OwnerSignatures,
                body.clone()
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                2,
                Operation::OwnerSignatures,
                body.clone()
            )
            .await
            .is_err()
    );
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::OwnerQuery,
                json!({"device_keys":{"@foreign:example.test":[]}})
            )
            .await
            .is_err()
    );
    assert_eq!(count, f.calls.lock().unwrap().len());
    assert_eq!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::OwnerSignatures,
                body
            )
            .await
            .unwrap()
            .0,
        200
    );
    {
        let calls = f.calls.lock().unwrap();
        let (uri, forwarded) = calls.last().unwrap();
        let url = url::Url::parse(&format!("http://example.test{uri}")).unwrap();
        let params = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(params["user_id"], a.puppet_mxid);
        assert_eq!(params["device_id"], device);
        assert_eq!(
            forwarded[&f.p.mxid][&master]["signatures"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(
            forwarded[&f.p.mxid][&master]["signatures"]
                .get(&f.p.mxid)
                .is_none()
        );
    }
    f.auth
        .revoke_device(&f.session, f.p.device_id.as_ref().unwrap(), now_ms())
        .await
        .unwrap();
    let count = f.calls.lock().unwrap().len();
    assert!(
        f.domain
            .crypto_request(
                &f.matrix,
                &f.p,
                &f.agent,
                1,
                Operation::ReceiveSync,
                json!({"since":"opaque-test-token"})
            )
            .await
            .is_err()
    );
    assert_eq!(count, f.calls.lock().unwrap().len());
}
