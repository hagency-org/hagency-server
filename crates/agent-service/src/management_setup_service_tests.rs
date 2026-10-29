//! Actual loopback Matrix join requests, with PostgreSQL authorization between awaits.
use super::*;
use crate::Error;
use crate::matrix_client::MatrixClient;
use std::sync::{Mutex, atomic::AtomicBool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Matrix {
    gateway: Gateway,
    client: MatrixClient,
    states: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    joins: Arc<Mutex<Vec<String>>>,
    pause: Arc<AtomicBool>,
    started: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    lose: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Matrix {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Matrix {
    async fn new(s: &ProjectSetup, input: &ConfirmProjectSetup) -> Self {
        let mut data = states(s, input);
        for events in data.values_mut() {
            for e in events {
                if e["type"] == "m.room.member" && e["state_key"] == SERVICE {
                    e["content"]["membership"] = json!("invite");
                }
            }
        }
        let states = Arc::new(Mutex::new(data));
        let reads = states.clone();
        let gateway = Gateway::new(
            Arc::new(move |id| {
                let reads = reads.clone();
                Box::pin(async move {
                    reads
                        .lock()
                        .unwrap()
                        .get(&id)
                        .cloned()
                        .ok_or(Error::NotFound("fixture_room"))
                })
            }),
            SERVICE.into(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let joins = Arc::new(Mutex::new(Vec::new()));
        let pause = Arc::new(AtomicBool::new(false));
        let started = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let lose = Arc::new(AtomicBool::new(false));
        let (data, calls, hold, begin, finish, loss) = (
            states.clone(),
            joins.clone(),
            pause.clone(),
            started.clone(),
            resume.clone(),
            lose.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (data, calls, hold, begin, finish, loss) = (
                    data.clone(),
                    calls.clone(),
                    hold.clone(),
                    begin.clone(),
                    finish.clone(),
                    loss.clone(),
                );
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    loop {
                        let mut b = [0; 4096];
                        let n = socket.read(&mut b).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&b[..n]);
                        if raw.windows(4).any(|v| v == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let header = String::from_utf8_lossy(&raw);
                    assert!(header.starts_with("POST "));
                    let target = header.split_whitespace().nth(1).unwrap();
                    let url = Url::parse(&format!("http://fixture{target}")).unwrap();
                    assert_eq!(
                        url.query_pairs().find(|(k, _)| k == "user_id").unwrap().1,
                        SERVICE
                    );
                    let parts = url.path_segments().unwrap().collect::<Vec<_>>();
                    assert_eq!(&parts[..4], &["_matrix", "client", "v3", "join"]);
                    let room = percent_encoding::percent_decode_str(parts[4])
                        .decode_utf8()
                        .unwrap()
                        .into_owned();
                    calls.lock().unwrap().push(room.clone());
                    begin.notify_one();
                    if hold.as_ref().load(Ordering::SeqCst) {
                        finish.notified().await;
                    }
                    for e in data.lock().unwrap().get_mut(&room).unwrap() {
                        if e["type"] == "m.room.member" && e["state_key"] == SERVICE {
                            e["content"]["membership"] = json!("join");
                        }
                    }
                    if loss.swap(false, Ordering::SeqCst) {
                        return;
                    }
                    let body = json!({"room_id":room}).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            gateway,
            client: MatrixClient::new(origin, "fixture-only".into(), "example.test".into())
                .unwrap(),
            states,
            joins,
            pause,
            started,
            resume,
            lose,
            task,
        }
    }
}
fn app(f: &Fixture, m: &Matrix) -> Service {
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
    Service::new(
        App::new(
            f.auth.clone(),
            verifier,
            origin,
            issuer,
            "example.test".into(),
        )
        .with_domain(f.domain.clone(), m.gateway.clone())
        .with_management(f.policy.clone())
        .with_matrix_client(m.client.clone())
        .router(),
    )
}
async fn post(
    f: &Fixture,
    m: &Matrix,
    s: &ProjectSetup,
    input: &ConfirmProjectSetup,
) -> (StatusCode, Value) {
    let mut res = TestClient::post(format!(
        "http://example.test/hagency/api/v1/management/project-setups/{}/confirm",
        s.id
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(&f.outsider.token)
    .json(input)
    .send(&app(f, m))
    .await;
    let status = res.status_code.unwrap();
    let value = res.take_json().await.unwrap();
    (status, value)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_service_join_actual_http_then_confirm_replay_has_no_extra_joins() {
    let (f, _, _, s, _) = begun().await;
    let input = confirm(&s);
    let m = Matrix::new(&s, &input).await;
    let (status, value) = post(&f, &m, &s, &input).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["readiness"]["matrixReady"], true);
    assert_eq!(
        *m.joins.lock().unwrap(),
        vec![input.space_id.clone(), input.room_id.clone()]
    );
    assert_eq!(post(&f, &m, &s, &input).await.0, StatusCode::OK);
    assert_eq!(m.joins.lock().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_service_join_lost_reply_recovers_same_rooms_without_second_join() {
    let (f, _, _, s, _) = begun().await;
    let input = confirm(&s);
    let m = Matrix::new(&s, &input).await;
    m.lose.store(true, Ordering::SeqCst);
    let (_, value) = post(&f, &m, &s, &input).await;
    assert_eq!(value["code"], "project_setup_membership_outcome_unknown");
    assert_eq!(post(&f, &m, &s, &input).await.0, StatusCode::OK);
    assert_eq!(
        *m.joins.lock().unwrap(),
        vec![input.space_id.clone(), input.room_id.clone()]
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_service_join_rejects_roles_stale_markers_admin_and_revoked_capacity() {
    let (f, _, plan, s, admin) = begun().await;
    let input = confirm(&s);
    let m = Matrix::new(&s, &input).await;
    for actor in [&f.owner, &f.coordinator, &f.admin] {
        assert!(
            f.domain
                .accept_management_setup_service(
                    &actor.p,
                    &f.policy,
                    &s.id,
                    &input,
                    &m.gateway,
                    Some(&m.client)
                )
                .await
                .is_err()
        );
    }
    let mut stale = input.clone();
    stale.expected_revision += 1;
    assert!(
        f.domain
            .accept_management_setup_service(
                &f.outsider.p,
                &f.policy,
                &s.id,
                &stale,
                &m.gateway,
                Some(&m.client)
            )
            .await
            .is_err()
    );
    let original = m.states.lock().unwrap().clone();
    for events in m.states.lock().unwrap().values_mut() {
        for e in events {
            if e["type"] == "im.hagency.management.project_setup" {
                e["content"]["definitionDigest"] = json!("0".repeat(64));
            }
        }
    }
    assert!(post(&f, &m, &s, &input).await.0.is_client_error());
    *m.states.lock().unwrap() = original;
    for events in m.states.lock().unwrap().values_mut() {
        for e in events {
            if e["type"] == "m.room.member" && e["state_key"] == admin.p.mxid {
                e["content"]["membership"] = json!("invite");
            }
        }
    }
    assert!(post(&f, &m, &s, &input).await.0.is_client_error());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_projects SET state='revoked' WHERE id=$1")
        .bind::<Text, _>(&plan.id)
        .execute(&mut db)
        .await
        .unwrap();
    drop(db);
    assert!(post(&f, &m, &s, &input).await.0.is_client_error());
    assert!(m.joins.lock().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_service_join_cancel_during_actual_join_never_confirms_or_joins_second_room() {
    let (f, _, p, s, _) = begun().await;
    let input = confirm(&s);
    let m = Matrix::new(&s, &input).await;
    m.pause.store(true, Ordering::SeqCst);
    let request = post(&f, &m, &s, &input);
    let cancel = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), m.started.notified()).await.unwrap();
        f.domain
            .cancel_management_project_setup(
                &f.outsider.p,
                &f.policy,
                &s.id,
                CancelProjectSetup {
                    command_id: "cancel_while_join".into(),
                    expected_revision: s.revision,
                },
            )
            .await
            .unwrap();
        m.resume.notify_one();
    };
    let ((_, value), ()) = tokio::join!(request, cancel);
    assert_eq!(value["code"], "project_setup_membership_outcome_unknown");
    let r = read(&f, &p).await;
    assert!(!r.matrix_ready);
    assert_eq!(r.setup.unwrap().state, "cancelled");
    assert_eq!(m.joins.lock().unwrap().len(), 1);
    assert!(post(&f, &m, &s, &input).await.0.is_client_error());
    assert_eq!(m.joins.lock().unwrap().len(), 1);
}
