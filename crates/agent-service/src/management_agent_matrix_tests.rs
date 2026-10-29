use super::*;
#[path="management_agent_runtime_tests.rs"]
mod runtime;
use crate::management_agent_matrix_types::*;
use crate::matrix_client::MatrixClient;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    runtime::remove_migration(db).await;
    db.batch_execute("DROP TRIGGER queue_managed_matrix_identity ON hagency_agent_v1.agents;DROP TRIGGER queue_managed_matrix ON hagency_agent_v1.management_shared_assignments;DROP TABLE hagency_agent_v1.management_agent_matrix_checks,hagency_agent_v1.management_agent_matrix;DROP FUNCTION hagency_agent_v1.queue_managed_matrix(),hagency_agent_v1.retain_managed_matrix();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=9;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=9)").await.unwrap();
}

struct MatrixFixture {
    gateway: Gateway,
    matrix: MatrixClient,
    states: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    calls: Arc<Mutex<Vec<String>>>,
    join_started: Arc<tokio::sync::Notify>,
    join_continue: Arc<tokio::sync::Notify>,
    hold_join: Arc<std::sync::atomic::AtomicBool>,
    http: tokio::task::JoinHandle<()>,
}
impl Drop for MatrixFixture {
    fn drop(&mut self) {
        self.http.abort();
    }
}
fn member(states: &mut HashMap<String, Vec<Value>>, room: &str, puppet: &str, value: &str) {
    let events = states.get_mut(room).unwrap();
    events.retain(|e| !(e["type"] == "m.room.member" && e["state_key"] == puppet));
    events.push(json!({"type":"m.room.member","state_key":puppet,"content":{"membership":value}}));
}
async fn matrix_fixture(f: &Fixture, r: &ManagedAgentRequest) -> MatrixFixture {
    let setup = f
        .domain
        .management_project_setup(&f.outsider.p, &f.policy, &r.project_id)
        .await
        .unwrap()
        .setup
        .unwrap();
    matrix_fixture_from_states(states(&setup, &confirm(&setup))).await
}
async fn matrix_fixture_from_states(states: HashMap<String,Vec<Value>>) -> MatrixFixture {
    let data = Arc::new(Mutex::new(states));
    let reads = data.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let reads = reads.clone();
            Box::pin(async move {
                reads
                    .lock()
                    .unwrap()
                    .get(&room)
                    .cloned()
                    .ok_or(crate::Error::NotFound("fixture_room_missing"))
            })
        }),
        SERVICE.into(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let state = data.clone();
    let join_started = Arc::new(tokio::sync::Notify::new());
    let join_continue = Arc::new(tokio::sync::Notify::new());
    let hold_join = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (started, resume, hold) = (
        join_started.clone(),
        join_continue.clone(),
        hold_join.clone(),
    );
    let http = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (calls, state, started, resume, hold) = (
                captured.clone(),
                state.clone(),
                started.clone(),
                resume.clone(),
                hold.clone(),
            );
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let headers;
                loop {
                    let mut b = [0; 4096];
                    let n = socket.read(&mut b).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&b[..n]);
                    if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        headers = i + 4;
                        break;
                    }
                }
                let header = String::from_utf8(raw[..headers].to_vec()).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while raw.len() < headers + length {
                    let mut b = [0; 4096];
                    let n = socket.read(&mut b).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&b[..n]);
                }
                let target = header.split_whitespace().nth(1).unwrap();
                let url = Url::parse(&format!("http://localhost{target}")).unwrap();
                let parts: Vec<String> = url
                    .path_segments()
                    .unwrap()
                    .map(|s| {
                        percent_encoding::percent_decode_str(s)
                            .decode_utf8()
                            .unwrap()
                            .into_owned()
                    })
                    .collect();
                let user = url
                    .query_pairs()
                    .find(|(k, _)| k == "user_id")
                    .unwrap()
                    .1
                    .into_owned();
                calls.lock().unwrap().push(parts.join("/"));
                let mut result = json!({});
                if parts.last().map(String::as_str) == Some("whoami") {
                    result = json!({"user_id":user});
                } else if parts.get(3).map(String::as_str) == Some("join") {
                    started.notify_one();
                    if hold.as_ref().load(Ordering::SeqCst) {
                        resume.notified().await;
                    }
                    member(&mut state.lock().unwrap(), &parts[4], &user, "join");
                    result = json!({"room_id":parts[4]});
                } else if parts.last().map(String::as_str) == Some("invite") {
                    let body: Value =
                        serde_json::from_slice(&raw[headers..headers + length]).unwrap();
                    member(
                        &mut state.lock().unwrap(),
                        &parts[4],
                        body["user_id"].as_str().unwrap(),
                        "invite",
                    );
                } else if parts.last().map(String::as_str) == Some("leave") {
                    member(&mut state.lock().unwrap(), &parts[4], &user, "leave");
                }
                let body = serde_json::to_string(&result).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    MatrixFixture {
        gateway,
        matrix: MatrixClient::new(
            format!("http://{address}").parse().unwrap(),
            "fixture-only-AS".into(),
            "example.test".into(),
        )
        .unwrap(),
        states: data,
        calls,
        join_started,
        join_continue,
        hold_join,
        http,
    }
}
async fn prepared_matrix() -> (
    Fixture,
    ManagedAgentRequest,
    Actor,
    PendingManagedAssignment,
) {
    let (mut f, r, a) = approved().await;
    f.policy = f.policy.with_agent_matrix_provisioning(true);
    let d = poll(&f).await;
    let assignment = submit(&f, &d, reservation(&d))
        .await
        .unwrap()
        .assignment
        .unwrap();
    (f, r, a, assignment)
}
async fn matrix_poll(f: &Fixture) -> AgentMatrixDelivery {
    f.domain
        .poll_managed_agent_matrix(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
fn observation(d: &AgentMatrixDelivery, eligible: bool) -> AgentMatrixResultInput {
    let c = &d.command;
    AgentMatrixResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        observation: ManagedAgentMatrixObservation {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            assignment_id: c.assignment_id.clone(),
            agent_reservation_id: c.agent_reservation_id.clone(),
            reserve_command_digest: c.reserve_command_digest.clone(),
            reserve_receipt_digest: c.reserve_receipt_digest.clone(),
            observed_ledger_revision: 100,
            eligible,
            reason: (!eligible).then(|| "native_reservation_unavailable".into()),
            valid_until_ms: c.expires_at_ms,
        },
    }
}
async fn permit(f: &Fixture) -> AgentMatrixDelivery {
    let d = matrix_poll(f).await;
    f.domain
        .record_managed_agent_matrix(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            observation(&d, true),
        )
        .await
        .unwrap();
    d
}
async fn read_matrix(f: &Fixture, r: &ManagedAgentRequest) -> AgentMatrixReadiness {
    f.domain
        .managed_agent_matrix(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_command_cursor_renews_expired_native_check() {
    let (f, _, _, assignment) = prepared_matrix().await;
    let first = permit(&f).await;
    // Let the immutable challenge expire as it does after a failed Matrix call.
    // Keep its signed payload and original observation intact.
    tokio::time::sleep(std::time::Duration::from_millis(
        (first.command.expires_at_ms - now_ms()).max(0) as u64 + 10,
    )).await;
    let renewed = f.domain.poll_managed_agent_matrix(&f.owner.p, &f.policy, ExecutorPollInput {
        after: Some(first.command.command_id.clone()),
    }).await.unwrap().expect("the Desktop command cursor must wrap to the expired assignment");
    assert_eq!(renewed.command.assignment_id, assignment.id);
    assert_ne!(renewed.command.command_id, first.command.command_id);
    assert!(renewed.execution_allowed);
    assert!(renewed.command.expires_at_ms > now_ms());
    // Freshly authorized checks must not immediately reserve another ticket.
    f.domain.record_managed_agent_matrix(&f.owner.p, &f.policy,
        &renewed.command.command_id, observation(&renewed, true)).await.unwrap();
    assert!(f.domain.poll_managed_agent_matrix(&f.owner.p, &f.policy, ExecutorPollInput {
        after: Some(renewed.command.command_id),
    }).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_real_http_membership_never_enables_runtime_or_personal_binding() {
    let (f, r, _, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    let d = permit(&f).await;
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    let state = read_matrix(&f, &r).await;
    assert_eq!(state.state, "matrix_verified");
    assert_eq!(state.runtime_state, "unsupported");
    assert!(!state.work_available);
    assert!(state.verified_at_ms.is_some());
    assert!(m.calls.lock().unwrap().iter().any(|s| s.contains("/join/")));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT a.state='suspended' AND a.owner_user_id=$2 AND a.execution_device_id IS NULL AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id) AS matched FROM hagency_agent_v1.agents a WHERE a.id=$1").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&f.outsider.p.user_id).get_result::<Check>(&mut db).await.unwrap().matched);
    let old = f
        .domain
        .recover_managed_agent_matrix(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest,
            },
        )
        .await
        .unwrap();
    assert!(!old.execution_allowed);
    assert_eq!(old.command.expires_at_ms, d.command.expires_at_ms);
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert_eq!(
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.contains("/join/"))
            .count(),
        1
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_native_roles_expiry_replay_and_http_closed_input() {
    let (f, r, admin, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    let off = f.policy.clone().with_agent_matrix_provisioning(false);
    assert!(
        f.domain
            .poll_managed_agent_matrix(&f.owner.p, &off, ExecutorPollInput::default())
            .await
            .is_err()
    );
    for p in [&f.admin.p, &f.outsider.p, &f.coordinator.p, &admin.p] {
        assert!(
            f.domain
                .poll_managed_agent_matrix(p, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
    }
    let d = matrix_poll(&f).await;
    let input = observation(&d, true);
    for p in [&f.admin.p, &f.outsider.p, &f.coordinator.p, &admin.p] {
        assert!(
            f.domain
                .record_managed_agent_matrix(p, &f.policy, &d.command.command_id, input.clone())
                .await
                .is_err()
        );
    }
    let mut bad = input.clone();
    bad.observation.agent_reservation_id = "different_child".into();
    assert!(
        f.domain
            .record_managed_agent_matrix(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .is_err()
    );
    let service = svc(&f, m.gateway.clone());
    let mut raw = serde_json::to_value(&input).unwrap();
    raw["approved"] = json!(true);
    post(
        &service,
        &format!("agent-matrix/{}/result", d.command.command_id),
        &f.owner.token,
        raw,
        StatusCode::BAD_REQUEST,
    )
    .await;
    f.domain
        .record_managed_agent_matrix(&f.owner.p, &f.policy, &d.command.command_id, input.clone())
        .await
        .unwrap();
    let mut changed = input.clone();
    changed.observation.observed_ledger_revision += 1;
    assert!(
        f.domain
            .record_managed_agent_matrix(&f.owner.p, &f.policy, &d.command.command_id, changed)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET native_until_ms=1 WHERE assignment_id=$1").bind::<Text,_>(&a.id).execute(&mut db).await.unwrap();
    f.domain
        .record_managed_agent_matrix(&f.owner.p, &f.policy, &d.command.command_id, input)
        .await
        .unwrap();
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    assert_ne!(read_matrix(&f, &r).await.state, "matrix_verified");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_join_races_revoke_and_late_join_after_cleanup_is_left_again() {
    let (f, r, admin, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    permit(&f).await;
    m.hold_join.store(true, Ordering::SeqCst);
    let (domain, gateway, matrix, policy, id) = (
        f.domain.clone(),
        m.gateway.clone(),
        m.matrix.clone(),
        f.policy.clone(),
        a.id.clone(),
    );
    let worker = tokio::spawn(async move {
        domain
            .reconcile_managed_agent_matrix(&id, &gateway, &matrix, Some(&policy))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), m.join_started.notified())
        .await
        .unwrap();
    f.domain
        .revoke_managed_agent_request(
            &admin.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "revoke_during_join".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
    m.join_continue.notify_one();
    worker.await.unwrap().unwrap();
    let state = read_matrix(&f, &r).await;
    assert_eq!(state.state, "fenced");
    assert!(state.verified_at_ms.is_none());
    assert!(state.cleanup_observed_at_ms.is_some());
    member(
        &mut m.states.lock().unwrap(),
        &state.room_id,
        &state.puppet_mxid,
        "join",
    );
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, None)
        .await
        .unwrap();
    assert_eq!(
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.ends_with("/leave"))
            .count(),
        2
    );
    assert_eq!(
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.contains("/join/"))
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_decline_generation_change_and_changed_admin_never_join() {
    let (f, r, _, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    let d = matrix_poll(&f).await;
    f.domain
        .record_managed_agent_matrix(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            observation(&d, false),
        )
        .await
        .unwrap();
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    invalidate_authority(&f, &r.project_id).await;
    let old = f
        .domain
        .recover_managed_agent_matrix(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ExecutorRecoverInput {
                command_digest: d.command_digest,
            },
        )
        .await
        .unwrap();
    assert!(!old.execution_allowed);
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    assert_eq!(read_matrix(&f, &r).await.state, "fenced");
    let (f, r, _, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    let d = permit(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.devices SET generation=generation+1 WHERE id=$1")
        .bind::<Text, _>(f.owner.p.device_id.as_ref().unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    assert!(
        f.domain
            .record_managed_agent_matrix(
                &f.owner.p,
                &f.policy,
                &d.command.command_id,
                observation(&d, true)
            )
            .await
            .is_err()
    );
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    assert_eq!(read_matrix(&f, &r).await.state, "fenced");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_matrix_response_failure_rolls_back_and_matrix_fact_loss_fences_without_join() {
    let (f, r, _, a) = prepared_matrix().await;
    let m = matrix_fixture(&f, &r).await;
    let d = matrix_poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_matrix_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture'; END; $$;CREATE TRIGGER fixture_matrix_failure BEFORE UPDATE ON hagency_agent_v1.management_agent_matrix FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_matrix_failure()").await.unwrap();
    assert!(
        f.domain
            .record_managed_agent_matrix(
                &f.owner.p,
                &f.policy,
                &d.command.command_id,
                observation(&d, true)
            )
            .await
            .is_err()
    );
    db.batch_execute("DROP TRIGGER fixture_matrix_failure ON hagency_agent_v1.management_agent_matrix;DROP FUNCTION hagency_agent_v1.fixture_matrix_failure()").await.unwrap();
    assert!(sql_query("SELECT observation IS NULL AS matched FROM hagency_agent_v1.management_agent_matrix_checks WHERE id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    let d = permit(&f).await;
    member(
        &mut m.states.lock().unwrap(),
        &d.command.reserve_command.room_id,
        &d.command.reserve_command.decision_actor_mxid,
        "leave",
    );
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    assert!(m.calls.lock().unwrap().is_empty());
    assert_eq!(read_matrix(&f, &r).await.state, "fenced");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v9_matrix_migration_rolls_back_and_preserves_original_agent_owner_and_reservation() {
    // The runner provides an isolated throwaway database and serial execution.
    let (f, r, _, a) = prepared_matrix().await;
    let personal = f
        .domain
        .create_agent(
            &f.owner.p,
            CreateAgent {
                display_name: "Personal survives Matrix migration".into(),
                idempotency_key: "matrix_upgrade_personal".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let before = read_matrix(&f, &r).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_agent_matrix_checks(id text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=9 AND to_regclass('hagency_agent_v1.management_agent_matrix') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_matrix_checks")
        .await
        .unwrap();
    for _ in 0..2 {
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap();
    }
    let after = read_matrix(&f, &r).await;
    assert_eq!(after.assignment_id, a.id);
    assert_eq!(after.puppet_mxid, before.puppet_mxid);
    assert_eq!(after.room_id, before.room_id);
    assert_eq!(
        f.domain
            .agent(&f.owner.p, &personal.id, now_ms())
            .await
            .unwrap()
            .owner_user_id,
        f.owner.p.user_id
    );
    let assignment = f
        .domain
        .managed_agent_assignment(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(assignment.reserve_receipt_digest, a.reserve_receipt_digest);
    assert!(!assignment.execution_available);
}

#[test]
fn matrix_canonical_fixture_is_closed_and_matches_native() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-matrix-v1.json"
    ))
    .unwrap();
    let c: ManagedAgentMatrixChallenge =
        serde_json::from_value(fixture["challenge"].clone()).unwrap();
    c.validate("example.test").unwrap();
    let r: ManagedAgentMatrixObservation =
        serde_json::from_value(fixture["observation"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    assert_eq!(allocation_digest(&c).unwrap(), fixture["challengeDigest"]);
    assert_eq!(allocation_digest(&r).unwrap(), fixture["observationDigest"]);
    let mut bad = fixture["challenge"].clone();
    bad["runtimeReady"] = json!(true);
    assert!(serde_json::from_value::<ManagedAgentMatrixChallenge>(bad).is_err());
}
