use crate::{
    api::{App, now_ms},
    identity::{Identity, Verifier},
    secret_token,
    store::{RegisterDevice, Store},
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
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const ISSUER: &str = "https://example.test/auth/";
fn identity(subject: &str) -> Identity {
    Identity {
        issuer: ISSUER.into(),
        subject: subject.into(),
        mxid: format!("@{subject}:example.test"),
        client_id: "resume-sdk".into(),
        valid_until_ms: now_ms() + 30_000,
    }
}
fn service(store: Store, upstream: &str) -> Service {
    service_within(store, upstream, 30_000)
}
/// A host configured to grant an authorization window of up to `window_ms`.
fn service_within(store: Store, upstream: &str, window_ms: i64) -> Service {
    let issuer = Url::parse(ISSUER).unwrap();
    let upstream = Url::parse(upstream).unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        upstream.join("introspect").unwrap(),
        upstream,
        "synthetic-introspection-secret".into(),
        "example.test".into(),
    )
    .unwrap()
    .with_authorization_window(window_ms)
    .unwrap();
    Service::new(
        App::new(
            store,
            verifier,
            Url::parse("https://example.test/").unwrap(),
            issuer,
            "example.test".into(),
        )
        .router(),
    )
}
async fn current(app: &Service, token: &str, query: &str, status: StatusCode) -> Value {
    let mut response = TestClient::get(format!(
        "https://example.test/hagency/api/v1/devices/current{query}"
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(token)
    .send(app)
    .await;
    assert_eq!(response.status_code, Some(status));
    assert_eq!(response.headers()["cache-control"], "no-store");
    response.take_json::<Value>().await.unwrap()
}
async fn register(store: &Store, token: &str, installation: &str) -> crate::store::DeviceGrant {
    store
        .register_device(
            token,
            RegisterDevice {
                installation_id: installation.into(),
                name: "Original resume fixture".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
}
#[derive(diesel::QueryableByName, Debug, PartialEq)]
struct Counts {
    #[diesel(sql_type=BigInt)]
    sessions: i64,
    #[diesel(sql_type=BigInt)]
    devices: i64,
}
async fn counts(url: &str, user: &str) -> Counts {
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    sql_query("SELECT (SELECT COUNT(*) FROM hagency_agent_v1.sessions WHERE user_id=$1) AS sessions,(SELECT COUNT(*) FROM hagency_agent_v1.devices WHERE user_id=$1) AS devices")
        .bind::<Text,_>(user).get_result(&mut db).await.unwrap()
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_current_device_is_closed_read_only_and_rejects_owner_revoked_or_rotated_tokens() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("resume_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let device = register(&store, &owner.token, &format!("installation_{name}")).await;
    let app = service(store.clone(), "http://127.0.0.1:1/");
    let before = counts(&url, &owner.user_id).await;
    let value = current(&app, &device.token, "", StatusCode::OK).await;
    assert_eq!(value.as_object().unwrap().len(), 8);
    for (key, expected) in [
        ("userId", owner.user_id.as_str()),
        ("mxid", owner.mxid.as_str()),
        ("subject", name.as_str()),
        ("clientId", "resume-sdk"),
        ("issuer", ISSUER),
        ("deviceId", device.device_id.as_str()),
    ] {
        assert_eq!(value[key], expected);
    }
    assert_eq!(value["generation"], device.generation);
    let deadline = value["validUntilMs"].as_i64().unwrap();
    assert!(deadline > now_ms() && deadline <= now_ms() + 30_000);
    assert!(!value.to_string().contains(&device.token));
    current(&app, &owner.token, "", StatusCode::UNAUTHORIZED).await;
    current(
        &app,
        &device.token,
        "?generation=1",
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        counts(&url, &owner.user_id).await,
        before,
        "observation never registers a session or device"
    );
    let next = register(&store, &owner.token, &format!("installation_{name}")).await;
    assert_eq!(next.device_id, device.device_id);
    assert_eq!(next.generation, device.generation + 1);
    current(&app, &device.token, "", StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        current(&app, &next.token, "", StatusCode::OK).await["generation"],
        next.generation
    );
    store
        .revoke_device(&owner.token, &next.device_id, now_ms())
        .await
        .unwrap();
    current(&app, &next.token, "", StatusCode::UNAUTHORIZED).await;
}

async fn upstream(
    subject: String,
    wrong: Arc<AtomicBool>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let subject = subject.clone();
            let wrong = wrong.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0u8; 4096];
                let split = loop {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    };
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(offset) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break offset + 4;
                    }
                };
                let length = String::from_utf8_lossy(&raw[..split])
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                assert!(length < 4096);
                while raw.len() < split + length {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8_lossy(&raw);
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let value = match path {
                    "/introspect" => {
                        json!({"active":true,"sub":if wrong.as_ref().load(Ordering::Acquire){format!("other_{subject}")}else{subject.clone()},"username":subject,"client_id":"resume-sdk","scope":"urn:matrix:client:api:*","exp":now_ms()/1000+600})
                    }
                    "/_matrix/client/v3/account/whoami" => {
                        json!({"user_id":format!("@{subject}:example.test")})
                    }
                    _ => panic!("unexpected synthetic identity route"),
                };
                let body = value.to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            });
        }
    });
    (address, task)
}
async fn renew(app: &Service, token: &str, status: StatusCode) -> Value {
    let mut response =
        TestClient::post("https://example.test/hagency/api/v1/sessions/current/renew")
            .add_header("host", "example.test", true)
            .bearer_auth(token)
            .json(&json!({"accessToken":"synthetic-fresh-upstream-token"}))
            .send(app)
            .await;
    assert_eq!(response.status_code, Some(status));
    response.take_json().await.unwrap()
}
/// The header by which the first clients asked for a window. It gives the
/// default window whatever it asks.
const FIRST: &str = "x-hagency-authorization-window-ms";
/// The header of a client that shares one local session per server session.
const SECOND: &str = "x-hagency-authorization-window-v2-ms";
/// How far ahead a renewal with these request headers left the session valid.
async fn renewed_ahead(app: &Service, token: &str, headers: &[(&'static str, &str)]) -> i64 {
    let mut request =
        TestClient::post("https://example.test/hagency/api/v1/sessions/current/renew")
            .add_header("host", "example.test", true)
            .bearer_auth(token);
    for (name, value) in headers {
        request = request.add_header(*name, *value, true);
    }
    let mut response = request
        .json(&json!({"accessToken":"synthetic-fresh-upstream-token"}))
        .send(app)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let renewed: Value = response.take_json().await.unwrap();
    renewed["validUntilMs"].as_i64().unwrap() - now_ms()
}
/// How far ahead a sign-in with these request headers left its new session
/// valid. Each sign-in is a session of its own, so no earlier end is kept.
async fn signed_in_ahead(app: &Service, headers: &[(&'static str, &str)]) -> i64 {
    let mut request = TestClient::post("https://example.test/hagency/api/v1/sessions/pasion")
        .add_header("host", "example.test", true);
    for (name, value) in headers {
        request = request.add_header(*name, *value, true);
    }
    let mut response = request
        .json(&json!({"accessToken":"synthetic-fresh-upstream-token"}))
        .send(app)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let grant: Value = response.take_json().await.unwrap();
    grant["validUntilMs"].as_i64().unwrap() - now_ms()
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_longer_authorization_window_needs_the_host_and_the_second_header() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("window_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let (upstream, task) = upstream(name, Arc::new(AtomicBool::new(false))).await;
    // A host that was not configured grants thirty seconds whatever is asked.
    let default = service(store.clone(), &upstream);
    let ahead = renewed_ahead(&default, &owner.token, &[(SECOND, "120000")]).await;
    assert!((1..=30_000).contains(&ahead), "{ahead}");
    // A configured host grants more only to a request that asks with the
    // second header, never more than was asked nor more than the host is
    // configured for, and thirty seconds to a request it cannot read.
    let longer = service_within(store.clone(), &upstream, 90_000);
    let ahead = renewed_ahead(&longer, &owner.token, &[]).await;
    assert!((1..=30_000).contains(&ahead), "{ahead}");
    let ahead = renewed_ahead(&longer, &owner.token, &[(SECOND, "soon")]).await;
    assert!((1..=30_000).contains(&ahead), "{ahead}");
    let ahead = renewed_ahead(&longer, &owner.token, &[(SECOND, "60000")]).await;
    assert!((40_000..=60_000).contains(&ahead), "{ahead}");
    let ahead = renewed_ahead(&longer, &owner.token, &[(SECOND, "120000")]).await;
    assert!((70_000..=90_000).contains(&ahead), "{ahead}");
    let ahead = signed_in_ahead(&longer, &[(SECOND, "120000")]).await;
    assert!((70_000..=90_000).contains(&ahead), "{ahead}");
    let longest = service_within(store.clone(), &upstream, 120_000);
    let ahead = renewed_ahead(&longest, &owner.token, &[(SECOND, "120000")]).await;
    assert!((100_000..=120_000).contains(&ahead), "{ahead}");
    // A renewal never shortens a session. One that asks for less, cannot be
    // read, asks for nothing or only with the first header, or reaches a
    // host that grants less is answered with the end the session already has.
    for (app, headers) in [
        (&longest, &[(SECOND, "60000")][..]),
        (&longest, &[(SECOND, "soon")][..]),
        (&longest, &[][..]),
        (&longest, &[(FIRST, "120000")][..]),
        (&default, &[(SECOND, "120000")][..]),
    ] {
        let ahead = renewed_ahead(app, &owner.token, headers).await;
        assert!((100_000..=120_000).contains(&ahead), "{headers:?}: {ahead}");
    }
    store.sign_out(&owner.token).await.unwrap();
    task.abort();
}
/// The first clients to ask for a longer window (Hagency Desktop
/// 0.5.0-rc.19) make one local session per call and refuse their own calls
/// under one. The header they send still gives the default window.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_first_window_header_alone_keeps_the_default_window() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("window_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let (upstream, task) = upstream(name, Arc::new(AtomicBool::new(false))).await;
    let longest = service_within(store.clone(), &upstream, 120_000);
    for window in ["60000", "120000"] {
        let ahead = signed_in_ahead(&longest, &[(FIRST, window)]).await;
        assert!((1..=30_000).contains(&ahead), "sign-in {window}: {ahead}");
        let ahead = renewed_ahead(&longest, &owner.token, &[(FIRST, window)]).await;
        assert!((1..=30_000).contains(&ahead), "renewal {window}: {ahead}");
    }
    store.sign_out(&owner.token).await.unwrap();
    task.abort();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_second_window_header_decides_when_both_are_sent() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("window_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let (upstream, task) = upstream(name, Arc::new(AtomicBool::new(false))).await;
    let longest = service_within(store.clone(), &upstream, 120_000);
    let shorter = [(FIRST, "120000"), (SECOND, "60000")];
    let ahead = signed_in_ahead(&longest, &shorter).await;
    assert!((40_000..=60_000).contains(&ahead), "{ahead}");
    let ahead = renewed_ahead(&longest, &owner.token, &shorter).await;
    assert!((40_000..=60_000).contains(&ahead), "{ahead}");
    let longer = [(SECOND, "120000"), (FIRST, "30000")];
    let ahead = signed_in_ahead(&longest, &longer).await;
    assert!((100_000..=120_000).contains(&ahead), "{ahead}");
    let ahead = renewed_ahead(&longest, &owner.token, &longer).await;
    assert!((100_000..=120_000).contains(&ahead), "{ahead}");
    store.sign_out(&owner.token).await.unwrap();
    task.abort();
}
/// A second header whose value cannot be read gives the default window, as
/// the first header did, also beside a first header that asks for more.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_unreadable_second_window_header_keeps_the_default_window() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("window_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let (upstream, task) = upstream(name, Arc::new(AtomicBool::new(false))).await;
    let longest = service_within(store.clone(), &upstream, 120_000);
    for window in ["soon", "120000ms", "99999999999999999999", "1.5e5"] {
        let headers = [(FIRST, "120000"), (SECOND, window)];
        let ahead = signed_in_ahead(&longest, &headers).await;
        assert!((1..=30_000).contains(&ahead), "sign-in {window}: {ahead}");
        let ahead = renewed_ahead(&longest, &owner.token, &headers).await;
        assert!((1..=30_000).contains(&ahead), "renewal {window}: {ahead}");
    }
    store.sign_out(&owner.token).await.unwrap();
    task.abort();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_expired_original_session_http_renew_preserves_exact_device_and_rejects_changed_identity()
 {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("resume_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let device = register(&store, &owner.token, &format!("installation_{name}")).await;
    let original = store
        .authenticate(&device.token, now_ms(), true)
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=$1 WHERE id=$2")
        .bind::<BigInt, _>(now_ms() - 1)
        .bind::<Text, _>(&original.session_id)
        .execute(&mut db)
        .await
        .unwrap();
    drop(db);
    let before = counts(&url, &owner.user_id).await;
    let wrong = Arc::new(AtomicBool::new(true));
    let (upstream, task) = upstream(name, wrong.clone()).await;
    let app = service(store.clone(), &upstream);
    current(&app, &device.token, "", StatusCode::UNAUTHORIZED).await;
    renew(&app, &owner.token, StatusCode::CONFLICT).await;
    current(&app, &device.token, "", StatusCode::UNAUTHORIZED).await;
    wrong.store(false, Ordering::Release);
    assert!(
        renew(&app, &owner.token, StatusCode::OK).await["validUntilMs"]
            .as_i64()
            .unwrap()
            > now_ms()
    );
    let proof = current(&app, &device.token, "", StatusCode::OK).await;
    assert_eq!(proof["deviceId"], device.device_id);
    assert_eq!(proof["generation"], device.generation);
    let fresh = store
        .authenticate(&device.token, now_ms(), true)
        .await
        .unwrap();
    assert_eq!(fresh.session_id, original.session_id);
    assert_eq!(
        counts(&url, &owner.user_id).await,
        before,
        "renewal preserves original rows"
    );
    store.sign_out(&owner.token).await.unwrap();
    renew(&app, &owner.token, StatusCode::UNAUTHORIZED).await;
    current(&app, &device.token, "", StatusCode::UNAUTHORIZED).await;
    task.abort();
}
/// The header by which a client says that it follows its window.
const FOLLOW: (&str, &str) = ("x-hagency-activity-window", "session");
/// Renews as a client that sends these request headers, and returns the
/// window the session then follows for the activity leases issued to its
/// device, if any.
async fn activity_window_after_renewal(
    app: &Service,
    url: &str,
    token: &str,
    headers: &[(&'static str, &str)],
) -> Option<i64> {
    #[derive(diesel::QueryableByName)]
    struct Row {
        #[diesel(sql_type=diesel::sql_types::Nullable<BigInt>)]
        activity_window_ms: Option<i64>,
    }
    let mut request =
        TestClient::post("https://example.test/hagency/api/v1/sessions/current/renew")
            .add_header("host", "example.test", true)
            .bearer_auth(token);
    for (name, value) in headers {
        request = request.add_header(*name, *value, true);
    }
    let response = request
        .json(&json!({"accessToken":"synthetic-fresh-upstream-token"}))
        .send(app)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    sql_query("SELECT activity_window_ms FROM hagency_agent_v1.sessions WHERE token_hash=$1")
        .bind::<Text, _>(crate::hash(token))
        .get_result::<Row>(&mut db)
        .await
        .unwrap()
        .activity_window_ms
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_activity_window_is_the_granted_window_only_for_a_client_that_follows_it() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", ISSUER).await.unwrap();
    let name = format!("window_{}", secret_token());
    let owner = store.sign_in(identity(&name), now_ms()).await.unwrap();
    let (upstream, task) = upstream(name, Arc::new(AtomicBool::new(false))).await;
    let longer = service_within(store.clone(), &upstream, 120_000);
    let kept = |app, headers| activity_window_after_renewal(app, &url, &owner.token, headers);
    // A client that asks for the longer window and says nothing more checks
    // a lease against thirty seconds: it follows no window.
    assert_eq!(kept(&longer, &[(SECOND, "120000")]).await, None);
    // A client that follows its window follows what it was granted, no more.
    assert_eq!(
        kept(&longer, &[(SECOND, "120000"), FOLLOW]).await,
        Some(120_000)
    );
    assert_eq!(
        kept(&longer, &[(SECOND, "60000"), FOLLOW]).await,
        Some(60_000)
    );
    // One that asked for no longer window follows the thirty seconds it has.
    // That is not the same as following none: see `activity_until`.
    assert_eq!(kept(&longer, &[FOLLOW]).await, Some(30_000));
    // So does one that asked with the first header alone: it is granted the
    // default window (`requested_window`), and follows no more than that.
    assert_eq!(
        kept(&longer, &[(FIRST, "120000"), FOLLOW]).await,
        Some(30_000)
    );
    // Any other value of the header is not that statement.
    assert_eq!(
        kept(
            &longer,
            &[(SECOND, "120000"), ("x-hagency-activity-window", "120000")]
        )
        .await,
        None
    );
    // A host that grants thirty seconds has nothing longer to follow.
    let default = service(store.clone(), &upstream);
    assert_eq!(
        kept(&default, &[(SECOND, "120000"), FOLLOW]).await,
        Some(30_000)
    );
    // Each renewal says it again: one without it follows none.
    assert_eq!(
        kept(&longer, &[(SECOND, "120000"), FOLLOW]).await,
        Some(120_000)
    );
    assert_eq!(kept(&longer, &[(SECOND, "120000")]).await, None);
    store.sign_out(&owner.token).await.unwrap();
    task.abort();
}
