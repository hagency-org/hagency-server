use super::*;
use crate::{
    domain::{AdminFacts, BindRoom, CreateBoundAgent, DomainStore, RoomFacts},
    identity::Identity,
    store::{RegisterDevice, Store},
    transport::{DeliveryFacts, LeaseRef, Limits, SubmitReply},
};
use diesel::{
    sql_query,
    sql_types::{BigInt, Jsonb, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_unavailable_room_does_not_starve_routing_or_unknown_replies() {
    // This test runs real global worker scans, so isolate it from parallel tests.
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_fair_{}",
        &crate::hash(&crate::secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = reqwest::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move { exercise(url.as_str()).await }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}
async fn exercise(url: &str) {
    let auth = Store::open(url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let transport = TransportStore::open(url, Limits::default()).await.unwrap();
    let inbox = Inbox::open(url, "fixture-hs-token-only-no-real-secret-1234".into())
        .await
        .unwrap();
    let now = now_ms();
    let grant = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: "fair-owner".into(),
                mxid: "@fair:example.test".into(),
                client_id: "fair-client".into(),
                valid_until_ms: now + 1_000_000,
            },
            now,
        )
        .await
        .unwrap();
    let initial = auth
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "fair-device".into(),
                name: "Fixture".into(),
            },
            now,
        )
        .await
        .unwrap();
    let owner = auth.authenticate(&initial.token, now, true).await.unwrap();
    let space = "!space:example.test";
    let bad = "!unavailable:example.test";
    let good = "!healthy:example.test";
    let admin = |room: &str, is_space| AdminFacts {
        actor_mxid: owner.mxid.clone(),
        room_id: room.into(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space,
        linked_space_id: if is_space { None } else { Some(space.into()) },
    };
    let project = domain
        .register_project(&owner, space, &admin(space, true), now)
        .await
        .unwrap();
    for room in [bad, good] {
        domain
            .register_room(
                &owner,
                &project.id,
                room,
                &admin(space, true),
                &admin(room, false),
                now,
            )
            .await
            .unwrap();
    }
    let room_facts = |room: &str, puppet: Option<&str>| RoomFacts {
        owner_direct_valid: false,
        owner_mxid: owner.mxid.clone(),
        room_id: room.into(),
        space_id: space.into(),
        observed_at_ms: now,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: puppet.map(str::to_owned),
        puppet_in_room: puppet.is_some(),
        encrypted: false,
    };
    let created = domain
        .create_bound_agent(
            &owner,
            CreateBoundAgent {
                project_id: project.id.clone(),
                room_id: bad.into(),
                display_name: "Fairness probe".into(),
                idempotency_key: "fair-create".into(),
            },
            &room_facts(bad, None),
            now,
        )
        .await
        .unwrap();
    domain
        .activate_binding(
            &owner,
            &created.binding.id,
            created.binding.generation,
            &room_facts(bad, Some(&created.agent.puppet_mxid)),
            now,
        )
        .await
        .unwrap();
    let bound = domain
        .bind_room(
            &owner,
            &created.agent.id,
            BindRoom {
                project_id: project.id.clone(),
                room_id: good.into(),
                idempotency_key: "fair-bind".into(),
            },
            &room_facts(good, Some(&created.agent.puppet_mxid)),
            now,
        )
        .await
        .unwrap();
    domain
        .activate_binding(
            &owner,
            &bound.binding.id,
            bound.binding.generation,
            &room_facts(good, Some(&created.agent.puppet_mxid)),
            now,
        )
        .await
        .unwrap();
    let actor = owner.mxid.clone();
    let puppet = created.agent.puppet_mxid.clone();
    let state = vec![
        json!({"type":"m.room.member","state_key":actor,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":puppet,"content":{"membership":"join"}}),
        json!({"type":"m.space.child","state_key":bad,"content":{"via":["example.test"]}}),
        json!({"type":"m.space.child","state_key":good,"content":{"via":["example.test"]}}),
    ];
    let gateway = Gateway::new(
        std::sync::Arc::new(move |room| {
            let state = state.clone();
            Box::pin(async move {
                if room == bad {
                    Err(Error::Unavailable("matrix_room_temporarily_unavailable"))
                } else {
                    Ok(state)
                }
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let raw = |room: &str, id: &str| json!({"type":"m.room.message","room_id":room,"event_id":id,"sender":owner.mxid,"content":{"msgtype":"m.text","body":"fixture only","m.mentions":{"user_ids":[created.agent.puppet_mxid]}}});
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    for i in 0..12 {
        let id = format!("blocked_{i:02}");
        let body = json!({"events":[raw(bad,&format!("$blocked{i}"))]});
        sql_query("INSERT INTO hagency_agent_v1.inbound_transactions VALUES($1,$2,$3,$4)")
            .bind::<Text, _>(&id)
            .bind::<Text, _>(crate::hash(&body.to_string()))
            .bind::<Jsonb, _>(body)
            .bind::<BigInt, _>(now + i)
            .execute(&mut db)
            .await
            .unwrap();
        sql_query("INSERT INTO hagency_agent_v1.routing_jobs VALUES($1,'pending',$2)")
            .bind::<Text, _>(id)
            .bind::<BigInt, _>(now + i)
            .execute(&mut db)
            .await
            .unwrap();
    }
    // Healthy and failed Room events can also share a single AS transaction.
    let body = json!({"events":[raw(bad,"$mixed_failed"),raw(good,"$healthy")]});
    sql_query(
        "INSERT INTO hagency_agent_v1.inbound_transactions VALUES('healthy_last','digest',$1,$2)",
    )
    .bind::<Jsonb, _>(body)
    .bind::<BigInt, _>(now + 20)
    .execute(&mut db)
    .await
    .unwrap();
    sql_query("INSERT INTO hagency_agent_v1.routing_jobs VALUES('healthy_last','pending',$1)")
        .bind::<BigInt, _>(now + 20)
        .execute(&mut db)
        .await
        .unwrap();
    let mut cursor = String::new();
    for _ in 0..4 {
        route(&inbox, &transport, &gateway, &mut cursor)
            .await
            .unwrap();
    }
    let scopes = transport.routing_scopes(good).await.unwrap();
    assert_eq!(scopes.len(), 1);
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type=BigInt)]
        n: i64,
    }
    let routed = sql_query(
        "SELECT count(*) AS n FROM hagency_agent_v1.owner_events WHERE event_id='$healthy'",
    )
    .get_result::<Count>(&mut db)
    .await
    .unwrap();
    assert_eq!(routed.n, 1);
    assert_eq!(
        sql_query("SELECT count(*) AS n FROM hagency_agent_v1.routing_jobs WHERE state='pending'")
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .n,
        13,
        "transient transactions must remain retryable, including mixed transaction"
    );
    let device = auth
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "fair-device".into(),
                name: "Fixture".into(),
            },
            now,
        )
        .await
        .unwrap();
    let p = auth.authenticate(&device.token, now, true).await.unwrap();
    let lease = transport
        .acquire_for_test(&p, &created.agent.id, 60_000, false, now)
        .await
        .unwrap();
    let lease_ref = LeaseRef {
        agent_id: created.agent.id.clone(),
        epoch: lease.epoch,
    };
    let delivery = |room: &str| DeliveryFacts {
        owner_direct_valid: false,
        owner_mxid: p.mxid.clone(),
        requester_mxid: p.mxid.clone(),
        puppet_mxid: created.agent.puppet_mxid.clone(),
        room_id: room.into(),
        space_id: space.into(),
        observed_at_ms: now_ms(),
        owner_in_space: true,
        owner_in_room: true,
        requester_in_room: true,
        puppet_in_room: true,
        puppet_can_send_message: true,
        room_in_space: true,
        encrypted: false,
    };
    let mut healthy_reply = None;
    let mut healthy_processing = None;
    for i in 0..22 {
        let (room, binding) = if i == 21 {
            (good, &bound.binding.id)
        } else {
            (bad, &created.binding.id)
        };
        let event = parse_event(&raw(room, &format!("$reply{i}"))).unwrap();
        let facts = delivery(room);
        let dispatch = match transport
            .ingest_routed(binding, event, &facts, now_ms())
            .await
            .unwrap()
        {
            crate::transport::RouteResult::Queued { dispatch_id } => dispatch_id,
            _ => panic!("expected fixture event queue"),
        };
        transport
            .claim_event(&p, &lease_ref, &dispatch, &facts, now_ms())
            .await
            .unwrap();
        transport
            .acknowledge(&p, &lease_ref, &dispatch, now_ms())
            .await
            .unwrap();
        let exec = format!("fair-exec-{i}");
        transport
            .start_execution(&p, &lease_ref, &dispatch, &exec, &facts, now_ms())
            .await
            .unwrap();
        if i == 21 {
            healthy_processing = Some(
                transport
                    .mark_processing(&p, &lease_ref, &dispatch, &exec, &facts, now_ms())
                    .await
                    .unwrap(),
            );
        }
        let reply = transport
            .submit_reply(
                &p,
                &lease_ref,
                SubmitReply {
                    dispatch_id: dispatch,
                    execution_id: exec,
                    body: "No model was invoked".into(),
                },
                &facts,
                now_ms(),
            )
            .await
            .unwrap();
        if i == 21 {
            healthy_reply = Some(reply);
        }
    }
    let original = healthy_reply.unwrap();
    // Refused local TCP connection makes a definitely attempted send unknown.
    let matrix = MatrixClient::new(
        "http://127.0.0.1:1".parse().unwrap(),
        "fixture-AS-token-no-real-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let mut cursor = String::new();
    for _ in 0..4 {
        send(&transport, &gateway, &matrix, &mut cursor)
            .await
            .unwrap();
    }
    let replies = transport.reply_candidates(100, now_ms()).await.unwrap();
    let retried = replies.iter().find(|r| r.id == original.id).unwrap();
    assert_eq!(retried.state, "unknown");
    assert_eq!(retried.matrix_txn_id, original.matrix_txn_id);
    assert_eq!(retried.body, original.body);
    assert_eq!(retried.owner_event_id, original.owner_event_id);
    assert_eq!(
        sql_query(
            "SELECT count(*) AS n FROM hagency_agent_v1.owner_events WHERE state='completed'"
        )
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .n,
        22,
        "fair retries must not reopen model executions"
    );
    assert_eq!(
        sql_query("SELECT count(*) AS n FROM hagency_agent_v1.reply_outbox WHERE state='pending'")
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .n,
        21,
        "unavailable Room intents were not claimed or cancelled"
    );
    // The same real worker must distinguish an authoritative Matrix 403 from
    // transport ambiguity, including an earlier unknown attempt for this txn.
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0; 8192];
            if socket.read(&mut request).await.unwrap_or(0) == 0 {
                continue;
            }
            observed.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"errcode":"M_FORBIDDEN","error":"fixture permission denied"}"#;
            let response = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let denied_matrix = MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "fixture-AS-token-no-real-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    for _ in 0..4 {
        send(&transport, &gateway, &denied_matrix, &mut cursor)
            .await
            .unwrap();
    }
    assert_eq!(
        calls.as_ref().load(Ordering::SeqCst),
        1,
        "authoritative 403 must not be blindly retried as unknown"
    );
    assert!(
        transport
            .reply_candidates(100, now_ms())
            .await
            .unwrap()
            .iter()
            .all(|r| r.id != original.id)
    );
    let held = sql_query("SELECT count(*) AS n FROM hagency_agent_v1.reply_outbox WHERE id=$1 AND delivery_blocked AND state='unknown' AND matrix_txn_id=$2 AND body=$3")
        .bind::<Text,_>(&original.id).bind::<Text,_>(&original.matrix_txn_id).bind::<Text,_>(&original.body).get_result::<Count>(&mut db).await.unwrap();
    assert_eq!(held.n, 1);
    let processing = healthy_processing.unwrap();
    let mut processing_cursor = String::new();
    send_processing(&transport, &gateway, &matrix, &mut processing_cursor)
        .await
        .unwrap();
    let reaction = transport
        .processing_candidates("", 100, now_ms())
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == processing.id)
        .unwrap();
    assert_eq!(reaction.state, "unknown");
    let txn = reaction.matrix_txn_id.clone();
    let content = reaction.content.clone();
    assert_eq!(
        content,
        json!({"m.relates_to":{"rel_type":"m.annotation","event_id":"$reply21","key":"\u{1f440}"}})
    );
    for _ in 0..4 {
        send_processing(&transport, &gateway, &denied_matrix, &mut processing_cursor)
            .await
            .unwrap();
    }
    assert_eq!(
        calls.as_ref().load(Ordering::SeqCst),
        2,
        "processing 403 is held and never blindly resent"
    );
    assert!(
        transport
            .processing_candidates("", 100, now_ms())
            .await
            .unwrap()
            .iter()
            .all(|r| r.id != processing.id)
    );
    let held=sql_query("SELECT count(*) AS n FROM hagency_agent_v1.processing_outbox WHERE id=$1 AND state='unknown' AND delivery_blocked AND matrix_txn_id=$2 AND content=$3")
        .bind::<Text,_>(&processing.id).bind::<Text,_>(txn).bind::<Jsonb,_>(content).get_result::<Count>(&mut db).await.unwrap();
    assert_eq!(held.n, 1);
    // Per-request failures must deliver feedback even when another Room is
    // unavailable and after the executing device has released its lease.
    for i in 0..22 {
        let (room, binding) = if i == 21 {
            (good, &bound.binding.id)
        } else {
            (bad, &created.binding.id)
        };
        let facts = delivery(room);
        let event = parse_event(&raw(room, &format!("$failure{i}"))).unwrap();
        let dispatch = match transport
            .ingest_routed(binding, event, &facts, now_ms())
            .await
            .unwrap()
        {
            crate::transport::RouteResult::Queued { dispatch_id } => dispatch_id,
            _ => panic!("fixture queue"),
        };
        transport
            .claim_event(&p, &lease_ref, &dispatch, &facts, now_ms())
            .await
            .unwrap();
        transport
            .acknowledge(&p, &lease_ref, &dispatch, now_ms())
            .await
            .unwrap();
        let exec = format!("failure-{i}");
        transport
            .start_execution(&p, &lease_ref, &dispatch, &exec, &facts, now_ms())
            .await
            .unwrap();
        transport
            .finish_without_reply(&p, &lease_ref, &dispatch, &exec, "failed", now_ms())
            .await
            .unwrap();
    }
    transport
        .release_lease(&p, &lease_ref, now_ms())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let contents = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let captured = contents.clone();
    let status_server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&bytes);
                let Some((headers, body)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if body.len() < length {
                    continue;
                }
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(body).unwrap());
                let body = r#"{"event_id":"$status-sent:example.test"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                break;
            }
        }
    });
    let status_matrix = MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "fixture-AS-token-no-real-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let mut status_cursor = String::new();
    for _ in 0..4 {
        send_request_notices(&transport, &gateway, &status_matrix, &mut status_cursor)
            .await
            .unwrap();
    }
    {
        let messages = contents.lock().unwrap();
        assert_eq!(
            messages.len(),
            1,
            "unavailable Rooms cannot starve the healthy failure notice"
        );
        assert_eq!(messages[0]["msgtype"], "m.notice");
        assert_eq!(
            messages[0]["m.relates_to"]["m.in_reply_to"]["event_id"],
            "$failure21"
        );
        assert_eq!(
            messages[0]["body"],
            crate::transport::request_notices::body("failed")
        );
    }
    let sent = sql_query(
        "SELECT count(*) AS n FROM hagency_agent_v1.request_notice_outbox WHERE state='sent'",
    )
    .get_result::<Count>(&mut db)
    .await
    .unwrap();
    assert_eq!(sent.n, 1);
    // Keep routing stuck in a real Gateway future beyond several clock ticks.
    // A failure accepted after the first tick still has to be delivered by the
    // independent status queue, before that routing proof times out.
    sql_query("UPDATE hagency_agent_v1.request_notice_outbox n SET state='cancelled' FROM hagency_agent_v1.owner_events e WHERE n.owner_event_id=e.id AND e.room_id=$1 AND n.state='pending'")
        .bind::<Text,_>(bad).execute(&mut db).await.unwrap();
    let slow_seen = std::sync::Arc::new(tokio::sync::Notify::new());
    let active_reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = slow_seen.clone();
    let active = active_reads.clone();
    let slow_state = vec![
        json!({"type":"m.room.member","state_key":owner.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":created.agent.puppet_mxid,"content":{"membership":"join"}}),
        json!({"type":"m.space.child","state_key":good,"content":{"via":["example.test"]}}),
    ];
    let slow_gateway = Gateway::new(
        std::sync::Arc::new(move |room| {
            let state = slow_state.clone();
            let seen = seen.clone();
            let active = active.clone();
            Box::pin(async move {
                if room == bad {
                    struct ActiveRead(std::sync::Arc<std::sync::atomic::AtomicUsize>);
                    impl Drop for ActiveRead {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _guard = ActiveRead(active);
                    seen.notify_one();
                    std::future::pending::<()>().await;
                }
                Ok(state)
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let worker = start(
        inbox.clone(),
        transport.clone(),
        slow_gateway,
        status_matrix.clone(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), slow_seen.notified())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let fresh_lease = LeaseRef {
        agent_id: created.agent.id.clone(),
        epoch: transport
            .acquire_for_test(&p, &created.agent.id, 30_000, false, now_ms())
            .await
            .unwrap()
            .epoch,
    };
    let facts = delivery(good);
    let dispatch = match transport
        .ingest_routed(
            &bound.binding.id,
            parse_event(&raw(good, "$failure-after-slow-tick")).unwrap(),
            &facts,
            now_ms(),
        )
        .await
        .unwrap()
    {
        crate::transport::RouteResult::Queued { dispatch_id } => dispatch_id,
        _ => panic!("independent worker fixture queue"),
    };
    transport
        .claim_event(&p, &fresh_lease, &dispatch, &facts, now_ms())
        .await
        .unwrap();
    transport
        .acknowledge(&p, &fresh_lease, &dispatch, now_ms())
        .await
        .unwrap();
    transport
        .start_execution(
            &p,
            &fresh_lease,
            &dispatch,
            "independent-status-run",
            &facts,
            now_ms(),
        )
        .await
        .unwrap();
    transport
        .finish_without_reply(
            &p,
            &fresh_lease,
            &dispatch,
            "independent-status-run",
            "failed",
            now_ms(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if contents.lock().unwrap().iter().any(|m| {
                m["m.relates_to"]["m.in_reply_to"]["event_id"] == "$failure-after-slow-tick"
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a slow routing pass must not block the next status tick");
    assert!(
        active_reads
            .as_ref()
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    );
    worker.abort();
    let _ = worker.await;
    assert_eq!(
        active_reads
            .as_ref()
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "parent shutdown must drop all queue futures"
    );
    status_server.abort();
    let _ = status_server.await;
    server.abort();
    let _ = server.await;
}
