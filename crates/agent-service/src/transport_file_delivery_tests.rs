use super::*;
use crate::file_delivery::{Delivery, Manifest, Target};
use serde_json::{Value, json};
fn manifest() -> Manifest {
    Manifest {
        filename: "original.txt".into(),
        caption: Some("Original caption".into()),
        bytes: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
    }
}
async fn staged(f: &Fixture, lease: &LeaseRef, name: &str) -> (Target, Delivery) {
    let id = enqueue(f, name).await;
    running(f, lease, &id, "file-execution").await;
    let d = f
        .transport
        .prepare_file(
            &f.p,
            lease,
            &id,
            "file-execution",
            "first-file",
            manifest(),
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let target = Target {
        dispatch_id: id,
        execution_id: "file-execution".into(),
        file_id: d.id.clone(),
    };
    f.transport
        .stage_file_chunk(
            &f.p,
            lease,
            &target,
            0,
            b"hello",
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let d = f
        .transport
        .queue_file(&f.p, lease, &target, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    (target, d)
}
async fn lease(f: &Fixture) -> LeaseRef {
    let l = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, crate::api::now_ms())
        .await
        .unwrap();
    LeaseRef {
        agent_id: l.agent_id,
        epoch: l.epoch,
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_staging_is_exact_scoped_and_unknown_upload_never_rearms() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let id = enqueue(&f, "file-stage").await;
    running(&f, &lease, &id, "file-execution").await;
    let d = f
        .transport
        .prepare_file(
            &f.p,
            &lease,
            &id,
            "file-execution",
            "first-file",
            manifest(),
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(d.state, "staging");
    let duplicate = f
        .transport
        .prepare_file(
            &f.p,
            &lease,
            &id,
            "file-execution",
            "first-file",
            manifest(),
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.id, d.id);
    let mut changed = manifest();
    changed.caption = Some("changed".into());
    assert!(
        f.transport
            .prepare_file(
                &f.p,
                &lease,
                &id,
                "file-execution",
                "first-file",
                changed,
                &f.facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    assert!(
        f.transport
            .prepare_file(
                &f.second,
                &lease,
                &id,
                "file-execution",
                "second-device",
                manifest(),
                &f.facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    let t = Target {
        dispatch_id: id,
        execution_id: "file-execution".into(),
        file_id: d.id,
    };
    assert!(
        f.transport
            .queue_file(&f.p, &lease, &t, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    for _ in 0..2 {
        assert_eq!(
            f.transport
                .stage_file_chunk(
                    &f.p,
                    &lease,
                    &t,
                    0,
                    &b"hello"[..3],
                    &f.facts,
                    crate::api::now_ms()
                )
                .await
                .unwrap()
                .received_bytes,
            3
        );
    }
    assert!(
        f.transport
            .stage_file_chunk(&f.p, &lease, &t, 0, b"bad", &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    assert!(
        f.transport
            .stage_file_chunk(&f.p, &lease, &t, 4, b"o", &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    f.transport
        .stage_file_chunk(&f.p, &lease, &t, 3, b"lo", &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .queue_file(&f.p, &lease, &t, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    let now = crate::api::now_ms();
    let claim = f
        .transport
        .claim_file(&t.file_id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(claim.bytes.as_deref(), Some(b"hello".as_slice()));
    assert!(
        f.transport
            .claim_file(&t.file_id, &f.facts, now)
            .await
            .is_err()
    );
    let deadline = {
        let mut db = f.transport.db.lock().await.unwrap();
        sql_query("SELECT worker_until_ms AS now_ms FROM hagency_agent_v1.file_outbox WHERE id=$1")
            .bind::<Text, _>(&t.file_id)
            .get_result::<Clock>(&mut *db)
            .await
            .unwrap()
            .now_ms
    };
    f.transport.expire_files(deadline).await.unwrap();
    f.transport
        .confirm_file_upload(
            &t.file_id,
            &claim.token,
            Some("mxc://example.test/late-upload"),
            deadline,
        )
        .await
        .unwrap();
    let historical = f
        .transport
        .file_delivery(&f.p, &t, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(historical.state, "unknown");
    assert!(historical.upload_accepted);
    assert!(
        !serde_json::to_string(&historical)
            .unwrap()
            .contains("mxc://")
    );
    assert!(
        f.transport
            .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    let reopened = TransportStore::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        Limits::default(),
    )
    .await
    .unwrap();
    assert!(
        reopened
            .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_send_unknown_settles_only_exact_trusted_message_and_retains_history() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let (t, _) = staged(&f, &lease, "file-unknown-send").await;
    let upload = f
        .transport
        .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .confirm_file_upload(
            &t.file_id,
            &upload.token,
            Some("mxc://example.test/confirmed-upload"),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let send = f
        .transport
        .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    assert!(send.bytes.is_none());
    f.transport
        .confirm_file_sent(&t.file_id, &send.token, None)
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    let event = json!({"type":"m.room.message","event_id":"$actual-file:test","sender":f.facts.puppet_mxid,"room_id":f.facts.room_id,"content":send.row.content().unwrap()});
    let mut foreign = event.clone();
    foreign["sender"] = json!("@other:test");
    assert!(!f.transport.observe_file_event(&foreign).await.unwrap());
    foreign = event.clone();
    foreign["room_id"] = json!("!other:test");
    assert!(!f.transport.observe_file_event(&foreign).await.unwrap());
    foreign = event.clone();
    foreign["content"]["url"] = json!("mxc://example.test/replaced");
    assert!(!f.transport.observe_file_event(&foreign).await.unwrap());
    assert!(f.transport.observe_file_event(&event).await.unwrap());
    assert!(f.transport.observe_file_event(&event).await.unwrap());
    f.transport
        .confirm_file_sent(&t.file_id, &send.token, Some("$actual-file:test"))
        .await
        .unwrap();
    f.transport
        .release_lease(&f.p, &lease, crate::api::now_ms())
        .await
        .unwrap();
    let history = f
        .transport
        .file_delivery(&f.second, &t, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(history.state, "sent");
    assert_eq!(
        history.matrix_event_id.as_deref(),
        Some("$actual-file:test")
    );
    let other = fixture().await;
    assert!(
        f.transport
            .file_delivery(&other.p, &t, crate::api::now_ms())
            .await
            .is_err()
    );
    let mut db = f.transport.db.lock().await.unwrap();
    for statement in [
        "DELETE FROM hagency_agent_v1.file_outbox WHERE id=$1",
        "UPDATE hagency_agent_v1.file_outbox SET state='queued' WHERE id=$1",
        "UPDATE hagency_agent_v1.file_outbox SET manifest='{}'::jsonb WHERE id=$1",
    ] {
        assert!(
            sql_query(statement)
                .bind::<Text, _>(&t.file_id)
                .execute(&mut *db)
                .await
                .is_err()
        );
    }
    #[derive(diesel::QueryableByName)]
    struct Length {
        #[diesel(sql_type=BigInt)]
        total: i64,
    }
    assert_eq!(sql_query("SELECT octet_length(content)::bigint AS total FROM hagency_agent_v1.file_outbox WHERE id=$1").bind::<Text,_>(&t.file_id).get_result::<Length>(&mut *db).await.unwrap().total,0);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_requester_departure_permanently_cancels_unpublished_bytes() {
    let mut f = fixture().await;
    let lease = lease(&f).await;
    let (t, _) = staged(&f, &lease, "file-revoke").await;
    let mut negative = f.facts.clone();
    negative.requester_in_room = false;
    assert!(
        f.transport
            .claim_file(&t.file_id, &negative, crate::api::now_ms())
            .await
            .is_err()
    );
    f.facts.observed_at_ms = crate::api::now_ms();
    assert!(
        f.transport
            .claim_file(&t.file_id, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    f.transport
        .expire_files(crate::api::now_ms())
        .await
        .unwrap();
    let status = f
        .transport
        .file_delivery(&f.p, &t, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(status.state, "cancelled");
    assert!(!status.upload_accepted);
}

fn gateway(f: &Fixture) -> crate::gateway::Gateway {
    let facts = f.facts.clone();
    crate::gateway::Gateway::new(
        Arc::new(move |room| {
            let f = facts.clone();
            Box::pin(async move {
                let member = |id: &str| json!({"type":"m.room.member","state_key":id,"content":{"membership":"join"}});
                if room == f.space_id {
                    Ok(vec![
                        member(&f.owner_mxid),
                        json!({"type":"m.space.child","state_key":f.room_id,"content":{"via":["example.test"]}}),
                    ])
                } else {
                    if room != f.room_id {
                        return Err(Error::Unavailable("unrelated_fixture_room"));
                    }
                    Ok(vec![
                        member(&f.owner_mxid),
                        member(&f.requester_mxid),
                        member(&f.puppet_mxid),
                        json!({"type":"m.room.create","state_key":"","content":{}}),
                    ])
                }
            })
        }),
        "@_hagency_service:example.test".into(),
    )
}
struct Wire {
    headers: String,
    path: String,
    body: Vec<u8>,
}
async fn matrix(
    fail_send: bool,
) -> (
    crate::matrix_client::MatrixClient,
    Arc<std::sync::Mutex<Vec<Wire>>>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let records = Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = records.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let out = out.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                let split = loop {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(i) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                        break i + 4;
                    }
                    assert!(bytes.len() < 32768);
                };
                let headers = String::from_utf8_lossy(&bytes[..split]).into_owned();
                let length = headers
                    .lines()
                    .find_map(|s| {
                        s.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                assert!(length <= crate::file_delivery::MAX_BYTES);
                while bytes.len() < split + length {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let body = bytes[split..split + length].to_vec();
                let (code, response) = if path.starts_with("/_matrix/media/v3/upload") {
                    (
                        200,
                        json!({"content_uri":"mxc://example.test/uploaded-wire-file"}),
                    )
                } else {
                    assert!(path.contains("/send/m.room.message/hagency-file-fil_"));
                    let content: Value = serde_json::from_slice(&body).unwrap();
                    if fail_send {
                        (500, json!({"errcode":"M_UNKNOWN"}))
                    } else {
                        (
                            200,
                            json!({"event_id":format!("$published_{}:example.test",content["org.hagency.file_delivery_id"].as_str().unwrap())}),
                        )
                    }
                };
                out.lock().unwrap().push(Wire {
                    headers,
                    path,
                    body,
                });
                let response = response.to_string();
                stream.write_all(format!("HTTP/1.1 {code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
            });
        }
    });
    (
        crate::matrix_client::MatrixClient::new(
            origin,
            "test-sender-as-token".into(),
            "example.test".into(),
        )
        .unwrap(),
        records,
        task,
    )
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_real_routes_and_matrix_bytes_retain_original_sender_thread_and_lost_ack() {
    use crate::{api::App, identity::Verifier};
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    use salvo::{
        Service,
        http::StatusCode,
        test::{ResponseExt, TestClient},
    };
    async fn post(
        service: &Service,
        path: &str,
        token: &str,
        body: Value,
        status: StatusCode,
    ) -> Value {
        let mut response = TestClient::post(format!(
            "https://example.test/hagency/api/v1/execution/files/{path}"
        ))
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .json(&body)
        .send(service)
        .await;
        let actual = response.status_code;
        let result = response.take_json::<Value>().await.unwrap();
        assert_eq!(actual, Some(status), "response: {result}");
        result
    }
    for fail_send in [false, true] {
        let f = fixture().await;
        let lease = lease(&f).await;
        let dispatch = enqueue(
            &f,
            if fail_send {
                "wire-unknown"
            } else {
                "wire-sent"
            },
        )
        .await;
        running(&f, &lease, &dispatch, "file-execution").await;
        let gateway = gateway(&f);
        let (matrix, wire, task) = matrix(fail_send).await;
        let issuer = url::Url::parse("https://example.test/auth/").unwrap();
        let origin = url::Url::parse("https://example.test/").unwrap();
        let verifier = Verifier::new(
            issuer.clone(),
            issuer.join("introspect").unwrap(),
            origin.clone(),
            "unused-test-secret".into(),
            "example.test".into(),
        )
        .unwrap();
        let service = Service::new(
            App::new(
                f.auth.clone(),
                verifier,
                origin,
                issuer,
                "example.test".into(),
            )
            .with_domain(f.domain.clone(), gateway.clone())
            .with_transport(f.transport.clone())
            .with_matrix_client(matrix.clone())
            .router(),
        );
        let mut discovery = TestClient::get("https://example.test/hagency/api/v1/discovery")
            .add_header("host", "example.test", true)
            .send(&service)
            .await;
        assert_eq!(discovery.status_code, Some(StatusCode::OK));
        assert!(
            discovery.take_json::<Value>().await.unwrap()["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(crate::file_delivery::CAPABILITY))
        );
        let request = json!({"lease":lease,"dispatchId":dispatch,"executionId":"file-execution","requestId":"wire-file","file":manifest()});
        post(
            &service,
            "prepare",
            &f.credential,
            request.clone(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let mut extra = request.clone();
        extra["roomId"] = json!("!arbitrary:test");
        post(
            &service,
            "prepare",
            &f.device_token,
            extra,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let d = post(
            &service,
            "prepare",
            &f.device_token,
            request,
            StatusCode::OK,
        )
        .await;
        let id = d["delivery"]["id"].as_str().unwrap().to_owned();
        let target = Target {
            dispatch_id: dispatch.clone(),
            execution_id: "file-execution".into(),
            file_id: id.clone(),
        };
        post(&service,"chunk",&f.device_token,json!({"lease":lease,"target":target,"offset":0,"data":STANDARD_NO_PAD.encode(b"hello")}),StatusCode::OK).await;
        post(
            &service,
            "queue",
            &f.device_token,
            json!({"lease":lease,"target":target}),
            StatusCode::OK,
        )
        .await;
        crate::workers::send_files(&f.transport, &gateway, &matrix, &mut String::new())
            .await
            .unwrap();
        let uploaded = post(
            &service,
            "status",
            &f.device_token,
            json!(target),
            StatusCode::OK,
        )
        .await;
        assert_eq!(uploaded["delivery"]["state"], "uploaded");
        assert!(!uploaded.to_string().contains("mxc://"));
        crate::workers::send_files(&f.transport, &gateway, &matrix, &mut String::new())
            .await
            .unwrap();
        crate::workers::send_files(&f.transport, &gateway, &matrix, &mut String::new())
            .await
            .unwrap();
        let thread_root = f
            .transport
            .owned_event_header(&f.p, &dispatch, crate::api::now_ms())
            .await
            .unwrap()
            .thread_root;
        let content = {
            let wire = wire.lock().unwrap();
            assert_eq!(wire.len(), 2, "unknown/sent must not reupload or resend");
            assert_eq!(wire[0].body, b"hello");
            for request in wire.iter() {
                assert!(
                    request
                        .headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-sender-as-token")
                );
                let url = url::Url::parse(&format!("http://example.test{}", request.path)).unwrap();
                assert!(
                    url.query_pairs()
                        .any(|(key, value)| key == "user_id" && value == f.facts.puppet_mxid)
                );
            }
            let content: Value = serde_json::from_slice(&wire[1].body).unwrap();
            assert_eq!(content["filename"], "original.txt");
            assert_eq!(content["body"], "Original caption");
            assert_eq!(content["info"]["size"], 5);
            assert_eq!(content["m.relates_to"]["event_id"], thread_root);
            content
        };
        let before = post(
            &service,
            "status",
            &f.device_token,
            json!(target),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            before["delivery"]["state"],
            if fail_send { "unknown" } else { "sent" }
        );
        let event_id = format!("$published_{id}:example.test");
        let event = json!({"type":"m.room.message","event_id":event_id,"sender":f.facts.puppet_mxid,"room_id":f.facts.room_id,"content":content});
        let hs = "fixture-file-hs-token-only-not-real-secret";
        let inbox = crate::appservice::Inbox::open(
            &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
            hs.into(),
        )
        .await
        .unwrap();
        let as_service = Service::new(inbox.router());
        let response = TestClient::put(format!(
            "https://example.test/_matrix/app/v1/transactions/file_{id}"
        ))
        .bearer_auth(hs)
        .json(&json!({"events":[event]}))
        .send(&as_service)
        .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let mut route_cursor = format!("file_{id}");
        route_cursor.pop();
        crate::workers::route(&inbox, &f.transport, &gateway, &mut route_cursor)
            .await
            .unwrap();
        let observed = post(
            &service,
            "status",
            &f.device_token,
            json!(target),
            StatusCode::OK,
        )
        .await;
        assert_eq!(observed["delivery"]["state"], "sent");
        assert_eq!(observed["delivery"]["matrixEventId"], event_id);
        {
            let mut db = f.transport.db.lock().await.unwrap();
            let e = TransportStore::dispatch(&mut db, &f.p, &dispatch)
                .await
                .unwrap();
            assert_eq!(e.state, "running", "a file receipt is not task completion");
        }
        task.abort();
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_zero_bytes_and_actual_hash_are_checked_before_upload() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let dispatch = enqueue(&f, "file-empty").await;
    running(&f, &lease, &dispatch, "file-execution").await;
    for valid_hash in [false, true] {
        let mut file = manifest();
        file.bytes = 0;
        if valid_hash {
            file.sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into();
        }
        let d = f
            .transport
            .prepare_file(
                &f.p,
                &lease,
                &dispatch,
                "file-execution",
                if valid_hash { "empty" } else { "wrong-hash" },
                file,
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        let target = Target {
            dispatch_id: dispatch.clone(),
            execution_id: "file-execution".into(),
            file_id: d.id,
        };
        let result = f
            .transport
            .queue_file(&f.p, &lease, &target, &f.facts, crate::api::now_ms())
            .await;
        if valid_hash {
            assert_eq!(result.unwrap().state, "queued");
            let claim = f
                .transport
                .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
                .await
                .unwrap();
            assert_eq!(claim.bytes.as_deref(), Some(b"".as_slice()));
            f.transport
                .confirm_file_upload(&target.file_id, &claim.token, None, crate::api::now_ms())
                .await
                .unwrap();
        } else {
            assert!(matches!(
                result,
                Err(Error::Conflict("file_bytes_mismatch"))
            ));
        }
    }
    f.transport
        .release_lease(&f.p, &lease, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .expire_files(crate::api::now_ms())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_waiting_claim_observes_committed_device_revocation() {
    let f = fixture().await;
    let lease = lease(&f).await;
    let (target, _) = staged(&f, &lease, "file-device-race").await;
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut blocker = AsyncPgConnection::establish(&url).await.unwrap();
    blocker.batch_execute("BEGIN").await.unwrap();
    sql_query("UPDATE hagency_agent_v1.devices SET revoked=true WHERE id=$1")
        .bind::<Text, _>(f.p.device_id.as_deref().unwrap())
        .execute(&mut blocker)
        .await
        .unwrap();
    let t = f.transport.clone();
    let facts = f.facts.clone();
    let id = target.file_id.clone();
    let task = tokio::spawn(async move { t.claim_file(&id, &facts, crate::api::now_ms()).await });
    // Prove the original claim is actually waiting on this connection, rather
    // than relying on a sleep to assume it has reached the authorization lock.
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        loop {
            let waiting=sql_query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE pg_backend_pid()=ANY(pg_blocking_pids(a.pid))) AS matched").get_result::<Flag>(&mut blocker).await.unwrap();
            if waiting.matched {break}
            assert!(!task.is_finished());
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    blocker.batch_execute("COMMIT").await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized(_))));
    let status = f
        .transport
        .file_delivery(&f.second, &target, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(status.state, "queued");
    assert!(!status.upload_accepted);
    f.transport
        .expire_files(crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .file_delivery(&f.second, &target, crate::api::now_ms())
            .await
            .unwrap()
            .state,
        "cancelled"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_uploaded_bytes_do_not_grant_send_after_device_revocation_or_room_encryption()
{
    let f = fixture().await;
    let lease = lease(&f).await;
    let (target, _) = staged(&f, &lease, "file-upload-revoke").await;
    let upload = f
        .transport
        .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .confirm_file_upload(
            &target.file_id,
            &upload.token,
            Some("mxc://example.test/original-before-revocation"),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let mut encrypted = f.facts.clone();
    encrypted.encrypted = true;
    assert!(matches!(
        f.transport
            .claim_file(&target.file_id, &encrypted, crate::api::now_ms())
            .await,
        Err(Error::Conflict("encrypted_room_requires_client_crypto"))
    ));
    f.auth
        .revoke_device(
            &f.credential,
            f.p.device_id.as_deref().unwrap(),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
            .await
            .is_err()
    );
    f.transport
        .expire_files(crate::api::now_ms())
        .await
        .unwrap();
    let historical = f
        .transport
        .file_delivery(&f.second, &target, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(historical.state, "cancelled");
    assert!(historical.upload_accepted);
    assert!(historical.matrix_event_id.is_none());
}
