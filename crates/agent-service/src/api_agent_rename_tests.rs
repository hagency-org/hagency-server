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

/// Records each request's head and body; answers `{}` with 200.
async fn fake_matrix() -> (Url, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0; 16384];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let reply = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (origin, seen)
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

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_owner_renames_agent_and_matrix_profile_follows() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", "https://example.test/auth/").await.unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_").await.unwrap();
    let suffix = secret_token();
    let owner_token = token(&store, &format!("rename_owner_{suffix}")).await;
    let other_token = token(&store, &format!("rename_other_{suffix}")).await;
    let device = crate::store::RegisterDevice { installation_id: "rename-device".into(), name: "Rename device".into() };
    let device = store.register_device(&owner_token, device, now_ms()).await.unwrap();
    let owner = store.authenticate(&device.token, now_ms(), true).await.unwrap();
    let create = CreateAgent { visibility: AgentVisibility::Private, display_name: "Before".into(), idempotency_key: "rename-create".into() };
    let agent = domain.create_agent(&owner, create, now_ms()).await.unwrap();
    domain.confirm_identity_provisioned(&agent.id, agent.generation).await.unwrap();
    let (matrix, seen) = fake_matrix().await;
    let service = service(&store, &domain, matrix);
    let path = format!("/hagency/api/v1/agents/{}", agent.id);
    for invalid in [json!({}), json!({"displayName":""}), json!({"displayName":"   "}), json!({"displayName":"a".repeat(65)}), json!({"displayName":"bad\u{7}name"}), json!({"displayName":"ok","ownerUserId":owner.user_id})] {
        assert_eq!(patch(&service, &path, &owner_token, invalid).await.0, StatusCode::BAD_REQUEST);
    }
    let (status, _) = patch(&service, &path, &other_token, json!({"displayName":"Hijack"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(seen.lock().unwrap().is_empty(), "no Matrix call for rejected renames");
    let name = "Résumé bot ".to_string() + &"x".repeat(53);
    let (status, body) = patch(&service, &path, &owner_token, json!({"displayName":name})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent"]["displayName"], name);
    assert_eq!(domain.agent(&owner, &agent.id, now_ms()).await.unwrap().display_name, name);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("PUT /_matrix/client/v3/profile/"));
    assert!(requests[0].contains("/displayname?user_id="));
    assert!(requests[0].ends_with(&json!({"displayname":name}).to_string()));
    domain.retire_agent(&owner, &agent.id, now_ms()).await.unwrap();
    let (status, _) = patch(&service, &path, &owner_token, json!({"displayName":"After retire"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
}
