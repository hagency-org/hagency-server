//! Isolated Matrix login interleavings. Every test persists the claim before
//! releasing the mock homeserver, rather than relying on scheduler timing.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use super::*;

// Cleanup scans are deliberately global and fair. Give each interleaving its
// own database so unrelated retiring fixtures cannot consume its bounded batch.
async fn fixture() -> Fixture {
    let source =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let name = format!("hagency_nonce_{}", &hash(&secret_token())[..24]);
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    drop(control);
    let mut url = url::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    // As with existing isolated fixtures, the test-cluster owner removes child
    // databases after the complete suite, including any failed-case evidence.
    fixture_at(url.as_str()).await
}

pub(crate) struct LoginServer {
    pub(crate) matrix: crate::matrix_client::MatrixClient,
    started: Arc<Notify>,
    pub(crate) resume: Arc<Notify>,
    fail_logout: Arc<AtomicBool>,
    hold_first_logout: Arc<AtomicBool>,
    logout_started: Arc<Notify>,
    logout_resume: Arc<Notify>,
    logins: Arc<AtomicUsize>,
    logouts: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for LoginServer {
    fn drop(&mut self) {
        self.resume.notify_one();
        self.logout_resume.notify_one();
        self.task.abort();
    }
}
impl LoginServer {
    pub(crate) async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let started = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let fail_logout = Arc::new(AtomicBool::new(false));
        let hold_first_logout = Arc::new(AtomicBool::new(false));
        let logout_started = Arc::new(Notify::new());
        let logout_resume = Arc::new(Notify::new());
        let logins = Arc::new(AtomicUsize::new(0));
        let logouts = Arc::new(AtomicUsize::new(0));
        let (s, r, f, l, o, h, ls, lr) = (
            started.clone(),
            resume.clone(),
            fail_logout.clone(),
            logins.clone(),
            logouts.clone(),
            hold_first_logout.clone(),
            logout_started.clone(),
            logout_resume.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (s, r, f, l, o, h, ls, lr) = (
                    s.clone(),
                    r.clone(),
                    f.clone(),
                    l.clone(),
                    o.clone(),
                    h.clone(),
                    ls.clone(),
                    lr.clone(),
                );
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut buf = [0; 4096];
                    let (header_end, length) = loop {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        data.extend_from_slice(&buf[..n]);
                        if let Some(i) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&data[..i]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            break (i + 4, length);
                        }
                    };
                    while data.len() < header_end + length {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        data.extend_from_slice(&buf[..n]);
                    }
                    let path = String::from_utf8_lossy(&data[..header_end]);
                    let (status, body) = if path.starts_with("POST /_matrix/client/v3/login ") {
                        let body: serde_json::Value =
                            serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
                        l.fetch_add(1, Ordering::SeqCst);
                        s.notify_one();
                        r.notified().await;
                        (
                            200,
                            json!({"user_id":format!("@{}:example.test",body["identifier"]["user"].as_str().unwrap()),"device_id":body["device_id"],"access_token":"test-issued-token"}),
                        )
                    } else if path.starts_with("POST /_matrix/client/v3/logout?") {
                        let count = o.fetch_add(1, Ordering::SeqCst);
                        let fail = f.as_ref().load(Ordering::SeqCst);
                        if count == 0 && h.as_ref().load(Ordering::SeqCst) {
                            ls.notify_one();
                            lr.notified().await;
                        }
                        if fail {
                            (
                                500,
                                json!({"errcode":"M_UNKNOWN","error":"injected logout failure"}),
                            )
                        } else {
                            (200, json!({}))
                        }
                    } else {
                        panic!("unexpected Matrix request {path}");
                    };
                    let body = serde_json::to_vec(&body).unwrap();
                    let headers = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self {
            matrix: crate::matrix_client::MatrixClient::new(
                url::Url::parse(&origin).unwrap(),
                "test-only".into(),
                "example.test".into(),
            )
            .unwrap(),
            started,
            resume,
            fail_logout,
            hold_first_logout,
            logout_started,
            logout_resume,
            logins,
            logouts,
            task,
        }
    }
    pub(crate) async fn wait_login(&self) {
        timeout(Duration::from_secs(3), self.started.notified())
            .await
            .expect("login must start");
    }
}
#[derive(diesel::QueryableByName)]
struct DeviceState {
    #[diesel(sql_type=Bool)]
    revoked: bool,
    #[diesel(sql_type=Bool)]
    retiring: bool,
    #[diesel(sql_type=Bool)]
    pending: bool,
}
async fn state(f: &Fixture, id: &str) -> DeviceState {
    let mut db = f.transport.db.test_connection().await.unwrap();
    sql_query("SELECT revoked,retiring,issuance_nonce IS NOT NULL AS pending FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND device_id=$2")
        .bind::<Text,_>(&f.agent).bind::<Text,_>(id).get_result(&mut *db).await.unwrap()
}
async fn expire(f: &Fixture, id: &str) {
    let mut db = f.transport.db.test_connection().await.unwrap();
    sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET issuance_until_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1 WHERE scope_id=$1 AND device_id=$2")
        .bind::<Text,_>(&f.agent).bind::<Text,_>(id).execute(&mut *db).await.unwrap();
}
async fn lease(f: &Fixture) -> LeaseRef {
    let v = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, crate::api::now_ms())
        .await
        .unwrap();
    LeaseRef {
        agent_id: v.agent_id,
        epoch: v.epoch,
    }
}
fn begin(
    f: &Fixture,
    lease: &LeaseRef,
    server: &LoginServer,
    id: &'static str,
) -> tokio::task::JoinHandle<Result<(String, String)>> {
    let store = f.transport.clone();
    let p = Principal {
        user_id: f.p.user_id.clone(),
        mxid: f.p.mxid.clone(),
        subject: f.p.subject.clone(),
        session_id: f.p.session_id.clone(),
        client_id: f.p.client_id.clone(),
        valid_until_ms: f.p.valid_until_ms,
        device_id: f.p.device_id.clone(),
        device_generation: f.p.device_generation,
    };
    let lease = lease.clone();
    let matrix = server.matrix.clone();
    let puppet = f.facts.puppet_mxid.clone();
    tokio::spawn(async move {
        store
            .issue_matrix_device(&p, &lease, &puppet, id, &matrix)
            .await
    })
}
async fn cleanup_until_revoked(f: &Fixture, id: &str, server: &LoginServer) {
    for _ in 0..32 {
        let _ = f
            .transport
            .revoke_stale_matrix_devices(&server.matrix)
            .await;
        if state(f, id).await.revoked {
            return;
        }
    }
    panic!("fair cleanup did not reach device");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn revoke_during_login_rejects_token_and_persists_failed_logout() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EEMIDLOGINREVOKE";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    assert!(state(&f, id).await.pending, "claim precedes Matrix IO");
    // This must complete while login is still blocked: neither the shared DB
    // connection nor the authorization advisory lock may span Matrix IO.
    timeout(
        Duration::from_secs(2),
        f.auth.revoke_device(
            &f.credential,
            f.p.device_id.as_deref().unwrap(),
            crate::api::now_ms(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    server.fail_logout.store(true, Ordering::SeqCst);
    server.resume.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized(_))));
    let saved = state(&f, id).await;
    assert!(saved.retiring && !saved.revoked && !saved.pending);
    assert!(server.logouts.as_ref().load(Ordering::SeqCst) > 0);
    server.fail_logout.store(false, Ordering::SeqCst);
    cleanup_until_revoked(&f, id, &server).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn concurrent_refresh_is_serialized_without_holding_a_database_connection() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EESERIALREFRESH";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    let result = timeout(
        Duration::from_secs(2),
        f.transport
            .issue_matrix_device(&f.p, &lease, &f.facts.puppet_mxid, id, &server.matrix),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(Error::Conflict("matrix_device_issuance_pending"))
    ));
    assert_eq!(server.logins.as_ref().load(Ordering::SeqCst), 1);
    server.resume.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        (id.to_owned(), "test-issued-token".to_owned())
    );
    let saved = state(&f, id).await;
    assert!(!saved.retiring && !saved.revoked && !saved.pending);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn late_login_restores_cleanup_after_a_revoker_already_logged_out() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EELATELOGIN";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    expire(&f, id).await;
    cleanup_until_revoked(&f, id, &server).await;
    assert!(
        state(&f, id).await.pending,
        "late completion must still own its cleanup nonce"
    );
    server.fail_logout.store(true, Ordering::SeqCst);
    server.resume.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized(_))));
    let saved = state(&f, id).await;
    assert!(
        saved.retiring && !saved.revoked && !saved.pending,
        "late login must restore the durable cleanup obligation"
    );
    server.fail_logout.store(false, Ordering::SeqCst);
    cleanup_until_revoked(&f, id, &server).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cancelled_login_claim_is_retired_and_never_reuses_its_olm_identity() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EECANCELLEDLOGIN";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(state(&f, id).await.pending);
    expire(&f, id).await;
    cleanup_until_revoked(&f, id, &server).await;
    let mut db = f.transport.db.test_connection().await.unwrap();
    let (device, generation) = device(&f.p).unwrap();
    let next = crate::executor_matrix_device::select(&mut db, &f.agent, device, generation, id)
        .await
        .unwrap();
    assert_ne!(
        next, id,
        "an uncertain login must not reuse a potentially deleted Olm identity"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn an_older_logout_completion_cannot_erase_a_late_login_cleanup_obligation() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EELOGOUTORDER";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    expire(&f, id).await;
    server.hold_first_logout.store(true, Ordering::SeqCst);
    let store = f.transport.clone();
    let matrix = server.matrix.clone();
    let cleanup = tokio::spawn(async move { store.revoke_stale_matrix_devices(&matrix).await });
    timeout(Duration::from_secs(3), server.logout_started.notified())
        .await
        .unwrap();
    // The earlier logout has processed successfully, but its response is delayed.
    // Login then finishes and the newer retirement's logout deliberately fails.
    server.fail_logout.store(true, Ordering::SeqCst);
    server.resume.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized(_))));
    let saved = state(&f, id).await;
    assert!(saved.retiring && !saved.revoked && !saved.pending);
    server.logout_resume.notify_one();
    cleanup.await.unwrap().unwrap();
    assert!(
        !state(&f, id).await.revoked,
        "the earlier logout may not acknowledge a newer cleanup generation"
    );
    server.fail_logout.store(false, Ordering::SeqCst);
    cleanup_until_revoked(&f, id, &server).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn executor_reassignment_during_login_rejects_the_previous_device() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let server = LoginServer::new().await;
    let id = "HAGE2EEREASSIGNLOGIN";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    let current = f
        .domain
        .agent(&f.second, &f.agent, crate::api::now_ms())
        .await
        .unwrap();
    timeout(
        Duration::from_secs(2),
        f.domain.set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: current.generation,
            },
            crate::api::now_ms(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    server.resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Unauthorized("execution_device_required"))
    ));
    assert!(
        state(&f, id).await.revoked,
        "a completed but no longer authorized login is cleaned up"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn lease_expiry_while_login_is_in_flight_prevents_token_release() {
    let f = fixture().await;
    let server = LoginServer::new().await;
    let value = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 1000, false, crate::api::now_ms())
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: value.agent_id,
        epoch: value.epoch,
    };
    let id = "HAGE2EEEXPIREDLEASE";
    let task = begin(&f, &lease, &server, id);
    server.wait_login().await;
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    server.resume.notify_one();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized(_))));
    assert!(state(&f, id).await.revoked);
}
