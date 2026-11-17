//! Owner rename flow: HTTP PATCH -> stored name -> Matrix profile PUT.
use crate::{
    api::{App, now_ms},
    domain::{AgentVisibility, CreateAgent, DomainStore},
    gateway::Gateway,
    identity::{Identity, Verifier},
    matrix_client::MatrixClient,
    secret_token,
    store::Store,
};
use salvo::{
    Service,
    http::StatusCode,
    test::{ResponseExt, TestClient},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

type Seen = Arc<Mutex<Vec<String>>>;
/// Lets a test act while the first profile PUT is held: (reached, release).
type Gate = (tokio::sync::mpsc::Sender<()>, tokio::sync::mpsc::Receiver<()>);

/// Reads one whole HTTP request: headers, then `content-length` body bytes.
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
            if buf.len() >= end + 4 + length { return text; }
        }
        match socket.read(&mut chunk).await { Ok(0) | Err(_) => return text, Ok(n) => buf.extend_from_slice(&chunk[..n]) }
    }
}

/// Records each request; answers `status` with `{}`. The first request waits on `gate`.
async fn fake_matrix(status: u16, gate: Option<Gate>) -> (Url, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut gate = gate;
        while let Ok((mut socket, _)) = listener.accept().await {
            let request = read_request(&mut socket).await;
            log.lock().unwrap().push(request);
            if let Some((reached, mut release)) = gate.take() {
                reached.send(()).await.unwrap();
                release.recv().await.unwrap();
            }
            let reply = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (origin, seen)
}

fn profile_put(request: &str, name: &str) -> bool {
    request.starts_with("PUT /_matrix/client/v3/profile/") && request.contains("/displayname?user_id=") && request.ends_with(&json!({"displayname":name}).to_string())
}

async fn token(store: &Store, name: &str) -> String {
    let identity = Identity {
        issuer: "https://example.test/auth/".into(),
        subject: name.into(),
        mxid: format!("@{name}:example.test"),
        client_id: "rename-test".into(),
        valid_until_ms: now_ms() + 30000,
    };
    store.sign_in(identity, now_ms()).await.unwrap().token
}

async fn patch(service: &Service, path: &str, credential: &str, body: Value) -> (StatusCode, Value) {
    let mut response = TestClient::patch(format!("https://example.test{path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(credential)
        .json(&body)
        .send(service)
        .await;
    let status = response.status_code.unwrap();
    (status, response.take_json::<Value>().await.unwrap_or(Value::Null))
}

fn service(store: &Store, domain: &DomainStore, matrix: Url) -> Service {
    let issuer = Url::parse("https://example.test/auth/").unwrap();
    let origin = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(issuer.clone(), issuer.join("introspect").unwrap(), origin.clone(), "unused-test-secret".into(), "example.test".into()).unwrap();
    let gateway = Gateway::new(Arc::new(|_| Box::pin(async { Ok(Vec::new()) })), "@_hagency_service:example.test".into());
    let client = MatrixClient::new(matrix, "fixture-as-token-only-01234567890123456789".into(), "example.test".into()).unwrap();
    let app = App::new(store.clone(), verifier, origin, issuer, "example.test".into())
        .with_domain(domain.clone(), gateway)
        .with_matrix_client(client);
    Service::new(app.router())
}

struct Fixture { store: Store, domain: DomainStore, owner: crate::store::Principal, owner_token: String, other_token: String, agent: crate::domain::Agent, path: String }

/// One owner with a provisioned (active) private Agent named "Before".
async fn fixture(tag: &str, provisioned: bool) -> Fixture {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", "https://example.test/auth/").await.unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_").await.unwrap();
    let suffix = secret_token();
    let owner_token = token(&store, &format!("rename_owner_{tag}_{suffix}")).await;
    let other_token = token(&store, &format!("rename_other_{tag}_{suffix}")).await;
    let device = crate::store::RegisterDevice { installation_id: format!("rename-{tag}"), name: "Rename device".into() };
    let device = store.register_device(&owner_token, device, now_ms()).await.unwrap();
    let owner = store.authenticate(&device.token, now_ms(), true).await.unwrap();
    let create = CreateAgent { visibility: AgentVisibility::Private, display_name: "Before".into(), idempotency_key: format!("rename-{tag}") };
    let agent = domain.create_agent(&owner, create, now_ms()).await.unwrap();
    if provisioned {
        domain.confirm_identity_provisioned(&agent.id, agent.generation).await.unwrap();
    }
    let path = format!("/hagency/api/v1/agents/{}", agent.id);
    Fixture { store, domain, owner, owner_token, other_token, agent, path }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_owner_renames_agent_and_matrix_profile_follows() {
    let f = fixture("flow", false).await;
    let (matrix, seen) = fake_matrix(200, None).await;
    let service = service(&f.store, &f.domain, matrix);
    let (status, body) = patch(&service, &f.path, &f.owner_token, json!({"displayName":"Too early"})).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("agent_provisioning")));
    f.domain.confirm_identity_provisioned(&f.agent.id, f.agent.generation).await.unwrap();
    for invalid in [json!({}), json!({"displayName":""}), json!({"displayName":"   "}), json!({"displayName":"a".repeat(65)}), json!({"displayName":"bad\u{7}name"}), json!({"displayName":"ok","ownerUserId":f.owner.user_id})] {
        assert_eq!(patch(&service, &f.path, &f.owner_token, invalid).await.0, StatusCode::BAD_REQUEST);
    }
    let (status, _) = patch(&service, &f.path, &f.other_token, json!({"displayName":"Hijack"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(seen.lock().unwrap().is_empty(), "no Matrix call for rejected renames");
    let name = "Résumé bot ".to_string() + &"x".repeat(53);
    let (status, body) = patch(&service, &f.path, &f.owner_token, json!({"displayName":name})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent"]["displayName"], name);
    assert_eq!(f.domain.agent(&f.owner, &f.agent.id, now_ms()).await.unwrap().display_name, name);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(profile_put(&requests[0], &name), "{}", requests[0]);
    f.domain.retire_agent(&f.owner, &f.agent.id, now_ms()).await.unwrap();
    let (status, _) = patch(&service, &f.path, &f.owner_token, json!({"displayName":"After retire"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_matrix_failure_is_503_and_keeps_the_saved_name_for_retry() {
    let f = fixture("fail", true).await;
    let (matrix, seen) = fake_matrix(500, None).await;
    let service = service(&f.store, &f.domain, matrix);
    let (status, body) = patch(&service, &f.path, &f.owner_token, json!({"displayName":"Saved"})).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("matrix_display_name_unavailable")));
    assert_eq!(f.domain.agent(&f.owner, &f.agent.id, now_ms()).await.unwrap().display_name, "Saved");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_rename_landing_during_the_profile_put_converges_on_the_latest_name() {
    let f = fixture("race", true).await;
    let (reached_tx, mut reached) = tokio::sync::mpsc::channel(1);
    let (release, release_rx) = tokio::sync::mpsc::channel(1);
    let (matrix, seen) = fake_matrix(200, Some((reached_tx, release_rx))).await;
    let service = service(&f.store, &f.domain, matrix);
    let first = patch(&service, &f.path, &f.owner_token, json!({"displayName":"First"}));
    let racer = async {
        reached.recv().await.unwrap();
        f.domain.rename_agent(&f.owner, &f.agent.id, "Second", now_ms()).await.unwrap();
        release.send(()).await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(first, racer);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent"]["displayName"], "Second", "the reply is the stored name");
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(profile_put(&requests[0], "First") && profile_put(&requests[1], "Second"));
}

/// The binding worker's snapshot predates a rename; it PUTs the stored name.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_worker_snapshot_takes_the_stored_name_under_the_profile_lock() {
    let f = fixture("worker", true).await;
    let snapshot = f.domain.agent(&f.owner, &f.agent.id, now_ms()).await.unwrap();
    let held = crate::domain::PROFILE_WRITES.lock().await;
    let (matrix, _seen) = fake_matrix(200, None).await;
    let service = service(&f.store, &f.domain, matrix);
    let rename = patch(&service, &f.path, &f.owner_token, json!({"displayName":"Renamed"}));
    let waited = tokio::time::timeout(std::time::Duration::from_millis(200), rename).await;
    assert!(waited.is_err(), "a rename waits while a worker holds the profile lock");
    drop(held);
    let (status, _) = patch(&service, &f.path, &f.owner_token, json!({"displayName":"Renamed"})).await;
    assert_eq!(status, StatusCode::OK);
    let _profile = crate::domain::PROFILE_WRITES.lock().await;
    assert_eq!(f.domain.with_stored_name(snapshot).await.unwrap().display_name, "Renamed");
}
