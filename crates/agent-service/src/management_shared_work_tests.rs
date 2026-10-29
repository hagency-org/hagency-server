#[path="management_agent_budget_usage_tests.rs"]
mod budget_usage;
use super::*;
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    attempts::remove_migration(db).await;
    db.batch_execute("DROP TRIGGER fence_shared_work ON hagency_agent_v1.management_shared_runtimes;DROP TABLE hagency_agent_v1.management_shared_replies,hagency_agent_v1.management_shared_work_receipts,hagency_agent_v1.management_shared_work;DROP FUNCTION hagency_agent_v1.fence_shared_work(),hagency_agent_v1.retain_shared_work(),hagency_agent_v1.validate_shared_work();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=12;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=12)").await.unwrap();
}
use crate::{management_shared_work_types::*, transport::RoutedEvent};
use std::collections::BTreeSet;

pub(super) async fn message(f: &Fixture, r: &ManagedAgentRequest, id: &str) -> RoutedEvent {
    let m = read_matrix(f, r).await;
    RoutedEvent {
        event_id: format!("${id}"),
        room_id: m.room_id,
        sender_mxid: f.outsider.p.mxid.clone(),
        body: "Please answer with hello.".into(),
        mentioned_mxids: BTreeSet::from([m.puppet_mxid]),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    }
}
pub(super) async fn queued(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    a: &SharedActivityLease,
    id: &str,
) -> SharedWorkItem {
    let e = message(f, r, id).await;
    f.domain
        .route_shared_event(&f.policy, &e, now_ms(), &m.gateway)
        .await
        .unwrap();
    f.domain
        .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
        .await
        .unwrap()
        .unwrap()
}
fn input(a: &SharedActivityLease, w: &SharedWorkItem) -> SharedWorkInput {
    SharedWorkInput {
        authority: a.authority.clone(),
        work_id: w.id.clone(),
        work_digest: allocation_digest(w).unwrap(),
    }
}
pub(super) async fn started(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    a: &SharedActivityLease,
    id: &str,
) -> SharedWorkItem {
    let w = queued(f, r, m, a, id).await;
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    w
}
pub(super) async fn ticket(
    f: &Fixture,
    d: &SharedRuntimeDelivery,
    w: &SharedWorkItem,
) -> SharedWorkResultTicket {
    f.domain
        .recover_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            SharedWorkRecoverInput {
                runtime_command_digest: d.command_digest.clone(),
                work_digest: allocation_digest(w).unwrap(),
                observation_id: None,
            },
        )
        .await
        .unwrap()
}
pub(super) fn outcome(
    d: &SharedRuntimeDelivery,
    w: &SharedWorkItem,
    t: &SharedWorkResultTicket,
    revision: i64,
    state: &str,
) -> SharedWorkResultUpload {
    let body = (state == "completed").then(|| "Hello.".to_string());
    SharedWorkResultUpload {
        work_digest: t.work_digest.clone(),
        token: t.token.clone(),
        receipt: SharedWorkReceipt {
            version: 1,
            observation_id: format!("{}_usage_{revision}", w.id),
            work_id: w.id.clone(),
            work_digest: t.work_digest.clone(),
            execution_id: w.execution_id.clone(),
            call_id: format!("call_{}", w.id),
            runtime_command_digest: d.command_digest.clone(),
            process_digest: "a".repeat(64),
            ledger_revision: revision,
            state: state.into(),
            known_consumed_tokens: 17,
            retained_tokens: Some(17),
            usage_complete: true,
            body_digest: body.as_ref().map(|s| crate::hash(s)),
            body,
        },
    }
}
pub(super) struct Sender {
    pub(super) matrix: MatrixClient,
    pub(super) calls: Arc<Mutex<Vec<(String, Value)>>>,
    drop_first: Arc<std::sync::atomic::AtomicBool>,
    hold: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.release.close();
        self.task.abort();
    }
}
pub(super) async fn sender() -> Sender {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let drop_first = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hold = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (saved, dropper, held, started, resume) = (
        calls.clone(),
        drop_first.clone(),
        hold.clone(),
        entered.clone(),
        release.clone(),
    );
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (saved, dropper, held, started, resume) = (
                saved.clone(),
                dropper.clone(),
                held.clone(),
                started.clone(),
                resume.clone(),
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
                    .find_map(|s| {
                        s.to_ascii_lowercase()
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
                let url = Url::parse(&format!(
                    "http://localhost{}",
                    header.split_whitespace().nth(1).unwrap()
                ))
                .unwrap();
                let txn = percent_encoding::percent_decode_str(
                    url.path_segments().unwrap().next_back().unwrap(),
                )
                .decode_utf8()
                .unwrap()
                .into_owned();
                let body = serde_json::from_slice(&raw[headers..headers + length]).unwrap();
                saved.lock().unwrap().push((txn.clone(), body));
                started.notify_one();
                if held.as_ref().load(Ordering::SeqCst) {
                    let _ = resume.acquire().await;
                }
                if dropper.swap(false, Ordering::SeqCst) {
                    return;
                }
                let result = json!({"event_id":format!("$reply_{txn}")}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{result}",
                    result.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    let matrix = MatrixClient::new(
        Url::parse(&format!("http://{address}")).unwrap(),
        "fixture-as-token".into(),
        "example.test".into(),
    )
    .unwrap();
    Sender {
        matrix,
        calls,
        drop_first,
        hold,
        entered,
        release,
        task,
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_exact_intake_ack_start_result_history_and_matrix_reply() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = queued(&f, &r, &m, &a, "first_work").await;
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let one = f
        .domain
        .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(one.newly_started);
    let again = f
        .domain
        .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(!again.newly_started);
    assert_eq!(one.work.execution_id, again.work.execution_id);
    let t = ticket(&f, &d, &w).await;
    assert!(t.recorded.is_none());
    let saved = f
        .domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 201, "completed"),
        )
        .await
        .unwrap();
    assert_eq!(saved.state, "completed");
    let replay = f
        .domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 201, "completed"),
        )
        .await
        .unwrap();
    assert_eq!(replay.receipt_digest, saved.receipt_digest);
    let s = sender().await;
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    assert_eq!(s.calls.lock().unwrap().len(), 1);
    assert_eq!(s.calls.lock().unwrap()[0].1["body"], "Hello.");
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    assert_eq!(s.calls.lock().unwrap().len(), 1);
    let page = f
        .domain
        .shared_work_history(&f.outsider.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].reply_state.as_deref(), Some("sent"));
    assert_eq!(
        page.items[0].usage.as_ref().unwrap().known_consumed_tokens,
        17
    );
    assert!(
        f.domain
            .shared_work_history(&admin.p, &f.policy, &r.id, SharedWorkQuery::default())
            .await
            .is_ok()
    );
    for p in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert!(
            f.domain
                .shared_work_history(p, &f.policy, &r.id, SharedWorkQuery::default())
                .await
                .is_err()
        );
    }
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("Please answer")
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_unknown_matrix_reply_retries_exact_transaction_without_second_start() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = started(&f, &r, &m, &a, "lost_reply").await;
    let t = ticket(&f, &d, &w).await;
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 201, "completed"),
        )
        .await
        .unwrap();
    let s = sender().await;
    s.drop_first.store(true, Ordering::SeqCst);
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "UPDATE hagency_agent_v1.management_shared_replies SET lease_until_ms=0 WHERE work_id=$1",
    )
    .bind::<Text, _>(&w.id)
    .execute(&mut db)
    .await
    .unwrap();
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    {
        let calls = s.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], calls[1]);
    }
    assert!(
        !f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_late_result_after_revoke_retains_usage_without_reply_or_reexecution() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = started(&f, &r, &m, &a, "late_usage").await;
    revoke(&f, &r, &admin).await;
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
            .await
            .is_err()
    );
    let t = ticket(&f, &d, &w).await;
    let mut unknown = outcome(&d, &w, &t, 201, "unknown");
    unknown.receipt.usage_complete = false;
    unknown.receipt.retained_tokens = None;
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, unknown)
        .await
        .unwrap();
    let page = f
        .domain
        .shared_work_history(&f.outsider.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    let usage = page.items[0].usage.as_ref().unwrap();
    assert_eq!(usage.known_consumed_tokens, 17);
    assert_eq!(usage.retained_tokens, None);
    assert!(!usage.complete);
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 202, "completed"),
        )
        .await
        .unwrap();
    let s = sender().await;
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    assert!(s.calls.lock().unwrap().is_empty());
    let page = f
        .domain
        .shared_work_history(&f.outsider.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    assert_eq!(page.items[0].reply_state.as_deref(), Some("blocked"));
    assert!(page.items[0].usage.as_ref().unwrap().complete);
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_inflight_matrix_send_revoke_records_original_late_event() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = started(&f, &r, &m, &a, "inflight_send").await;
    let t = ticket(&f, &d, &w).await;
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 201, "completed"),
        )
        .await
        .unwrap();
    let s = sender().await;
    s.hold.store(true, Ordering::SeqCst);
    let mut cursor = String::new();
    let send = f
        .domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut cursor);
    let cancel = async {
        s.entered.notified().await;
        revoke(&f, &r, &admin).await;
        s.release.close();
    };
    let (result, _) = tokio::join!(send, cancel);
    result.unwrap();
    let page = f
        .domain
        .shared_work_history(&f.outsider.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    assert_eq!(page.items[0].reply_state.as_deref(), Some("sent"));
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    assert_eq!(s.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_no_prompt_for_wrong_roles_or_unmentioned_or_changed_rooms() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (_d, a) = qualify(&f).await;
    let mut e = message(&f, &r, "no_mentions").await;
    e.mentioned_mxids.clear();
    f.domain
        .route_shared_event(&f.policy, &e, now_ms(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
            .await
            .unwrap()
            .is_none()
    );
    let w = queued(&f, &r, &m, &a, "private_prompt").await;
    for p in [&f.outsider.p, &f.coordinator.p, &f.admin.p, &admin.p] {
        assert!(
            f.domain
                .poll_shared_work(p, &f.policy, a.authority.clone(), &m.gateway)
                .await
                .is_err()
        );
    }
    member(
        &mut m.states.lock().unwrap(),
        &w.room_id,
        &f.outsider.p.mxid,
        "leave",
    );
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_incomplete_usage_blocks_next_start_and_changed_receipt_rolls_back() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = started(&f, &r, &m, &a, "unsettled_first").await;
    let t = ticket(&f, &d, &w).await;
    let mut incomplete = outcome(&d, &w, &t, 201, "completed");
    incomplete.receipt.retained_tokens = None;
    incomplete.receipt.usage_complete = false;
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, incomplete)
        .await
        .unwrap();
    let next = queued(&f, &r, &m, &a, "unsettled_next").await;
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &next), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &next), &m.gateway)
            .await
            .is_err()
    );
    let mut changed = outcome(&d, &w, &t, 202, "completed");
    changed.receipt.body = Some("Changed reply".into());
    changed.receipt.body_digest = Some(crate::hash("Changed reply"));
    assert!(
        f.domain
            .record_shared_work_result(&f.owner.p, &f.policy, &w.id, changed)
            .await
            .is_err()
    );
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 202, "completed"),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &next), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_actual_appservice_and_closed_executor_http_are_idempotent() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let e = message(&f, &r, "appservice_work").await;
    let hs_token = "fixture-only-shared-hs-token-12345678";
    let inbox = crate::appservice::Inbox::open(&f.url, hs_token.into())
        .await
        .unwrap();
    let as_service = Service::new(inbox.router());
    let txn = format!("zz_shared_{}", crate::entity_id().unwrap());
    let raw = json!({"events":[{"type":"m.room.message","event_id":e.event_id,"room_id":e.room_id,"sender":e.sender_mxid,"content":{"msgtype":"m.text","body":e.body,"m.mentions":{"user_ids":e.mentioned_mxids}}}]});
    for _ in 0..2 {
        let response = Box::pin(
            TestClient::put(format!(
                "https://example.test/_matrix/app/v1/transactions/{txn}"
            ))
            .bearer_auth(hs_token)
            .json(&raw)
            .send(&as_service),
        )
        .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
    }
    let transport =
        crate::transport::TransportStore::open(&f.url, crate::transport::Limits::default())
            .await
            .unwrap();
    let mut cursor = "zz_shared_".to_string();
    let context = (f.domain.clone(), f.policy.clone());
    Box::pin(crate::workers::route_with_shared(
        &inbox,
        &transport,
        &m.gateway,
        &mut cursor,
        Some(&context),
    ))
    .await
    .unwrap();
    let api = svc(&f, m.gateway.clone());
    post(
        &api,
        "shared-execution/work/poll",
        &f.owner.token,
        json!({"authority":a.authority,"ownerMxid":f.owner.p.mxid}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let response = post(
        &api,
        "shared-execution/work/poll",
        &f.owner.token,
        json!({"authority":a.authority}),
        StatusCode::OK,
    )
    .await;
    let w: SharedWorkItem = serde_json::from_value(response["work"].clone()).unwrap();
    assert_eq!(w.body, "Please answer with hello.");
    post(
        &api,
        "shared-execution/work/poll",
        &f.admin.token,
        json!({"authority":a.authority}),
        StatusCode::NOT_FOUND,
    )
    .await;
    post(
        &api,
        "shared-execution/work/ack",
        &f.owner.token,
        serde_json::to_value(input(&a, &w)).unwrap(),
        StatusCode::OK,
    )
    .await;
    let first = post(
        &api,
        "shared-execution/work/start",
        &f.owner.token,
        serde_json::to_value(input(&a, &w)).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["newlyStarted"], true);
    let replay = post(
        &api,
        "shared-execution/work/start",
        &f.owner.token,
        serde_json::to_value(input(&a, &w)).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay["newlyStarted"], false);
    let ticket_reply=post(&api,&format!("shared-execution/work/{}/recover",w.id),&f.owner.token,json!({"runtimeCommandDigest":d.command_digest,"workDigest":allocation_digest(&w).unwrap(),"observationId":null}),StatusCode::OK).await;
    let t: SharedWorkResultTicket = serde_json::from_value(ticket_reply["ticket"].clone()).unwrap();
    assert_eq!(t.state, "started");
    assert!(t.recorded.is_none());
    post(
        &api,
        &format!("shared-execution/work/{}/result", w.id),
        &f.owner.token,
        serde_json::to_value(outcome(&d, &w, &t, 201, "completed")).unwrap(),
        StatusCode::OK,
    )
    .await;
    let page = post(
        &api,
        &format!("agent-requests/{}/work/query", r.id),
        &f.outsider.token,
        json!({"limit":16}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(page["work"]["items"].as_array().unwrap().len(), 1);
    assert!(!page.to_string().contains("Please answer"));
    let recovered = ticket(&f, &d, &w).await;
    assert_eq!(recovered.reply_state.as_deref(), Some("pending"));
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_receipt_reply_atomicity_and_global_native_revision_are_preserved() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = started(&f, &r, &m, &a, "atomic_work").await;
    let t = ticket(&f, &d, &w).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_reply_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fixture_reply_reject BEFORE INSERT ON hagency_agent_v1.management_shared_replies FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_reply_reject()").await.unwrap();
    assert!(
        f.domain
            .record_shared_work_result(
                &f.owner.p,
                &f.policy,
                &w.id,
                outcome(&d, &w, &t, 201, "completed")
            )
            .await
            .is_err()
    );
    assert!(sql_query("SELECT state='started' AND known_consumed_tokens=0 AND latest_receipt IS NULL AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1").bind::<Text,_>(&w.id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_reply_reject ON hagency_agent_v1.management_shared_replies;DROP FUNCTION hagency_agent_v1.fixture_reply_reject()").await.unwrap();
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            outcome(&d, &w, &t, 201, "completed"),
        )
        .await
        .unwrap();
    assert!(save(&f, &d, "stopped", 201).await.is_err());
    revoke(&f, &r, &admin).await;
    let fence = poll(&f).await;
    let mut clash = fence_result(&fence, true);
    if let AgentExecutorReceipt::Fence(ref mut receipt) = clash.receipt {
        receipt.ledger_revision = 201;
        receipt.fence_revision = 201;
    }
    assert!(submit(&f, &fence, clash).await.is_err());
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.management_shared_work_receipts WHERE work_id=$1")
            .bind::<Text, _>(&w.id)
            .execute(&mut db)
            .await
            .is_err()
    );
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.management_shared_replies SET content='{}' WHERE work_id=$1"
        )
        .bind::<Text, _>(&w.id)
        .execute(&mut db)
        .await
        .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_v12_migration_rolls_back_without_changing_original_process_or_activity() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_replies(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=12 AND to_regclass('hagency_agent_v1.management_shared_work') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_replies")
        .await
        .unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let original = history(&f, &d).await;
    assert_eq!(original.command_digest, d.command_digest);
    assert_eq!(original.recorded.unwrap().ledger_revision, 200);
    assert!(!original.execution_allowed);
    let w = queued(&f, &r, &m, &a, "after_migration").await;
    assert_eq!(w.runtime_command_digest, d.command_digest);
}

async fn usage_settlement_case(late: bool) {
    use crate::management_parent_types::*;
    let (mut f, r, admin, _assignment, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_parent_settlement(true);
    let (d, activity) = qualify(&f).await;
    let w = started(&f, &r, &m, &activity, "settlement_usage").await;
    let t = ticket(&f, &d, &w).await;
    if !late {
        f.domain
            .record_shared_work_result(
                &f.owner.p,
                &f.policy,
                &w.id,
                outcome(&d, &w, &t, 201, "completed"),
            )
            .await
            .unwrap();
    }
    revoke(&f, &r, &admin).await;
    let fence = poll(&f).await;
    let mut cleanup = fence_result(&fence, true);
    if let AgentExecutorReceipt::Fence(ref mut receipt) = cleanup.receipt {
        receipt.fence_revision = 301;
        receipt.ledger_revision = 301;
        receipt.retained_tokens = Some(0);
        receipt.usage_complete = true;
        receipt.process_state = "stopped".into();
    }
    if !late {
        assert_eq!(
            submit(&f, &fence, cleanup.clone())
                .await
                .unwrap_err()
                .to_string(),
            "agent_executor_usage_changed"
        );
        if let AgentExecutorReceipt::Fence(ref mut receipt) = cleanup.receipt {
            receipt.known_consumed_tokens = 17;
            receipt.retained_tokens = Some(17);
        }
    }
    submit(&f, &fence, cleanup).await.unwrap();
    let allocation = f
        .domain
        .management_project_allocation(&f.owner.p, &f.policy, &r.project_id)
        .await
        .unwrap()
        .unwrap();
    let grant = f
        .domain
        .management_resource_grant(&f.owner.p, &f.policy, &allocation.grant_id)
        .await
        .unwrap();
    f.domain
        .release_management_allocation(
            &f.owner.p,
            &f.policy,
            &allocation.id,
            RevokeInput {
                command_id: "release_used_project".into(),
                expected_revision: allocation.revision,
            },
        )
        .await
        .unwrap();
    let release = f
        .domain
        .poll_management_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release.command.operation, "project.release");
    let mut released = crate::management_tests::allocations::result(&release, "released", 302);
    if !late {
        assert_eq!(
            f.domain
                .record_management_executor_result(
                    &f.owner.p,
                    &f.policy,
                    &release.command.command_id,
                    released.clone()
                )
                .await
                .unwrap_err()
                .to_string(),
            "executor_usage_changed"
        );
        released.receipt.retained_tokens = 17;
    }
    f.domain
        .record_management_executor_result(
            &f.owner.p,
            &f.policy,
            &release.command.command_id,
            released,
        )
        .await
        .unwrap();
    f.domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &grant.id,
            ParentCloseInput {
                command_id: "close_used_parent".into(),
                expected_grant_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    let parent = f
        .domain
        .poll_management_parent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    let mut settlement = ParentResultInput {
        command_digest: parent.command_digest.clone(),
        token: parent.token.clone(),
        receipt: NativeGrantSettlementReceipt {
            version: 1,
            command_id: parent.command.command_id.clone(),
            command_digest: parent.command_digest.clone(),
            grant_id: grant.id.clone(),
            grant_receipt_digest: grant.receipt_digest.clone(),
            enrollment_id: grant.receipt.enrollment_id.clone(),
            period_key: grant.receipt.period_key.clone(),
            fence_revision: 309,
            ledger_revision: 310,
            state: "closed".into(),
            retained_tokens: 0,
            known_consumed_tokens: 0,
            unresolved_tokens: 0,
            liability_count: 0,
            liability_digest: "c".repeat(64),
        },
    };
    if !late {
        assert_eq!(
            f.domain
                .record_management_parent_result(
                    &f.owner.p,
                    &f.policy,
                    &parent.command.command_id,
                    settlement.clone()
                )
                .await
                .unwrap_err()
                .to_string(),
            "parent_usage_changed"
        );
        settlement.receipt.retained_tokens = 17;
        settlement.receipt.known_consumed_tokens = 17;
        settlement.receipt.liability_count = 1;
    }
    f.domain
        .record_management_parent_result(
            &f.owner.p,
            &f.policy,
            &parent.command.command_id,
            settlement.clone(),
        )
        .await
        .unwrap();
    if late {
        let fresh = ticket(&f, &d, &w).await;
        let ack = f
            .domain
            .record_shared_work_result(
                &f.owner.p,
                &f.policy,
                &w.id,
                outcome(&d, &w, &fresh, 201, "completed"),
            )
            .await
            .unwrap();
        assert_eq!(ack.reply_state.as_deref(), Some("blocked"));
    }
    // Removal reports independently recorded evidence. A historic zero fence
    // cannot erase later usage or claim that Matrix cleanup has happened.
    let removal = f
        .domain
        .managed_agent_removal(&f.outsider.p, &r.id)
        .await
        .unwrap();
    assert!(removal.central_fenced && !removal.can_cancel && !removal.can_revoke);
    assert_eq!(removal.usage.known_consumed_tokens, Some(17));
    assert_eq!(removal.usage.retained_tokens_floor, Some(17));
    assert_eq!(removal.usage.fence_usage_complete, Some(true));
    assert_eq!(removal.usage.requires_reconciliation, late);
    assert_eq!(
        removal.native_fence.unwrap().process_state.as_deref(),
        Some("stopped")
    );
    assert_eq!(removal.process_observation.unwrap().state, "qualified");
    assert!(removal.matrix.unwrap().cleanup_observed_at_ms.is_none());
    let allocation = f
        .domain
        .management_project_allocation(&f.owner.p, &f.policy, &r.project_id)
        .await
        .unwrap()
        .unwrap();
    let usage = allocation.observed_usage.unwrap();
    assert_eq!(usage.known_consumed_tokens, 17);
    assert_eq!(usage.retained_tokens_floor, 17);
    assert_eq!(usage.requires_reconciliation, late);
    assert_eq!(
        allocation.balance.unwrap().retained_tokens,
        if late { 0 } else { 17 }
    );
    let observed = f
        .domain
        .management_parent_settlement(&f.owner.p, &f.policy, &grant.id)
        .await
        .unwrap();
    assert_eq!(observed.held_tokens, 17);
    assert_eq!(observed.known_consumed_tokens, Some(17));
    assert_eq!(
        observed.known_unused_released_tokens,
        grant.receipt.allocated_tokens - 17
    );
    assert_eq!(
        observed.state,
        if late {
            "awaiting_reconciliation"
        } else {
            "closed"
        }
    );
    assert_eq!(
        observed.receipt.as_ref().unwrap().known_consumed_tokens,
        if late { 0 } else { 17 }
    );
    if late {
        f.domain
            .settle_management_parent(
                &f.owner.p,
                &f.policy,
                &grant.id,
                RevokeInput {
                    command_id: "reconcile_late_usage".into(),
                    expected_revision: observed.revision,
                },
            )
            .await
            .unwrap();
        let next = f
            .domain
            .poll_management_parent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .unwrap();
        settlement.command_digest = next.command_digest.clone();
        settlement.token = next.token;
        settlement.receipt.command_id = next.command.command_id.clone();
        settlement.receipt.command_digest = next.command_digest;
        settlement.receipt.ledger_revision = 311;
        settlement.receipt.known_consumed_tokens = 17;
        settlement.receipt.retained_tokens = 17;
        settlement.receipt.liability_count = 1;
        let reconciled = f
            .domain
            .record_management_parent_result(
                &f.owner.p,
                &f.policy,
                &next.command.command_id,
                settlement,
            )
            .await
            .unwrap();
        assert_eq!(reconciled.settlement.state, "closed");
        assert_eq!(reconciled.settlement.held_tokens, 17);
        assert_eq!(reconciled.settlement.unreconciled_child_tokens, 0);
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_known_usage_cannot_be_erased_by_agent_project_or_parent_cleanup() {
    Box::pin(usage_settlement_case(false)).await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_late_usage_after_zero_settlement_restores_conservative_liability() {
    Box::pin(usage_settlement_case(true)).await;
}

#[test]
fn canonical_shared_work_contract_matches_native_vectors_and_bounds_escaped_receipt() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-work-v1.json"
    ))
    .unwrap();
    let work: SharedWorkItem = serde_json::from_value(fixture["work"].clone()).unwrap();
    let receipt: SharedWorkReceipt = serde_json::from_value(fixture["receipt"].clone()).unwrap();
    work.validate().unwrap();
    receipt.validate_for(&work, &"a".repeat(64)).unwrap();
    assert_eq!(allocation_digest(&work).unwrap(), fixture["workDigest"]);
    assert_eq!(
        allocation_digest(&receipt).unwrap(),
        fixture["receiptDigest"]
    );
    let mut changed = fixture["work"].clone();
    changed["authority"] = json!(true);
    assert!(serde_json::from_value::<SharedWorkItem>(changed).is_err());
    let mut large = receipt;
    large.body = Some("\u{0001}".repeat(8192));
    large.body_digest = Some(crate::hash(large.body.as_ref().unwrap()));
    assert!(large.validate_for(&work, &"a".repeat(64)).is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_result_expiry_and_generation_rotation_allow_only_late_evidence() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, activity) = qualify(&f).await;
    let work = started(&f, &r, &m, &activity, "rotated_work").await;
    let t = ticket(&f, &d, &work).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_work SET lease_until_ms=1 WHERE id=$1")
        .bind::<Text, _>(&work.id)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .record_shared_work_result(
                &f.owner.p,
                &f.policy,
                &work.id,
                outcome(&d, &work, &t, 201, "completed")
            )
            .await
            .unwrap_err()
            .to_string(),
        "shared_work_result_ticket_changed"
    );
    let fresh = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Reauthorized".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let owner = f
        .auth
        .authenticate(&fresh.token, now_ms(), true)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(&owner, &f.policy, activity.authority, &m.gateway)
            .await
            .is_err()
    );
    assert!(ticket_error(&f, &d, &work).await.is_err());
    let recovered = f
        .domain
        .recover_shared_work_result(
            &owner,
            &f.policy,
            &work.id,
            SharedWorkRecoverInput {
                runtime_command_digest: d.command_digest.clone(),
                work_digest: allocation_digest(&work).unwrap(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let report = outcome(&d, &work, &recovered, 201, "completed");
    let result = f
        .domain
        .record_shared_work_result(&owner, &f.policy, &work.id, report)
        .await
        .unwrap();
    assert_eq!(result.reply_state.as_deref(), Some("blocked"));
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_work SET known_consumed_tokens=18,retained_tokens=18 WHERE id=$1").bind::<Text,_>(&work.id).execute(&mut db).await.is_err());
}
async fn ticket_error(
    f: &Fixture,
    d: &SharedRuntimeDelivery,
    w: &SharedWorkItem,
) -> crate::Result<SharedWorkResultTicket> {
    f.domain
        .recover_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            SharedWorkRecoverInput {
                runtime_command_digest: d.command_digest.clone(),
                work_digest: allocation_digest(w).unwrap(),
                observation_id: None,
            },
        )
        .await
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_history_discovery_and_bounded_pagination_survive_issuance_disable() {
    let (mut f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (_d, _activity) = qualify(&f).await;
    for n in 0..17 {
        let event = message(&f, &r, &format!("page_{n}")).await;
        f.domain
            .route_shared_event(&f.policy, &event, now_ms(), &m.gateway)
            .await
            .unwrap();
    }
    let first = f
        .domain
        .shared_work_history(&f.outsider.p, &f.policy, &r.id, SharedWorkQuery::default())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 16);
    assert!(first.next_after.is_some());
    let second = f
        .domain
        .shared_work_history(
            &admin.p,
            &f.policy,
            &r.id,
            SharedWorkQuery {
                after: first.next_after,
                limit: Some(16),
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_after.is_none());
    let ids = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|v| &v.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 17);
    assert!(first.items.iter().all(|v| v.usage.is_none()));
    f.policy = f
        .policy
        .with_project_reservations(false)
        .with_agent_executor(false);
    let api = svc(&f, m.gateway.clone());
    let mut response = Box::pin(
        TestClient::get("https://example.test/hagency/api/v1/management/session")
            .add_header("host", "example.test", true)
            .bearer_auth(&f.owner.token)
            .send(&api),
    )
    .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let session: Value = response.take_json().await.unwrap();
    assert!(
        session["features"]
            .as_array()
            .unwrap()
            .contains(&json!(SHARED_WORK_HISTORY_FEATURE))
    );
    post(
        &api,
        &format!("agent-requests/{}/work/query", r.id),
        &f.outsider.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    post(
        &api,
        &format!("agent-requests/{}/work/query", r.id),
        &f.outsider.token,
        json!({"limit":17}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    f.policy = f.policy.with_shared_execution(false);
    let api = svc(&f, m.gateway.clone());
    let mut response = Box::pin(
        TestClient::get("https://example.test/hagency/api/v1/management/session")
            .add_header("host", "example.test", true)
            .bearer_auth(&f.owner.token)
            .send(&api),
    )
    .await;
    let session: Value = response.take_json().await.unwrap();
    assert!(
        !session["features"]
            .as_array()
            .unwrap()
            .contains(&json!(SHARED_WORK_HISTORY_FEATURE))
    );
    let disabled = post(
        &api,
        &format!("agent-requests/{}/work/query", r.id),
        &f.outsider.token,
        json!({}),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert!(
        disabled
            .to_string()
            .contains("shared_execution_unavailable")
    );
}

pub(super) async fn another_assignment(
    f: &Fixture,
    first: &ManagedAgentRequest,
    admin: &Actor,
    m: &MatrixFixture,
    sequence: i64,
) -> (ManagedAgentRequest, PendingManagedAssignment) {
    let cx = f
        .domain
        .management_agent_readiness(&f.outsider.p, &f.policy, &first.project_id)
        .await
        .unwrap()
        .request_context
        .unwrap();
    let request = f
        .domain
        .request_managed_agent(
            &f.outsider.p,
            &f.policy,
            &first.project_id,
            RequestManagedAgent {
                command_id: format!("request_shared_agent_{sequence}"),
                expected_setup_revision: cx.setup_revision,
                expected_setup_binding_revision: cx.setup_binding_revision,
                expected_authority_revision: cx.authority_revision,
                expected_allocation_revision: cx.allocation_revision,
                definition: AgentDefinition {
                    name: format!("Shared helper {sequence}"),
                    capability: "coding".into(),
                    description: "Shared history fixture".into(),
                    tool_mode: "none".into(),
                },
                requested_tokens: 100,
            },
            &m.gateway,
        )
        .await
        .unwrap();
    let request = f
        .domain
        .decide_managed_agent(
            &admin.p,
            &f.policy,
            &request.id,
            decide(&request, &format!("approve_shared_agent_{sequence}")),
            &m.gateway,
        )
        .await
        .unwrap();
    let reserve = poll(f).await;
    let mut result = reservation(&reserve);
    if let AgentExecutorReceipt::Reserve(ref mut receipt) = result.receipt {
        receipt.ledger_revision = (sequence + 1) * 100;
    }
    let assignment = submit(f, &reserve, result)
        .await
        .unwrap()
        .assignment
        .unwrap();
    let check = matrix_poll(f).await;
    assert_eq!(check.command.assignment_id, assignment.id);
    let mut result = observation(&check, true);
    result.observation.observed_ledger_revision = (sequence + 1) * 100;
    f.domain
        .record_managed_agent_matrix(&f.owner.p, &f.policy, &check.command.command_id, result)
        .await
        .unwrap();
    f.domain
        .reconcile_managed_agent_matrix(&assignment.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    (request, assignment)
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_restart_index_is_role_scoped_paged_and_independent_of_admission() {
    let (mut f, first, admin, _assignment, m) = Box::pin(prepared()).await;
    assert!(
        f.domain
            .shared_work_index(&f.outsider.p, &f.policy, SharedWorkIndexQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let (first_runtime, activity) = qualify(&f).await;
    queued(&f, &first, &m, &activity, "index_first").await;
    save(&f, &first_runtime, "stopped", 201).await.unwrap();
    let (second, _assignment) = Box::pin(another_assignment(&f, &first, &admin, &m, 2)).await;
    let second_runtime = shared_poll(&f).await;
    let second_activity = save(&f, &second_runtime, "qualified", 301)
        .await
        .unwrap()
        .activity
        .unwrap();
    queued(&f, &second, &m, &second_activity, "index_second").await;
    let page = f
        .domain
        .shared_work_index(
            &f.outsider.p,
            &f.policy,
            SharedWorkIndexQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next_after.is_some());
    let next = f
        .domain
        .shared_work_index(
            &admin.p,
            &f.policy,
            SharedWorkIndexQuery {
                project_id: Some(first.project_id.clone()),
                after: page.next_after,
                limit: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert!(next.next_after.is_none());
    assert_ne!(page.items[0].request_id, next.items[0].request_id);
    assert_eq!(
        BTreeSet::from([
            page.items[0].request_id.clone(),
            next.items[0].request_id.clone()
        ]),
        BTreeSet::from([first.id.clone(), second.id.clone()])
    );
    for role in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert!(
            f.domain
                .shared_work_index(role, &f.policy, SharedWorkIndexQuery::default())
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    let replacement = actor(&f.auth, "unrelated_history_reader").await;
    assert!(f.domain.shared_work_index(&replacement.p, &f.policy, SharedWorkIndexQuery::default()).await.unwrap().items.is_empty());
    invalidate_authority(&f, &first.project_id).await;
    assert_eq!(f.domain.shared_work_index(&admin.p, &f.policy, SharedWorkIndexQuery::default()).await.unwrap().items.len(), 2);
    f.policy = f
        .policy
        .with_agent_admission(false)
        .with_project_reservations(false)
        .with_agent_executor(false);
    let api = svc(&f, m.gateway.clone());
    let index = post(
        &api,
        "agent-work/list",
        &f.outsider.token,
        json!({"projectId":first.project_id,"limit":1}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(index["requests"]["items"].as_array().unwrap().len(), 1);
    let item = index["requests"]["items"][0].as_object().unwrap();
    assert_eq!(item.len(), 4);
    assert!(item.contains_key("agentName"));
    post(
        &api,
        "agent-work/list",
        &f.outsider.token,
        json!({"ownerMxid":f.owner.p.mxid}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post(
        &api,
        &format!("agent-requests/{}/work/query", first.id),
        &f.outsider.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(
        f.domain
            .shared_work_index(
                &f.outsider.p,
                &f.policy,
                SharedWorkIndexQuery {
                    project_id: Some("unrelated_project".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_work_exact_mention_is_filtered_before_candidate_page_limit() {
    let (f, first, admin, _first_assignment, m) = Box::pin(prepared()).await;
    let (first_runtime, first_activity) = qualify(&f).await;
    let (second, second_assignment) = Box::pin(another_assignment(&f, &first, &admin, &m, 2)).await;
    let second_runtime = shared_poll(&f).await;
    assert_eq!(second_runtime.command.assignment_id, second_assignment.id);
    assert!(first_runtime.command.command_id < second_runtime.command.command_id);
    let second_activity = save(&f, &second_runtime, "qualified", 301)
        .await
        .unwrap()
        .activity
        .unwrap();
    let event = message(&f, &second, "exact_mentioned_sibling").await;
    f.domain
        .route_shared_event_single_candidate(&f.policy, &event, now_ms(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, first_activity.authority, &m.gateway)
            .await
            .unwrap()
            .is_none()
    );
    let work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            second_activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .expect("exact mentioned agent must be selected before the candidate cap");
    assert_eq!(work.assignment_id, second_assignment.id);
    assert_eq!(work.event_id, event.event_id);
    // Retrying the exact AS event uses the original inbox item, not another turn.
    f.domain
        .route_shared_event_single_candidate(&f.policy, &event, now_ms(), &m.gateway)
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, second_activity.authority, &m.gateway)
            .await
            .unwrap()
            .unwrap()
            .id,
        work.id
    );
}
