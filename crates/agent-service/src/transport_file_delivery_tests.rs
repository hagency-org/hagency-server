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
        let lookup =
            json!({"dispatchId":dispatch,"executionId":"file-execution","requestId":"wire-file"});
        post(
            &service,
            "lookup",
            &f.credential,
            lookup.clone(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let mut extra = lookup.clone();
        extra["lease"] = json!(lease);
        post(
            &service,
            "lookup",
            &f.device_token,
            extra,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let observation = post(
            &service,
            "lookup",
            &f.device_token,
            lookup.clone(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(observation["observation"]["delivery"], d["delivery"]);
        assert_eq!(observation["observation"]["preparationClosed"], false);
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

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_completed_request_file_denial_is_permanent_after_requester_rejoins_or_speaking_returns()
 {
    for uploaded in [false, true] {
        for kind in ["requester", "speaking", "encrypted"] {
            let f = fixture().await;
            let lease = lease(&f).await;
            let (target, _) = staged(&f, &lease, "file-after-completion").await;
            if uploaded {
                let claim = f
                    .transport
                    .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
                    .await
                    .unwrap();
                f.transport
                    .confirm_file_upload(
                        &target.file_id,
                        &claim.token,
                        Some("mxc://example.test/completed-original"),
                        crate::api::now_ms(),
                    )
                    .await
                    .unwrap();
            }
            f.transport
                .submit_reply(
                    &f.p,
                    &lease,
                    SubmitReply {
                        dispatch_id: target.dispatch_id.clone(),
                        execution_id: target.execution_id.clone(),
                        body: "Original task completed; file still pending".into(),
                    },
                    &f.facts,
                    crate::api::now_ms(),
                )
                .await
                .unwrap();
            let mut negative = f.facts.clone();
            match kind {
                "requester" => negative.requester_in_room = false,
                "speaking" => negative.puppet_can_send_message = false,
                _ => negative.encrypted = true,
            }
            assert!(
                f.transport
                    .claim_file(&target.file_id, &negative, crate::api::now_ms())
                    .await
                    .is_err()
            );
            // Old completed task outcomes cannot be rewritten just to fence a file.
            {
                let mut db = f.transport.db.lock().await.unwrap();
                assert_eq!(
                    TransportStore::dispatch(&mut db, &f.p, &target.dispatch_id)
                        .await
                        .unwrap()
                        .state,
                    "completed"
                );
            }
            assert!(
                f.transport
                    .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
                    .await
                    .is_err(),
                "{kind} loss after completion must survive restored positive facts; uploaded={uploaded}"
            );
            f.transport
                .expire_files(crate::api::now_ms())
                .await
                .unwrap();
            let status = f
                .transport
                .file_delivery(&f.p, &target, crate::api::now_ms())
                .await
                .unwrap();
            assert_eq!(status.state, "cancelled");
            assert_eq!(status.upload_accepted, uploaded);
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_denial_before_preparation_is_shared_per_original_request_and_late_receipts_remain_historical()
 {
    let f = fixture().await;
    let lease = lease(&f).await;
    let first = enqueue(&f, "file-not-yet-prepared").await;
    running(&f, &lease, &first, "file-first").await;
    let mut negative = f.facts.clone();
    negative.requester_in_room = false;
    f.transport
        .observe_delivery_authority(
            None,
            &f.binding,
            &f.facts.requester_mxid,
            Some(&first),
            &negative,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .prepare_file(
                &f.p,
                &lease,
                &first,
                "file-first",
                "file",
                manifest(),
                &f.facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    // A new original request after the restored membership remains independent.
    let (target, _) = staged(&f, &lease, "file-new-request").await;
    let upload = f
        .transport
        .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .confirm_file_upload(
            &target.file_id,
            &upload.token,
            Some("mxc://example.test/original-before-block"),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let send = f
        .transport
        .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .observe_delivery_authority(
            None,
            &f.binding,
            &f.facts.requester_mxid,
            Some(&target.dispatch_id),
            &negative,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    f.transport
        .confirm_file_sent(
            &target.file_id,
            &send.token,
            Some("$actual_before_or_during_block"),
        )
        .await
        .unwrap();
    let history = f
        .transport
        .file_delivery(&f.p, &target, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(history.state, "sent");
    assert_eq!(
        history.matrix_event_id.as_deref(),
        Some("$actual_before_or_during_block")
    );
    let mut db = f.transport.db.lock().await.unwrap();
    for sql in [
        "DELETE FROM hagency_agent_v1.file_delivery_denials WHERE owner_event_id=$1",
        "UPDATE hagency_agent_v1.file_delivery_denials SET owner_event_id=owner_event_id WHERE owner_event_id=$1",
    ] {
        assert!(
            sql_query(sql)
                .bind::<Text, _>(&target.dispatch_id)
                .execute(&mut *db)
                .await
                .is_err()
        );
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_denial_upgrade_is_additive_and_preserves_all_original_records() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    let snapshot_sql = "SELECT 'snapshot' AS id,md5(coalesce((SELECT jsonb_agg(to_jsonb(f) ORDER BY id)::text FROM hagency_agent_v1.file_outbox f),'[]')||coalesce((SELECT jsonb_agg(to_jsonb(e) ORDER BY id)::text FROM hagency_agent_v1.owner_events e),'[]')||coalesce((SELECT jsonb_agg(to_jsonb(r) ORDER BY id)::text FROM hagency_agent_v1.reply_outbox r),'[]')) AS digest";
    let before = sql_query(snapshot_sql)
        .get_result::<Existing>(&mut db)
        .await
        .unwrap()
        .digest;
    let _store = TransportStore::open(&url, Limits::default()).await.unwrap();
    let after = sql_query(snapshot_sql)
        .get_result::<Existing>(&mut db)
        .await
        .unwrap()
        .digest;
    assert_eq!(before, after);
    let _again = TransportStore::open(&url, Limits::default()).await.unwrap();
    let ready = sql_query(
        "SELECT to_regclass('hagency_agent_v1.file_delivery_denials') IS NOT NULL AS matched",
    )
    .get_result::<Flag>(&mut db)
    .await
    .unwrap();
    assert!(ready.matched);
    assert_eq!(
        after,
        sql_query(snapshot_sql)
            .get_result::<Existing>(&mut db)
            .await
            .unwrap()
            .digest
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_lookup_recovers_lost_prepare_and_fences_final_absence() {
    use crate::file_delivery::FileLookup;
    for prepare_first in [false, true] {
        let f = fixture().await;
        let lease = lease(&f).await;
        let id = enqueue(&f, "lookup-original").await;
        running(&f, &lease, &id, "lookup-execution").await;
        let query = FileLookup {
            dispatch_id: id.clone(),
            execution_id: "lookup-execution".into(),
            request_id: "lost-prepare-response".into(),
        };
        let before = f
            .transport
            .lookup_file(&f.p, &query, crate::api::now_ms())
            .await
            .unwrap();
        assert!(before.delivery.is_none());
        assert!(
            !before.preparation_closed,
            "temporary absence is not final evidence"
        );
        if prepare_first {
            let prepared = f
                .transport
                .prepare_file(
                    &f.p,
                    &lease,
                    &id,
                    &query.execution_id,
                    &query.request_id,
                    manifest(),
                    &f.facts,
                    crate::api::now_ms(),
                )
                .await
                .unwrap();
            let recovered = f
                .transport
                .lookup_file(&f.second, &query, crate::api::now_ms())
                .await
                .unwrap();
            assert_eq!(recovered.delivery.unwrap().id, prepared.id);
            assert!(!recovered.preparation_closed);
        }
        f.transport
            .finish_without_reply(
                &f.p,
                &lease,
                &id,
                &query.execution_id,
                "unknown",
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        // No lease, running execution or fresh positive Matrix facts are needed
        // to observe history. This must never mint an upload or replay grant.
        let closed = f
            .transport
            .lookup_file(&f.second, &query, crate::api::now_ms())
            .await
            .unwrap();
        assert!(closed.preparation_closed);
        assert_eq!(closed.delivery.is_some(), prepare_first);
        let text = serde_json::to_string(&closed).unwrap();
        for private in ["mxc://", "workerToken", "worker_token", "content", "lease"] {
            assert!(!text.contains(private));
        }
        assert!(
            f.transport
                .prepare_file(
                    &f.p,
                    &lease,
                    &id,
                    &query.execution_id,
                    &query.request_id,
                    manifest(),
                    &f.facts,
                    crate::api::now_ms()
                )
                .await
                .is_err()
        );
        let wrong = FileLookup {
            execution_id: "different-execution".into(),
            ..query.clone()
        };
        assert!(
            f.transport
                .lookup_file(&f.p, &wrong, crate::api::now_ms())
                .await
                .is_err()
        );
        let other = fixture().await;
        assert!(
            f.transport
                .lookup_file(&other.p, &query, crate::api::now_ms())
                .await
                .is_err()
        );
        let reopened = TransportStore::open(
            &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
            Limits::default(),
        )
        .await
        .unwrap();
        let restored = reopened
            .lookup_file(&f.p, &query, crate::api::now_ms())
            .await
            .unwrap();
        assert!(restored.preparation_closed);
        assert_eq!(restored.delivery.is_some(), prepare_first);
    }
}

/// Actual PostgreSQL state and HTTP bodies cross the shutdown boundary. Each
/// scenario owns a database because file candidates are deliberately global.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_worker_shutdown_drains_file_io_and_receipts_without_claiming_next_file() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    for phase in ["upload", "send", "receipt", "timeout"] {
        let name = format!("hagency_drain_{}", &crate::secret_token()[..16]);
        control
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let mut url = url::Url::parse(&source).unwrap();
        url.set_path(&format!("/{name}"));
        let outcome =
            tokio::spawn(async move { file_shutdown_scenario(url.as_str(), phase).await }).await;
        control
            .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .await
            .unwrap();
        outcome.unwrap();
    }
}
async fn file_shutdown_scenario(url: &str, phase: &str) {
    use crate::worker_runtime::{WorkerRuntime, drain_workers};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };
    let f = fixture_at(url).await;
    let lease = lease(&f).await;
    let (target, _) = staged(&f, &lease, &format!("shutdown-{phase}")).await;
    let second = if phase != "timeout" {
        let d = f
            .transport
            .prepare_file(
                &f.p,
                &lease,
                &target.dispatch_id,
                &target.execution_id,
                "second-file",
                manifest(),
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        let next = Target {
            file_id: d.id,
            ..target.clone()
        };
        f.transport
            .stage_file_chunk(
                &f.p,
                &lease,
                &next,
                0,
                b"hello",
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        f.transport
            .queue_file(&f.p, &lease, &next, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        Some(next)
    } else {
        None
    };
    if phase == "send" {
        let (matrix, _, server) = matrix(false).await;
        crate::workers::send_files(&f.transport, &gateway(&f), &matrix, &mut String::new())
            .await
            .unwrap();
        server.abort();
        let _ = server.await;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let is_send = phase == "send";
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(split) = bytes
                .windows(4)
                .position(|s| s == b"\r\n\r\n")
                .map(|i| i + 4)
            {
                let headers = std::str::from_utf8(&bytes[..split]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if bytes.len() < split + length {
                    continue;
                }
                if is_send {
                    assert!(headers.contains("/send/m.room.message/"));
                    assert_eq!(
                        serde_json::from_slice::<Value>(&bytes[split..split + length]).unwrap()["filename"],
                        "original.txt"
                    );
                } else {
                    assert!(headers.contains("/_matrix/media/v3/upload"));
                    assert_eq!(&bytes[split..split + length], b"hello");
                }
                seen_tx.send(()).unwrap();
                break;
            }
        }
        release_rx.await.ok();
        let body = if is_send {
            r#"{"event_id":"$drained-file:example.test"}"#
        } else {
            r#"{"content_uri":"mxc://example.test/drained-file"}"#
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // A timed-out owner may already have dropped its socket.
        let _ = socket.write_all(response.as_bytes()).await;
    });
    let held_matrix = crate::matrix_client::MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "shutdown-fixture-as-token".into(),
        "example.test".into(),
    )
    .unwrap();
    let runtime = WorkerRuntime::default();
    let worker_runtime = runtime.clone();
    let transport = f.transport.clone();
    let live_gateway = gateway(&f);
    let worker = tokio::spawn(async move {
        let ctx = worker_runtime.queue("files");
        let result = crate::workers::send_files_controlled(
            &transport,
            &live_gateway,
            &held_matrix,
            &mut String::new(),
            &ctx,
        )
        .await;
        ctx.finish(&result);
        result.unwrap();
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), seen_rx)
        .await
        .unwrap()
        .unwrap();
    let status = f
        .transport
        .file_delivery(&f.p, &target, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(status.state, if is_send { "sending" } else { "uploading" });
    let mut blocker = AsyncPgConnection::establish(url).await.unwrap();
    if phase == "receipt" {
        blocker
            .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
            .await
            .unwrap();
    }
    let draining_runtime = runtime.clone();
    let deadline = if phase == "timeout" {
        std::time::Duration::from_millis(30)
    } else {
        std::time::Duration::from_secs(5)
    };
    let draining =
        tokio::spawn(
            async move { drain_workers(&draining_runtime, &mut [worker], deadline).await },
        );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    if phase == "timeout" {
        assert!(!draining.await.unwrap());
        release_tx.send(()).ok();
        server.await.unwrap();
        f.transport
            .expire_files(crate::api::now_ms() + 60_001)
            .await
            .unwrap();
        assert_eq!(
            f.transport
                .file_delivery(&f.p, &target, crate::api::now_ms())
                .await
                .unwrap()
                .state,
            "unknown"
        );
        // A fresh worker must not reconstruct upload custody after cancellation.
        let (matrix, wire, server) = matrix(false).await;
        crate::workers::send_files(&f.transport, &gateway(&f), &matrix, &mut String::new())
            .await
            .unwrap();
        assert!(wire.lock().unwrap().is_empty());
        server.abort();
        let _ = server.await;
        return;
    }
    assert!(
        !draining.is_finished(),
        "drain must retain the in-flight HTTP future"
    );
    release_tx.send(()).unwrap();
    server.await.unwrap();
    if phase == "receipt" {
        #[derive(diesel::QueryableByName)]
        struct Waiting {
            #[diesel(sql_type=diesel::sql_types::Bool)]
            value: bool,
        }
        tokio::time::timeout(std::time::Duration::from_secs(2),async {
            loop {
                let waiting=sql_query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory') AS value").get_result::<Waiting>(&mut blocker).await.unwrap().value;
                if waiting {break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert!(
            !draining.is_finished(),
            "drain must retain receipt persistence after actual HTTP acceptance"
        );
        blocker.batch_execute("ROLLBACK").await.unwrap();
    }
    assert!(draining.await.unwrap());
    let status = f
        .transport
        .file_delivery(&f.p, &target, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(status.state, if is_send { "sent" } else { "uploaded" });
    let next = f
        .transport
        .file_delivery(&f.p, &second.unwrap(), crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(
        next.state,
        if is_send { "uploaded" } else { "queued" },
        "shutdown must not claim the next file from an already fetched batch"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_worker_health_detects_query_failure_recovers_idle_and_supervises_panic() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    let name = format!("hagency_health_{}", &crate::secret_token()[..16]);
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = url::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move { worker_health_scenario(url.as_str()).await }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}
async fn worker_health_scenario(url: &str) {
    use crate::{
        appservice::Inbox,
        worker_runtime::{WorkerRuntime, drain_workers},
    };
    let f = fixture_at(url).await;
    let inbox = Inbox::open(url, "health-fixture-hs-token-only-not-real-secret".into())
        .await
        .unwrap();
    let (matrix, _, server) = matrix(false).await;
    let runtime = WorkerRuntime::default();
    let worker = crate::workers::start_controlled(
        inbox.clone(),
        f.transport.clone(),
        gateway(&f),
        matrix.clone(),
        runtime.clone(),
    );
    async fn wait_health(runtime: &WorkerRuntime, healthy: bool) {
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                if runtime.health().healthy == healthy {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    wait_health(&runtime, true).await;
    // Break an actual query dependency rather than fabricating error counters.
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    db.batch_execute("ALTER TABLE hagency_agent_v1.routing_jobs RENAME TO routing_jobs_fault")
        .await
        .unwrap();
    wait_health(&runtime, false).await;
    let snapshot = serde_json::to_value(runtime.health()).unwrap();
    let routing = snapshot["queues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["name"] == "routing")
        .unwrap();
    assert!(routing["consecutiveFailures"].as_u64().unwrap() >= 3);
    assert_eq!(routing["error"], "storage_unavailable");
    db.batch_execute("ALTER TABLE hagency_agent_v1.routing_jobs_fault RENAME TO routing_jobs")
        .await
        .unwrap();
    wait_health(&runtime, true).await;
    assert!(drain_workers(&runtime, &mut [worker], std::time::Duration::from_secs(2)).await);
    // An actual worker panic during Gateway IO must become failed health, not
    // silently leave a healthy HTTP process with all joined queues gone.
    let runtime = WorkerRuntime::default();
    let panicking = crate::gateway::Gateway::new(
        Arc::new(|_| Box::pin(async { panic!("injected Gateway worker panic") })),
        "@_hagency_service:example.test".into(),
    );
    let body = json!({"events":[{"type":"m.room.message","event_id":"$health-panic:example.test","room_id":f.facts.room_id,"sender":f.facts.owner_mxid,"content":{"msgtype":"m.text","body":"health panic","m.mentions":{"user_ids":[f.facts.puppet_mxid]}}}]});
    inbox
        .accept("health-panic", body, crate::api::now_ms())
        .await
        .unwrap();
    let worker = crate::workers::start_controlled(
        inbox,
        f.transport.clone(),
        panicking,
        matrix,
        runtime.clone(),
    );
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.is_panic());
    assert!(!runtime.health().healthy);
    assert!(
        serde_json::to_value(runtime.health()).unwrap()["queues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|q| q["running"] == false)
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_worker_shutdown_closes_final_admission_after_actual_claim_lock_waits() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut admin = AsyncPgConnection::establish(&source).await.unwrap();
    for (kind, lock) in [
        ("reply", "target"),
        ("file", "target"),
        ("processing", "target"),
        ("pause", "target"),
        ("request", "target"),
        ("reply", "device"),
        ("file", "device"),
    ] {
        let name = format!("hagency_admission_{}", &crate::secret_token()[..16]);
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let mut url = url::Url::parse(&source).unwrap();
        url.set_path(&format!("/{name}"));
        let outcome =
            tokio::spawn(async move { admission_lock_scenario(url.as_str(), kind, lock).await })
                .await;
        admin
            .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .await
            .unwrap();
        outcome.unwrap();
    }
}
async fn admission_lock_scenario(url: &str, kind: &str, lock: &str) {
    use crate::{
        appservice::Inbox,
        worker_runtime::{WorkerRuntime, drain_workers},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let f = fixture_at(url).await;
    let lease = lease(&f).await;
    let now = crate::api::now_ms();
    let (table, id, initial) = if kind == "pause" {
        f.domain
            .suspend_binding(&f.p, &f.binding, now)
            .await
            .unwrap();
        f.transport
            .ingest_routed(
                &f.binding,
                event(&f, "admission-paused", true, None),
                &f.facts,
                now,
            )
            .await
            .unwrap();
        let rows = f.transport.pause_notice_candidates("", now).await.unwrap();
        (
            "pause_notice_outbox",
            rows.into_iter()
                .find(|r| r.binding_id == f.binding)
                .unwrap()
                .id,
            "pending",
        )
    } else if kind == "file" {
        let (target, _) = staged(&f, &lease, "admission-file").await;
        ("file_outbox", target.file_id, "queued")
    } else {
        let dispatch = enqueue(&f, "admission-request").await;
        running(&f, &lease, &dispatch, "admission-execution").await;
        match kind {
            "reply" => {
                let reply = f
                    .transport
                    .submit_reply(
                        &f.p,
                        &lease,
                        SubmitReply {
                            dispatch_id: dispatch,
                            execution_id: "admission-execution".into(),
                            body: "Admission fixture reply".into(),
                        },
                        &f.facts,
                        now,
                    )
                    .await
                    .unwrap();
                ("reply_outbox", reply.id, "pending")
            }
            "processing" => {
                let receipt = f
                    .transport
                    .mark_processing(
                        &f.p,
                        &lease,
                        &dispatch,
                        "admission-execution",
                        &f.facts,
                        now,
                    )
                    .await
                    .unwrap();
                ("processing_outbox", receipt.id, "pending")
            }
            "request" => {
                f.transport
                    .finish_without_reply(
                        &f.p,
                        &lease,
                        &dispatch,
                        "admission-execution",
                        "failed",
                        now,
                    )
                    .await
                    .unwrap();
                let rows = f
                    .transport
                    .request_notice_candidates("", now)
                    .await
                    .unwrap();
                (
                    "request_notice_outbox",
                    rows.into_iter()
                        .find(|r| r.owner_event_id == dispatch)
                        .unwrap()
                        .id,
                    "pending",
                )
            }
            _ => unreachable!(),
        }
    };
    let inbox = Inbox::open(
        url,
        "admission-fixture-hs-token-only-not-real-secret".into(),
    )
    .await
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let upstream = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            captured.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut bytes = [0; 8192];
                let _ = socket.read(&mut bytes).await;
                let body = r#"{"event_id":"$unexpected-admission:example.test","content_uri":"mxc://example.test/unexpected-admission"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let matrix = crate::matrix_client::MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "admission-fixture-as-token".into(),
        "example.test".into(),
    )
    .unwrap();
    let mut blocker = AsyncPgConnection::establish(url).await.unwrap();
    // pg_stat_activity snapshots are cached inside a transaction. Observe from
    // an independent autocommit connection while blocker retains the real lock.
    let mut observer = AsyncPgConnection::establish(url).await.unwrap();
    crate::key(&id).unwrap();
    if lock == "device" {
        let device = f.p.device_id.as_deref().unwrap();
        crate::key(device).unwrap();
        blocker
            .batch_execute(&format!(
                "BEGIN; SELECT id FROM hagency_agent_v1.devices WHERE id='{device}' FOR UPDATE"
            ))
            .await
            .unwrap();
    } else {
        blocker
            .batch_execute(&format!(
                "BEGIN; SELECT id FROM hagency_agent_v1.{table} WHERE id='{id}' FOR UPDATE"
            ))
            .await
            .unwrap();
    }
    let runtime = WorkerRuntime::default();
    let worker = crate::workers::start_controlled(
        inbox,
        f.transport.clone(),
        gateway(&f),
        matrix,
        runtime.clone(),
    );
    #[derive(diesel::QueryableByName)]
    struct Waiting {
        #[diesel(sql_type=diesel::sql_types::Bool)]
        value: bool,
    }
    let pattern = if lock == "device" {
        "%hagency_agent_v1.devices%FOR SHARE%".to_owned()
    } else {
        format!("SELECT true AS matched FROM hagency_agent_v1.{table}%FOR UPDATE")
    };
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let blocked=sql_query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1) AS value").bind::<Text,_>(&pattern).get_result::<Waiting>(&mut observer).await.unwrap().value;
            if blocked {break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|_|panic!("{kind} did not reach its actual {lock} claim lock"));
    // The actual production worker has already passed its outer stopping check
    // and is suspended inside claim SQL. Only the final transactional gate can
    // prevent first custody after this point; cancelling the future is forbidden.
    runtime.begin_shutdown();
    blocker.batch_execute("ROLLBACK").await.unwrap();
    assert!(drain_workers(&runtime, &mut [worker], std::time::Duration::from_secs(5)).await);
    let record=sql_query(format!("SELECT state AS id,coalesce(worker_token,'') AS digest FROM hagency_agent_v1.{table} WHERE id=$1")).bind::<Text,_>(&id).get_result::<Existing>(&mut blocker).await.unwrap();
    assert_eq!(
        record.id, initial,
        "{kind}/{lock} must not commit first send/upload custody after shutdown"
    );
    assert!(
        record.digest.is_empty(),
        "{kind}/{lock} must retain its unclaimed worker token"
    );
    assert_eq!(
        requests.as_ref().load(Ordering::SeqCst),
        0,
        "{kind}/{lock} must not emit a new HTTP request"
    );
    assert!(
        serde_json::to_value(runtime.health()).unwrap()["queues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|q| q["consecutiveFailures"] == 0),
        "normal shutdown admission refusal is not a dependency failure"
    );
    upstream.abort();
    let _ = upstream.await;
}
