use super::*;
use crate::{
    domain::{AdminFacts, CreateBoundAgent, DomainStore, RoomFacts},
    identity::Identity,
    store::{RegisterDevice, Store},
};
struct Fixture {
    auth: Store,
    domain: DomainStore,
    transport: TransportStore,
    credential: String,
    device_token: String,
    p: Principal,
    second: Principal,
    agent: String,
    binding: String,
    facts: DeliveryFacts,
}
async fn fixture() -> Fixture {
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let auth = Store::open(&url, "example.test", "https://example.test/_pasion/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let transport = TransportStore::open(&url, Limits::default()).await.unwrap();
    let now = crate::api::now_ms();
    let suffix = secret_token();
    let grant = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/_pasion/".into(),
                subject: format!("transport-{suffix}"),
                mxid: format!("@transport_{suffix}:example.test"),
                client_id: "transport-client".into(),
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
                installation_id: "first".into(),
                name: "First client".into(),
            },
            now,
        )
        .await
        .unwrap();
    let user = auth.authenticate(&initial.token, now, true).await.unwrap();
    let space = format!("!transport_space_{suffix}:example.test");
    let room = format!("!transport_room_{suffix}:example.test");
    let sf = AdminFacts {
        actor_mxid: user.mxid.clone(),
        room_id: space.clone(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let rf = AdminFacts {
        actor_mxid: user.mxid.clone(),
        room_id: room.clone(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space: false,
        linked_space_id: Some(space.clone()),
    };
    let project = domain
        .register_project(&user, &space, &sf, now)
        .await
        .unwrap();
    crate::assert_entity_id(&project.id, "prj_");
    domain
        .register_room(&user, &project.id, &room, &sf, &rf, now)
        .await
        .unwrap();
    let mut facts = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: user.mxid.clone(),
        room_id: room.clone(),
        space_id: space.clone(),
        observed_at_ms: now,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: None,
        puppet_in_room: false,
        encrypted: false,
    };
    let created = domain
        .create_bound_agent(
            &user,
            CreateBoundAgent {
                project_id: project.id,
                room_id: room.clone(),
                display_name: "Transport Codex".into(),
                idempotency_key: "create".into(),
            },
            &facts,
            now,
        )
        .await
        .unwrap();
    facts.puppet_mxid = Some(created.agent.puppet_mxid.clone());
    crate::assert_entity_id(&created.agent.id, "agt_");
    crate::assert_entity_id(&created.binding.id, "bnd_");
    facts.puppet_in_room = true;
    domain
        .activate_binding(
            &user,
            &created.binding.id,
            created.binding.generation,
            &facts,
            now,
        )
        .await
        .unwrap();
    let first = auth
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "first".into(),
                name: "First client".into(),
            },
            now,
        )
        .await
        .unwrap();
    let second = auth
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "second".into(),
                name: "Second client".into(),
            },
            now,
        )
        .await
        .unwrap();
    let p = auth.authenticate(&first.token, now, true).await.unwrap();
    let second = auth.authenticate(&second.token, now, true).await.unwrap();
    let facts = DeliveryFacts {
        owner_direct_valid: false,
        owner_mxid: user.mxid,
        requester_mxid: "@requester:example.test".into(),
        puppet_mxid: created.agent.puppet_mxid,
        room_id: room,
        space_id: space,
        observed_at_ms: now,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        requester_in_room: true,
        puppet_in_room: true,
        puppet_can_send_message: true,
        encrypted: false,
    };
    Fixture {
        auth,
        domain,
        transport,
        credential: grant.token,
        device_token: first.token,
        p,
        second,
        agent: created.agent.id,
        binding: created.binding.id,
        facts,
    }
}
fn event(f: &Fixture, id: &str, mention: bool, thread: Option<&str>) -> RoutedEvent {
    RoutedEvent {
        event_id: format!("${id}"),
        room_id: f.facts.room_id.clone(),
        sender_mxid: f.facts.requester_mxid.clone(),
        body: "Run a task".into(),
        mentioned_mxids: if mention {
            [f.facts.puppet_mxid.clone()].into()
        } else {
            BTreeSet::new()
        },
        thread_root: thread.map(str::to_owned),
        encrypted: false,
        is_edit: false,
    }
}
async fn enqueue(f: &Fixture, id: &str) -> String {
    match f
        .transport
        .ingest_routed(
            &f.binding,
            event(f, id, true, None),
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap()
    {
        RouteResult::Queued { dispatch_id } => {
            crate::assert_entity_id(&dispatch_id, "evt_");
            dispatch_id
        }
        _ => panic!("expected queue insertion"),
    }
}
async fn running(f: &Fixture, lease: &LeaseRef, id: &str, execution: &str) {
    f.transport
        .claim_event(&f.p, lease, id, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .acknowledge(&f.p, lease, id, crate::api::now_ms())
        .await
        .unwrap();
    f.transport
        .start_execution(&f.p, lease, id, execution, &f.facts, crate::api::now_ms())
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_queue_ack_execution_reply_and_unknown_retry_are_distinct() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    // Client is not connected; ingestion is nevertheless durable.
    let id = enqueue(&f, "offline-initial").await;
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                event(&f, "offline-initial", true, None),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Duplicate { .. }
    ));
    let mut changed = event(&f, "offline-initial", true, None);
    changed.body = "changed".into();
    assert!(
        f.transport
            .ingest_routed(&f.binding, changed, &f.facts, now)
            .await
            .is_err()
    );
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                event(&f, "ordinary", false, None),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    f.domain
        .set_thread_auto_reply(&f.p, &f.binding, true, now)
        .await
        .unwrap();
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                event(&f, "thread-followup", false, Some("$offline-initial")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Queued { .. }
    ));
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    assert!(
        f.transport
            .acquire_for_test(&f.second, &f.agent, 30_000, false, now)
            .await
            .is_err()
    );
    let claimed = f
        .transport
        .claim_event(&f.p, &reference, &id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(claimed.state, "offered");
    assert!(
        f.transport
            .start_execution(&f.p, &reference, &id, "exec-a", &f.facts, now)
            .await
            .is_err()
    );
    f.transport
        .acknowledge(&f.p, &reference, &id, now)
        .await
        .unwrap();
    f.transport
        .acknowledge(&f.p, &reference, &id, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .authorize_tool_execution(&f.p, &reference, &id, "exec-a", &f.facts, now)
            .await
            .is_err()
    );
    let active = f
        .transport
        .start_execution(&f.p, &reference, &id, "exec-a", &f.facts, now)
        .await
        .unwrap();
    assert_eq!(active.dispatch.state, "running");
    assert!(active.newly_started);
    assert_eq!(
        f.transport
            .authorize_tool_execution(&f.p, &reference, &id, "exec-a", &f.facts, now)
            .await
            .unwrap()
            .state,
        "running"
    );
    assert!(
        f.transport
            .authorize_tool_execution(&f.p, &reference, &id, "exec-other", &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .authorize_tool_execution(&f.second, &reference, &id, "exec-a", &f.facts, now)
            .await
            .is_err()
    );
    let mut tool_denied = f.facts.clone();
    tool_denied.requester_in_room = false;
    assert!(
        f.transport
            .authorize_tool_execution(&f.p, &reference, &id, "exec-a", &tool_denied, now)
            .await
            .is_err()
    );

    assert!(
        !f.transport
            .start_execution(&f.p, &reference, &id, "exec-a", &f.facts, now)
            .await
            .unwrap()
            .newly_started
    );
    assert!(
        f.transport
            .start_execution(&f.p, &reference, &id, "exec-other", &f.facts, now)
            .await
            .is_err()
    );
    let reply = || SubmitReply {
        dispatch_id: id.clone(),
        execution_id: "exec-a".into(),
        body: "Task complete".into(),
    };
    let committed = f
        .transport
        .submit_reply(&f.p, &reference, reply(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .submit_reply(&f.p, &reference, reply(), &f.facts, now)
            .await
            .unwrap()
            .matrix_txn_id,
        committed.matrix_txn_id
    );
    let mut changed = reply();
    changed.body = "Different reply".into();
    assert!(
        f.transport
            .submit_reply(&f.p, &reference, changed, &f.facts, now)
            .await
            .is_err()
    );
    let sending = f
        .transport
        .claim_reply(&committed.id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(sending.state, "sending");
    assert!(
        f.transport
            .claim_reply(&committed.id, &f.facts, now)
            .await
            .is_err()
    );
    let worker = sending.worker_token.as_deref().unwrap();
    let unknown = f
        .transport
        .confirm_reply(&sending.id, worker, None, now)
        .await
        .unwrap();
    assert_eq!(unknown.state, "unknown");
    let retry = f
        .transport
        .claim_reply(&committed.id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(retry.matrix_txn_id, sending.matrix_txn_id);
    assert_eq!(retry.body, sending.body);
    assert!(
        f.transport
            .confirm_reply(&retry.id, worker, Some("$result"), now)
            .await
            .is_err()
    );
    let worker = retry.worker_token.as_deref().unwrap();
    let sent = f
        .transport
        .confirm_reply(&retry.id, worker, Some("$result"), now)
        .await
        .unwrap();
    assert_eq!(sent.state, "sent");
    assert_eq!(
        f.transport
            .confirm_reply(&retry.id, worker, Some("$result"), now)
            .await
            .unwrap()
            .matrix_event_id
            .as_deref(),
        Some("$result")
    );
    assert!(
        f.transport
            .confirm_reply(&retry.id, worker, Some("$other"), now)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_takeover_and_device_revocation_fence_old_execution() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let run = enqueue(&f, "running").await;
    let ack = enqueue(&f, "ack-only").await;
    running(&f, &reference, &run, "exec-old").await;
    f.transport
        .claim_event(&f.p, &reference, &ack, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .acknowledge(&f.p, &reference, &ack, now)
        .await
        .unwrap();
    let instance = f.domain.agent(&f.p, &f.agent, now).await.unwrap();
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: instance.generation,
            },
            now,
        )
        .await
        .unwrap();
    let second = f
        .transport
        .acquire_for_test(&f.second, &f.agent, 30_000, true, now)
        .await
        .unwrap();
    assert!(second.epoch > reference.epoch);
    let second_ref = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: second.epoch,
    };
    assert!(
        f.transport
            .renew_lease(&f.p, &reference, 30_000, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .submit_reply(
                &f.p,
                &reference,
                SubmitReply {
                    dispatch_id: run.clone(),
                    execution_id: "exec-old".into(),
                    body: "Stale reply".into()
                },
                &f.facts,
                now
            )
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_event(&f.second, &second_ref, &run, &f.facts, now)
            .await
            .is_err()
    );
    let reclaimed = f
        .transport
        .claim_event(&f.second, &second_ref, &ack, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(reclaimed.state, "offered");
    let mut db = f.transport.db.lock().await;
    let state =
        sql_query("SELECT state AS id,digest FROM hagency_agent_v1.owner_events WHERE id=$1")
            .bind::<Text, _>(&run)
            .get_result::<Existing>(&mut *db)
            .await
            .unwrap();
    assert_eq!(state.id, "unknown");
    drop(db);
    f.auth
        .revoke_device(&f.credential, f.second.device_id.as_deref().unwrap(), now)
        .await
        .unwrap();
    assert!(
        f.transport
            .event_candidates(&f.second, &second_ref, 10, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .acknowledge(&f.second, &second_ref, &ack, now)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_binding_pause_or_requester_leave_blocks_reply_and_worker() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "permission").await;
    running(&f, &reference, &id, "exec-permission").await;
    let mut left = f.facts.clone();
    left.requester_in_room = false;
    assert!(
        f.transport
            .submit_reply(
                &f.p,
                &reference,
                SubmitReply {
                    dispatch_id: id.clone(),
                    execution_id: "exec-permission".into(),
                    body: "Reply".into()
                },
                &left,
                now
            )
            .await
            .is_err()
    );
    let committed = f
        .transport
        .submit_reply(
            &f.p,
            &reference,
            SubmitReply {
                dispatch_id: id.clone(),
                execution_id: "exec-permission".into(),
                body: "Reply".into(),
            },
            &f.facts,
            now,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_reply(&committed.id, &left, now)
            .await
            .is_err()
    );
    f.domain
        .suspend_binding(&f.p, &f.binding, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_reply(&committed.id, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_event(&f.p, &reference, &id, &f.facts, now)
            .await
            .is_err()
    );
}
#[test]
fn transport_dtos_do_not_accept_claimed_owner_sender_or_room() {
    assert!(serde_json::from_value::<SubmitReply>(serde_json::json!({"dispatchId":"d","executionId":"e","body":"reply","ownerUserId":"fake","roomId":"!fake:s"})).is_err());
    assert!(
        serde_json::from_value::<LeaseRef>(
            serde_json::json!({"agentId":"a","epoch":1,"ownerUserId":"fake"})
        )
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_transport_owner_isolation_and_terminal_unknown_constraints() {
    let f = fixture().await;
    let stranger = fixture().await;
    let now = crate::api::now_ms();
    assert!(
        f.transport
            .acquire_for_test(&stranger.p, &f.agent, 30_000, true, now)
            .await
            .is_err()
    );
    let granted = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: granted.epoch,
    };
    let id = enqueue(&f, "unknown-terminal").await;
    assert!(
        f.transport
            .owned_event_header(&stranger.p, &id, now)
            .await
            .is_err()
    );
    let header = f
        .transport
        .owned_event_header(&f.p, &id, now)
        .await
        .unwrap();
    assert_eq!(header.requester_mxid, f.facts.requester_mxid);
    let scopes = f.transport.routing_scopes(&f.facts.room_id).await.unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].binding_id, f.binding);
    assert_eq!(
        f.transport
            .binding_scope(&f.binding)
            .await
            .unwrap()
            .owner_user_id,
        f.p.user_id
    );
    running(&f, &lease, &id, "exec-unknown").await;
    f.transport
        .finish_without_reply(&f.p, &lease, &id, "exec-unknown", "unknown", now)
        .await
        .unwrap();
    f.transport
        .finish_without_reply(&f.p, &lease, &id, "exec-unknown", "unknown", now)
        .await
        .unwrap();
    let mut db = f.transport.db.lock().await;
    assert!(
        sql_query("UPDATE hagency_agent_v1.owner_events SET state='pending' WHERE id=$1")
            .bind::<Text, _>(&id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    assert!(
        sql_query("UPDATE hagency_agent_v1.owner_events SET owner_user_id=$1 WHERE id=$2")
            .bind::<Text, _>(&stranger.p.user_id)
            .bind::<Text, _>(&id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    assert!(
        sql_query("UPDATE hagency_agent_v1.execution_leases SET device_id=$1 WHERE agent_id=$2")
            .bind::<Text, _>(stranger.p.device_id.as_deref().unwrap())
            .bind::<Text, _>(&f.agent)
            .execute(&mut *db)
            .await
            .is_err()
    );
    drop(db);
    assert!(
        f.transport
            .start_execution(&f.p, &lease, &id, "new-attempt", &f.facts, now)
            .await
            .is_err()
    );
    let queued = enqueue(&f, "queued-scope-removal").await;
    f.domain
        .suspend_binding(&f.p, &f.binding, now)
        .await
        .unwrap();
    assert!(f.transport.invalidate_stale_dispatches(now).await.unwrap() > 0);
    let mut db = f.transport.db.lock().await;
    let cancelled =
        sql_query("SELECT state AS id,digest FROM hagency_agent_v1.owner_events WHERE id=$1")
            .bind::<Text, _>(&queued)
            .get_result::<Existing>(&mut *db)
            .await
            .unwrap();
    assert_eq!(cancelled.id, "cancelled");
    drop(db);
    assert!(
        f.transport
            .routing_scopes(&f.facts.room_id)
            .await
            .unwrap()
            .iter()
            .all(|scope| !scope.active)
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_definitive_member_denial_does_not_starve_other_events_and_preserves_unknown_reply()
 {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let grant = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: grant.epoch,
    };
    let blocked = enqueue(&f, "member-left").await;
    let valid = enqueue(&f, "member-valid").await;
    assert!(
        !f.transport
            .reject_event_delivery(&blocked, &f.facts, now)
            .await
            .unwrap()
    );
    let mut left = f.facts.clone();
    left.requester_in_room = false;
    let mut mismatched = left.clone();
    mismatched.owner_mxid = "@wrong:example.test".into();
    assert!(
        f.transport
            .reject_event_delivery(&blocked, &mismatched, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reject_event_delivery(&blocked, &left, now)
            .await
            .unwrap()
    );
    let candidates = f
        .transport
        .event_candidates(&f.p, &lease, 1, now)
        .await
        .unwrap();
    assert_eq!(candidates[0].id, valid);
    running(&f, &lease, &valid, "exec-network-unknown").await;
    let intent = f
        .transport
        .submit_reply(
            &f.p,
            &lease,
            SubmitReply {
                dispatch_id: valid,
                execution_id: "exec-network-unknown".into(),
                body: "Keep exact unknown intent".into(),
            },
            &f.facts,
            now,
        )
        .await
        .unwrap();
    let sending = f
        .transport
        .claim_reply(&intent.id, &f.facts, now)
        .await
        .unwrap();
    let worker = sending.worker_token.as_deref().unwrap();
    f.transport
        .confirm_reply(&intent.id, worker, None, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .block_reply_delivery(&intent.id, &left, now)
            .await
            .unwrap()
    );
    assert!(
        f.transport
            .reply_candidates(100, now)
            .await
            .unwrap()
            .into_iter()
            .all(|reply| reply.id != intent.id)
    );
    assert!(
        f.transport
            .claim_reply(&intent.id, &f.facts, now)
            .await
            .is_err()
    );
    let mut db = f.transport.db.lock().await;
    let preserved = TransportStore::reply(&mut db, &intent.id).await.unwrap();
    assert_eq!(preserved.state, "unknown");
    assert_eq!(preserved.matrix_txn_id, intent.matrix_txn_id);
    assert_eq!(preserved.body, intent.body);
    assert!(
        sql_query("UPDATE hagency_agent_v1.reply_outbox SET delivery_blocked=false WHERE id=$1")
            .bind::<Text, _>(&intent.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    drop(db);
    let mut observed = ObservedReply {
        event_id: "$proven-historical-send".into(),
        matrix_txn_id: intent.matrix_txn_id.clone(),
        sender_mxid: intent.puppet_mxid.clone(),
        room_id: intent.room_id.clone(),
        thread_root: intent.thread_root.clone(),
        body: intent.body.clone(),
        observed_at_ms: crate::api::now_ms(),
    };
    observed.matrix_txn_id = "wrong-txn".into();
    assert!(
        f.transport
            .reconcile_reply_sent(&intent.id, &observed, now)
            .await
            .is_err()
    );
    observed.matrix_txn_id = intent.matrix_txn_id.clone();
    let recovered = f
        .transport
        .reconcile_reply_sent(&intent.id, &observed, now)
        .await
        .unwrap();
    assert_eq!(recovered.state, "sent");
    assert!(recovered.delivery_blocked);
    assert_eq!(
        recovered.matrix_event_id.as_deref(),
        Some("$proven-historical-send")
    );
    assert!(
        f.transport
            .claim_reply(&intent.id, &f.facts, now)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_known_reply_recovers_under_new_epoch_without_reexecuting_unknown() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let original = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let old = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: original.epoch,
    };
    let id = enqueue(&f, "known-after-takeover").await;
    running(&f, &old, &id, "known-execution").await;
    let instance = f.domain.agent(&f.p, &f.agent, now).await.unwrap();
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: instance.generation,
            },
            now,
        )
        .await
        .unwrap();
    let current = f
        .transport
        .acquire_for_test(&f.second, &f.agent, 60_000, true, now)
        .await
        .unwrap();
    let new = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: current.epoch,
    };
    let request = || SubmitReply {
        dispatch_id: id.clone(),
        execution_id: "known-execution".into(),
        body: "Locally durable result".into(),
    };
    assert!(
        f.transport
            .submit_reply(&f.p, &old, request(), &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .submit_reply(&f.second, &new, request(), &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reconcile_known_reply(&f.p, &old, request(), &f.facts, now)
            .await
            .is_err()
    );
    let mut denied = f.facts.clone();
    denied.requester_in_room = false;
    assert!(
        f.transport
            .reconcile_known_reply(&f.second, &new, request(), &denied, now)
            .await
            .is_err()
    );
    let intent = f
        .transport
        .reconcile_known_reply(&f.second, &new, request(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(intent.dispatch_epoch, old.epoch);
    crate::assert_entity_id(&intent.id, "rep_");
    assert_eq!(intent.delivery_epoch, new.epoch);
    assert_eq!(intent.matrix_txn_id, format!("hagency_{}", hash(&id)));
    let replay = f
        .transport
        .reconcile_known_reply(&f.second, &new, request(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(replay.id, intent.id);
    let mut changed = request();
    changed.body = "Changed result".into();
    assert!(
        f.transport
            .reconcile_known_reply(&f.second, &new, changed, &f.facts, now)
            .await
            .is_err()
    );
    let mut db = f.transport.db.lock().await;
    let dispatch = TransportStore::dispatch(&mut db, &f.second, &id)
        .await
        .unwrap();
    assert_eq!(dispatch.state, "unknown");
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.owner_events SET execution_id='changed_original' WHERE id=$1"
        )
        .bind::<Text, _>(&id)
        .execute(&mut *db)
        .await
        .is_err()
    );
    assert_eq!(dispatch.dispatch_epoch, Some(old.epoch));
    assert_eq!(dispatch.execution_id.as_deref(), Some("known-execution"));
    drop(db);
    assert!(
        f.transport
            .start_execution(&f.second, &new, &id, "known-execution", &f.facts, now)
            .await
            .is_err()
    );
    let send = f
        .transport
        .claim_reply(&intent.id, &f.facts, now)
        .await
        .unwrap();
    let sent = f
        .transport
        .confirm_reply(
            &send.id,
            send.worker_token.as_deref().unwrap(),
            Some("$known-reconciled"),
            now,
        )
        .await
        .unwrap();
    assert_eq!(sent.state, "sent");
    let historical = f
        .transport
        .reconcile_known_reply(&f.second, &new, request(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(
        historical.matrix_event_id.as_deref(),
        Some("$known-reconciled")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_known_reply_reauthorizes_only_unsent_cancel_and_preserves_ambiguous_txn() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let original = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let old = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: original.epoch,
    };
    let pending_id = enqueue(&f, "cancelled-known").await;
    running(&f, &old, &pending_id, "cancelled-execution").await;
    let pending_request = || SubmitReply {
        dispatch_id: pending_id.clone(),
        execution_id: "cancelled-execution".into(),
        body: "Pending known text".into(),
    };
    let pending = f
        .transport
        .submit_reply(&f.p, &old, pending_request(), &f.facts, now)
        .await
        .unwrap();
    let unknown_id = enqueue(&f, "ambiguous-known").await;
    running(&f, &old, &unknown_id, "ambiguous-execution").await;
    let unknown_request = || SubmitReply {
        dispatch_id: unknown_id.clone(),
        execution_id: "ambiguous-execution".into(),
        body: "Ambiguous known text".into(),
    };
    let unknown = f
        .transport
        .submit_reply(&f.p, &old, unknown_request(), &f.facts, now)
        .await
        .unwrap();
    let active_send = f
        .transport
        .claim_reply(&unknown.id, &f.facts, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .reconcile_known_reply(&f.p, &old, unknown_request(), &f.facts, now)
            .await
            .is_err()
    );
    f.transport
        .confirm_reply(
            &active_send.id,
            active_send.worker_token.as_deref().unwrap(),
            None,
            now,
        )
        .await
        .unwrap();
    f.transport.release_lease(&f.p, &old, now).await.unwrap();
    let instance = f.domain.agent(&f.p, &f.agent, now).await.unwrap();
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: instance.generation,
            },
            now,
        )
        .await
        .unwrap();
    let current = f
        .transport
        .acquire_for_test(&f.second, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let new = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: current.epoch,
    };
    let recovered = f
        .transport
        .reconcile_known_reply(&f.second, &new, pending_request(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(recovered.id, pending.id);
    assert_eq!(recovered.state, "pending");
    assert_eq!(recovered.matrix_txn_id, pending.matrix_txn_id);
    assert_eq!(recovered.dispatch_epoch, old.epoch);
    assert_eq!(recovered.delivery_epoch, new.epoch);
    let uncertain = f
        .transport
        .reconcile_known_reply(&f.second, &new, unknown_request(), &f.facts, now)
        .await
        .unwrap();
    assert_eq!(uncertain.id, unknown.id);
    assert_eq!(uncertain.state, "unknown");
    assert_eq!(uncertain.matrix_txn_id, unknown.matrix_txn_id);
    assert_eq!(uncertain.delivery_epoch, new.epoch);
    let candidates = f.transport.reply_candidates(100, now).await.unwrap();
    assert!(candidates.iter().any(|v| v.id == pending.id));
    assert!(candidates.iter().any(|v| v.id == unknown.id));
    let mut left = f.facts.clone();
    left.requester_in_room = false;
    assert!(
        f.transport
            .block_reply_delivery(&pending.id, &left, now)
            .await
            .unwrap()
    );
    assert!(
        f.transport
            .reconcile_known_reply(&f.second, &new, pending_request(), &f.facts, now)
            .await
            .is_err()
    );
    let mut db = f.transport.db.lock().await;
    assert!(
        sql_query("UPDATE hagency_agent_v1.reply_outbox SET delivery_epoch=$1 WHERE id=$2")
            .bind::<BigInt, _>(old.epoch)
            .bind::<Text, _>(&unknown.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    drop(db);
    let principal = f
        .auth
        .authenticate(&f.credential, now, false)
        .await
        .unwrap();
    f.domain
        .leave_binding(&principal, &f.binding, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .reconcile_known_reply(&f.second, &new, unknown_request(), &f.facts, now)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_speaking_permission_loss_is_durable_and_unknown_send_is_not_replayed() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let granted = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: granted.epoch,
    };
    let queued = enqueue(&f, "power-queued").await;
    let id = enqueue(&f, "power-unknown-send").await;
    running(&f, &lease, &id, "power-execution").await;
    let request = || SubmitReply {
        dispatch_id: id.clone(),
        execution_id: "power-execution".into(),
        body: "Exact known output".into(),
    };
    let intent = f
        .transport
        .submit_reply(&f.p, &lease, request(), &f.facts, now)
        .await
        .unwrap();
    let claimed = f
        .transport
        .claim_reply(&intent.id, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .confirm_reply(
            &intent.id,
            claimed.worker_token.as_deref().unwrap(),
            None,
            now,
        )
        .await
        .unwrap();
    let unknown_execution = enqueue(&f, "power-unknown-execution-without-outbox").await;
    running(&f, &lease, &unknown_execution, "power-unknown-execution").await;
    f.transport
        .finish_without_reply(
            &f.p,
            &lease,
            &unknown_execution,
            "power-unknown-execution",
            "unknown",
            now,
        )
        .await
        .unwrap();
    let mut denied = f.facts.clone();
    denied.puppet_can_send_message = false;
    assert!(
        f.transport
            .claim_reply(&intent.id, &denied, now)
            .await
            .is_err()
    );
    let stored = {
        let mut db = f.transport.db.lock().await;
        TransportStore::reply(&mut db, &intent.id).await.unwrap()
    };
    assert!(stored.delivery_blocked);
    assert_eq!(stored.state, "unknown");
    assert_eq!(stored.body, intent.body);
    assert_eq!(stored.matrix_txn_id, intent.matrix_txn_id);
    assert_eq!(stored.payload_digest, intent.payload_digest);
    // Rights restored later cannot turn the old ambiguous send into a new send.
    assert!(
        f.transport
            .claim_reply(&intent.id, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reconcile_known_reply(&f.p, &lease, request(), &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_event(&f.p, &lease, &queued, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reconcile_known_reply(
                &f.p,
                &lease,
                SubmitReply {
                    dispatch_id: unknown_execution,
                    execution_id: "power-unknown-execution".into(),
                    body: "Late known result".into()
                },
                &f.facts,
                now
            )
            .await
            .is_err()
    );
    // A new request after actual permission restoration remains usable.
    let fresh = enqueue(&f, "power-restored-new-request").await;
    assert!(
        f.transport
            .claim_event(&f.p, &lease, &fresh, &f.facts, now)
            .await
            .is_ok()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_ttl_blocks_stale_admission_and_tools_but_preserves_started_known_output() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let granted = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: granted.epoch,
    };
    let pending = enqueue(&f, "ttl-pending").await;
    let offered = enqueue(&f, "ttl-offered").await;
    f.transport
        .claim_event(&f.p, &lease, &offered, &f.facts, now)
        .await
        .unwrap();
    let ack = enqueue(&f, "ttl-acknowledged").await;
    f.transport
        .claim_event(&f.p, &lease, &ack, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .acknowledge(&f.p, &lease, &ack, now)
        .await
        .unwrap();
    let run = enqueue(&f, "ttl-started").await;
    running(&f, &lease, &run, "ttl-execution").await;
    {
        let mut db = f.transport.db.lock().await;
        for id in [&pending, &offered, &ack, &run] {
            sql_query("UPDATE hagency_agent_v1.owner_events SET created_at_ms=$1 WHERE id=$2")
                .bind::<BigInt, _>(now - f.transport.limits.event_ttl_ms)
                .bind::<Text, _>(id)
                .execute(&mut *db)
                .await
                .unwrap();
        }
    }
    // Passing a stale caller timestamp cannot extend server time or TTL.
    assert!(matches!(
        f.transport
            .claim_event(&f.p, &lease, &pending, &f.facts, 1)
            .await,
        Err(Error::Conflict("event_expired"))
    ));
    assert!(matches!(
        f.transport.acknowledge(&f.p, &lease, &offered, 1).await,
        Err(Error::Conflict("event_expired"))
    ));
    assert!(matches!(
        f.transport
            .start_execution(&f.p, &lease, &ack, "ttl-late", &f.facts, 1)
            .await,
        Err(Error::Conflict("event_expired"))
    ));
    assert!(matches!(
        f.transport
            .authorize_tool_execution(&f.p, &lease, &run, "ttl-execution", &f.facts, 1)
            .await,
        Err(Error::Conflict("event_expired"))
    ));
    let query = f
        .transport
        .start_execution(&f.p, &lease, &run, "ttl-execution", &f.facts, now)
        .await
        .unwrap();
    assert!(!query.newly_started);
    // An already produced result is not a new inference or tool invocation.
    let reply = f
        .transport
        .submit_reply(
            &f.p,
            &lease,
            SubmitReply {
                dispatch_id: run,
                execution_id: "ttl-execution".into(),
                body: "Already known output".into(),
            },
            &f.facts,
            now,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_reply(&reply.id, &f.facts, now)
            .await
            .is_ok()
    );
    assert!(
        f.transport
            .event_candidates(&f.p, &lease, 100, now)
            .await
            .unwrap()
            .is_empty()
    );
    let mut db = f.transport.db.lock().await;
    for id in [pending, offered, ack] {
        let record = TransportStore::dispatch(&mut db, &f.p, &id).await.unwrap();
        assert_eq!(record.state, "cancelled");
        assert_eq!(record.outcome.as_deref(), Some("expired"));
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_delayed_appservice_routing_never_resets_the_original_request_ttl() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    assert!(matches!(
        f.transport
            .ingest_routed_received(
                &f.binding,
                event(&f, "late-as-event", true, None),
                &f.facts,
                now - f.transport.limits.event_ttl_ms,
                now
            )
            .await,
        Err(Error::Conflict("event_expired"))
    ));
    assert!(matches!(
        f.transport
            .ingest_routed_received(
                &f.binding,
                event(&f, "future-receipt", true, None),
                &f.facts,
                now + 60_000,
                now
            )
            .await,
        Err(Error::Invalid("invalid_event_received_time"))
    ));
    let original = now - 10_000;
    let RouteResult::Queued { dispatch_id } = f
        .transport
        .ingest_routed_received(
            &f.binding,
            event(&f, "delayed-but-live", true, None),
            &f.facts,
            original,
            now,
        )
        .await
        .unwrap()
    else {
        panic!("live request not queued")
    };
    let mut db = f.transport.db.lock().await;
    let stored =
        sql_query("SELECT created_at_ms AS now_ms FROM hagency_agent_v1.owner_events WHERE id=$1")
            .bind::<Text, _>(&dispatch_id)
            .get_result::<Clock>(&mut *db)
            .await
            .unwrap();
    assert_eq!(stored.now_ms, original);
    let denied = sql_query("SELECT count(*) AS total FROM hagency_agent_v1.owner_events WHERE binding_id=$1 AND event_id IN ('$late-as-event','$future-receipt')").bind::<Text,_>(&f.binding).get_result::<Count>(&mut *db).await.unwrap();
    assert_eq!(denied.total, 0);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_selected_binding_is_filtered_before_limit_and_cannot_cross_owner_or_agent() {
    let f = fixture().await;
    let stranger = fixture().await;
    let now = crate::api::now_ms();
    let user = f
        .auth
        .authenticate(&f.credential, now, false)
        .await
        .unwrap();
    let binding = f.domain.binding(&user, &f.binding, now).await.unwrap();
    let room = format!("!other_{}:example.test", secret_token());
    let space_admin = AdminFacts {
        actor_mxid: user.mxid.clone(),
        room_id: f.facts.space_id.clone(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let room_admin = AdminFacts {
        room_id: room.clone(),
        is_space: false,
        linked_space_id: Some(f.facts.space_id.clone()),
        ..space_admin.clone()
    };
    f.domain
        .register_room(
            &user,
            binding.project_id.as_deref().unwrap(),
            &room,
            &space_admin,
            &room_admin,
            now,
        )
        .await
        .unwrap();
    let facts = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: user.mxid.clone(),
        room_id: room.clone(),
        space_id: f.facts.space_id.clone(),
        observed_at_ms: now,
        room_in_space: true,
        owner_in_space: true,
        owner_in_room: true,
        service_can_invite: true,
        puppet_mxid: Some(f.facts.puppet_mxid.clone()),
        puppet_in_room: true,
        encrypted: false,
    };
    let other = f
        .domain
        .bind_room(
            &user,
            &f.agent,
            crate::domain::BindRoom {
                project_id: binding.project_id.unwrap(),
                room_id: room.clone(),
                idempotency_key: "bind-second".into(),
            },
            &facts,
            now,
        )
        .await
        .unwrap();
    f.domain
        .activate_binding(
            &user,
            &other.binding.id,
            other.binding.generation,
            &facts,
            now,
        )
        .await
        .unwrap();
    for i in 0..3 {
        enqueue(&f, &format!("older-other-{i}")).await;
    }
    let second_facts = DeliveryFacts {
        owner_direct_valid: false,
        room_id: room.clone(),
        ..f.facts.clone()
    };
    let mut request = event(&f, "selected-room-event", true, None);
    request.room_id = room;
    let RouteResult::Queued { dispatch_id } = f
        .transport
        .ingest_routed(&other.binding.id, request, &second_facts, now)
        .await
        .unwrap()
    else {
        panic!("not queued")
    };
    let grant = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: grant.epoch,
    };
    let selected = f
        .transport
        .event_candidates_for_binding(&f.p, &lease, &other.binding.id, 1, now)
        .await
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, dispatch_id);
    assert!(
        f.transport
            .event_candidates_for_binding(&f.p, &lease, &stranger.binding, 1, now)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_failed_tool_authorization_still_commits_owner_membership_generation_fence() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let granted = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: granted.epoch,
    };
    let id = enqueue(&f, "owner-leave-during-tool").await;
    running(&f, &lease, &id, "owner-leave-exec").await;
    let mut denied = f.facts.clone();
    denied.owner_in_space = false;
    assert!(
        f.transport
            .authorize_tool_execution(&f.p, &lease, &id, "owner-leave-exec", &denied, now)
            .await
            .is_err()
    );
    let owner = f
        .auth
        .authenticate(&f.credential, now, false)
        .await
        .unwrap();
    let binding = f.domain.binding(&owner, &f.binding, now).await.unwrap();
    assert_eq!(binding.state, "leaving");
    let old = {
        let mut db = f.transport.db.lock().await;
        TransportStore::dispatch(&mut db, &f.p, &id).await.unwrap()
    };
    assert!(binding.generation > old.binding_generation);
    let restored = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: f.facts.owner_mxid.clone(),
        room_id: f.facts.room_id.clone(),
        space_id: f.facts.space_id.clone(),
        observed_at_ms: now,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: Some(f.facts.puppet_mxid.clone()),
        puppet_in_room: true,
        encrypted: false,
    };
    assert!(
        f.domain
            .resume_binding(&owner, &f.binding, &restored, now)
            .await
            .is_err()
    );
    let mut departed = restored.clone();
    departed.puppet_in_room = false;
    f.domain
        .confirm_left_trusted(&f.binding, binding.generation, &departed, now)
        .await
        .unwrap();
    let rebound = f
        .domain
        .bind_room(
            &owner,
            &f.agent,
            crate::domain::BindRoom {
                project_id: binding.project_id.clone().unwrap(),
                room_id: binding.room_id.clone(),
                idempotency_key: "tool-membership-rebind".into(),
            },
            &restored,
            now,
        )
        .await
        .unwrap();
    f.domain
        .activate_binding(
            &owner,
            &f.binding,
            rebound.binding.generation,
            &restored,
            now,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .authorize_tool_execution(&f.p, &lease, &id, "owner-leave-exec", &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .start_execution(&f.p, &lease, &id, "owner-leave-exec", &f.facts, now)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_matrix_403_after_fresh_check_is_a_permanent_hold_not_an_unknown_retry() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let granted = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: granted.epoch,
    };
    let id = enqueue(&f, "matrix-403-after-check").await;
    running(&f, &lease, &id, "matrix-403-execution").await;
    let request = || SubmitReply {
        dispatch_id: id.clone(),
        execution_id: "matrix-403-execution".into(),
        body: "Known reply whose send is denied".into(),
    };
    let intent = f
        .transport
        .submit_reply(&f.p, &lease, request(), &f.facts, now)
        .await
        .unwrap();
    let sending = f
        .transport
        .claim_reply(&intent.id, &f.facts, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .confirm_reply_permission_denied(&intent.id, "foreign-worker", now)
            .await
            .is_err()
    );
    let worker = sending.worker_token.as_deref().unwrap();
    let held = f
        .transport
        .confirm_reply_permission_denied(&intent.id, worker, now)
        .await
        .unwrap();
    assert!(held.delivery_blocked);
    assert_eq!(held.state, "unknown");
    assert_eq!(held.body, intent.body);
    assert_eq!(held.matrix_txn_id, intent.matrix_txn_id);
    let duplicate = f
        .transport
        .confirm_reply_permission_denied(&intent.id, worker, now)
        .await
        .unwrap();
    assert_eq!(duplicate.id, held.id);
    assert!(
        f.transport
            .claim_reply(&intent.id, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reconcile_known_reply(&f.p, &lease, request(), &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .reply_candidates(100, now)
            .await
            .unwrap()
            .iter()
            .all(|r| r.id != intent.id)
    );
}

#[path = "transport_history_tests.rs"]
mod history_tests;

#[path = "execution_device_tests.rs"]
mod execution_device_tests;

#[path = "owner_direct_tests.rs"]
mod owner_direct_tests;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_processing_reaction_is_fixed_durable_and_lease_fenced() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "processing-fixed-target").await;
    assert!(
        f.transport
            .mark_processing(&f.p, &lease, &id, "processing-run", &f.facts, now)
            .await
            .is_err()
    );
    running(&f, &lease, &id, "processing-run").await;
    assert!(
        f.transport
            .mark_processing(&f.second, &lease, &id, "processing-run", &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .mark_processing(&f.p, &lease, &id, "foreign-run", &f.facts, now)
            .await
            .is_err()
    );
    let receipt = f
        .transport
        .mark_processing(&f.p, &lease, &id, "processing-run", &f.facts, now)
        .await
        .unwrap();
    let again = f
        .transport
        .mark_processing(&f.p, &lease, &id, "processing-run", &f.facts, now)
        .await
        .unwrap();
    assert_eq!(receipt.id, again.id);
    let intent = f
        .transport
        .claim_processing(&receipt.id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(
        intent.content,
        serde_json::json!({"m.relates_to":{"rel_type":"m.annotation","event_id":"$processing-fixed-target","key":"👀"}})
    );
    let transaction = intent.matrix_txn_id.clone();
    let content = intent.content.clone();
    assert!(
        f.transport
            .confirm_processing(&intent.id, "wrong-worker", None, false, now)
            .await
            .is_err()
    );
    f.transport
        .confirm_processing(
            &intent.id,
            intent.worker_token.as_deref().unwrap(),
            None,
            false,
            now,
        )
        .await
        .unwrap();
    let retry = f
        .transport
        .claim_processing(&receipt.id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(retry.matrix_txn_id, transaction);
    assert_eq!(retry.content, content);
    f.transport
        .confirm_processing(
            &retry.id,
            retry.worker_token.as_deref().unwrap(),
            None,
            false,
            now,
        )
        .await
        .unwrap();
    // A short turn may finish before the sender's first scan; its old intent is
    // still a processing-start acknowledgement, not a new model authorization.
    f.transport
        .finish_without_reply(&f.p, &lease, &id, "processing-run", "failed", now)
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .mark_processing(&f.p, &lease, &id, "processing-run", &f.facts, now)
            .await
            .unwrap()
            .id,
        receipt.id
    );
    let retry = f
        .transport
        .claim_processing(&receipt.id, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .confirm_processing(
            &retry.id,
            retry.worker_token.as_deref().unwrap(),
            Some("$reaction-sent"),
            false,
            now,
        )
        .await
        .unwrap();
    let sent = f
        .transport
        .mark_processing(&f.p, &lease, &id, "processing-run", &f.facts, now)
        .await
        .unwrap();
    assert_eq!(sent.state, "sent");
    assert_eq!(sent.matrix_event_id.as_deref(), Some("$reaction-sent"));
    assert!(
        f.transport
            .claim_processing(&receipt.id, &f.facts, now)
            .await
            .is_err()
    );
    let terminal = enqueue(&f, "terminal-with-no-processing").await;
    running(&f, &lease, &terminal, "no-processing-run").await;
    f.transport
        .finish_without_reply(&f.p, &lease, &terminal, "no-processing-run", "failed", now)
        .await
        .unwrap();
    assert!(
        f.transport
            .mark_processing(&f.p, &lease, &terminal, "no-processing-run", &f.facts, now)
            .await
            .is_err()
    );
    let blocked = enqueue(&f, "processing-old-device").await;
    running(&f, &lease, &blocked, "old-device-run").await;
    let pending = f
        .transport
        .mark_processing(&f.p, &lease, &blocked, "old-device-run", &f.facts, now)
        .await
        .unwrap();
    let in_flight = f
        .transport
        .claim_processing(&pending.id, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .confirm_processing(
            &in_flight.id,
            in_flight.worker_token.as_deref().unwrap(),
            None,
            false,
            now,
        )
        .await
        .unwrap();
    let generation = f
        .domain
        .agent(&f.p, &f.agent, now)
        .await
        .unwrap()
        .generation;
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: generation,
            },
            now,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .mark_processing(&f.p, &lease, &blocked, "old-device-run", &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_processing(&pending.id, &f.facts, now)
            .await
            .is_err()
    );
    let mut db = f.transport.db.lock().await;
    assert!(
        sql_query("UPDATE hagency_agent_v1.processing_outbox SET content='{}'::jsonb WHERE id=$1")
            .bind::<Text, _>(&receipt.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.processing_outbox WHERE id=$1")
            .bind::<Text, _>(&receipt.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_processing_offline_cutover_is_standalone_exact_and_preserves_agent_state() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_processing_cutover_{}",
        &hash(&secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = reqwest::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
        let url=url.as_str();let now=crate::api::now_ms();
        let auth=Store::open(url,"example.test","https://example.test/_pasion/").await.unwrap();
        let domain=DomainStore::open(url,"example.test","_hagency_test_").await.unwrap();
        let transport=TransportStore::open(url,Limits::default()).await.unwrap();
        let grant=auth.sign_in(Identity{issuer:"https://example.test/_pasion/".into(),subject:"maintenance-owner".into(),mxid:"@maintenance:example.test".into(),client_id:"maintenance-fixture".into(),valid_until_ms:now+1_000_000},now).await.unwrap();
        let device=auth.register_device(&grant.token,RegisterDevice{installation_id:"maintenance-device".into(),name:"Maintenance".into()},now).await.unwrap();
        let p=auth.authenticate(&device.token,now,true).await.unwrap();
        let agent=domain.create_agent(&p,crate::domain::CreateAgent{display_name:"Permanent".into(),idempotency_key:"permanent-agent".into()},now).await.unwrap();
        domain.confirm_identity_provisioned(&agent.id,agent.generation).await.unwrap();
        transport.acquire_for_test(&p,&agent.id,60_000,false,now).await.unwrap();
        let mut db=AsyncPgConnection::establish(url).await.unwrap();
        #[derive(diesel::QueryableByName)] struct Snapshot {#[diesel(sql_type=Text)] id:String,#[diesel(sql_type=Text)] digest:String}
        let snapshot="SELECT (SELECT row_to_json(a)::text FROM hagency_agent_v1.agents a LIMIT 1) AS id,(SELECT row_to_json(l)::text FROM hagency_agent_v1.execution_leases l LIMIT 1) AS digest";
        let before=sql_query(snapshot).get_result::<Snapshot>(&mut db).await.unwrap();
        drop(transport);drop(domain);drop(auth);
        // Reconstruct the exact pre-feature v3 boundary in this isolated database.
        db.batch_execute("DROP VIEW hagency_agent_v1.processing_sendable; DROP TABLE hagency_agent_v1.processing_outbox; DROP FUNCTION hagency_agent_v1.retain_processing_scope(); DROP FUNCTION hagency_agent_v1.validate_processing_scope(); ALTER TABLE hagency_agent_v1.domain_deployment DROP CONSTRAINT domain_deployment_version_check; UPDATE hagency_agent_v1.domain_deployment SET version=3; ALTER TABLE hagency_agent_v1.domain_deployment ADD CONSTRAINT domain_deployment_version_check CHECK(version=3)").await.unwrap();
        assert!(DomainStore::open(url,"example.test","_hagency_test_").await.is_err());
        db.batch_execute(include_str!("../processing-reaction-cutover.sql")).await.unwrap();
        let after=sql_query(snapshot).get_result::<Snapshot>(&mut db).await.unwrap();
        assert_eq!(before.id,after.id);assert_eq!(before.digest,after.digest);
        DomainStore::open(url,"example.test","_hagency_test_").await.unwrap();
        TransportStore::open(url,Limits::default()).await.unwrap();
        assert!(db.batch_execute(include_str!("../processing-reaction-cutover.sql")).await.is_err());
        db.batch_execute("ROLLBACK").await.unwrap();
        let after=sql_query(snapshot).get_result::<Snapshot>(&mut db).await.unwrap();
        assert_eq!(before.id,after.id);assert_eq!(before.digest,after.digest);
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_processing_expired_observation_after_lock_wait_does_not_permanently_block() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let grant = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: grant.epoch,
    };
    let dispatch = enqueue(&f, "processing-stale-observation").await;
    running(&f, &lease, &dispatch, "processing-stale-run").await;
    let receipt = f
        .transport
        .mark_processing(
            &f.p,
            &lease,
            &dispatch,
            "processing-stale-run",
            &f.facts,
            now,
        )
        .await
        .unwrap();
    // The worker has already performed its valid independent preflight.
    f.transport
        .observe_delivery_authority(
            None,
            &f.binding,
            &f.facts.requester_mxid,
            Some(&dispatch),
            &f.facts,
            now,
        )
        .await
        .unwrap();
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut blocker = AsyncPgConnection::establish(&url).await.unwrap();
    blocker
        .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
        .await
        .unwrap();
    let t = f.transport.clone();
    let facts = f.facts.clone();
    let id = receipt.id.clone();
    // Inject the lock-wait's elapsed clock without a 31-second real sleep.
    let task =
        tokio::spawn(async move { t.block_processing_observed(&id, &facts, now + 31_000).await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!task.is_finished());
    blocker.batch_execute("COMMIT").await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Unavailable("matrix_state_unavailable"))
    ));
    let mut db = f.transport.db.lock().await;
    let intent=sql_query("SELECT state AS id,delivery_blocked::text AS digest FROM hagency_agent_v1.processing_outbox WHERE id=$1")
        .bind::<Text,_>(&receipt.id).get_result::<Existing>(&mut *db).await.unwrap();
    assert_eq!(intent.id, "pending");
    assert_eq!(intent.digest, "false");
    drop(db);
    // Fresh evidence remains sendable, not trapped by a permanent denial flag.
    assert!(
        f.transport
            .claim_processing(&receipt.id, &f.facts, crate::api::now_ms())
            .await
            .is_ok()
    );
}

fn room_facts(delivery: &DeliveryFacts) -> RoomFacts {
    RoomFacts {
        owner_direct_valid: delivery.owner_direct_valid,
        owner_mxid: delivery.owner_mxid.clone(),
        room_id: delivery.room_id.clone(),
        space_id: delivery.space_id.clone(),
        observed_at_ms: delivery.observed_at_ms,
        owner_in_space: delivery.owner_in_space,
        owner_in_room: delivery.owner_in_room,
        room_in_space: delivery.room_in_space,
        service_can_invite: false,
        puppet_mxid: Some(delivery.puppet_mxid.clone()),
        puppet_in_room: delivery.puppet_in_room,
        encrypted: delivery.encrypted,
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_paused_room_feedback_is_durable_deduplicated_and_never_infers() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let paused = f
        .domain
        .suspend_binding(&f.p, &f.binding, now)
        .await
        .unwrap();
    assert_eq!(paused.state, "suspended");
    assert!(paused.owner_service_paused);
    assert_eq!(
        serde_json::to_value(&paused).unwrap()["ownerServicePaused"],
        true
    );
    let scopes = f.transport.routing_scopes(&f.facts.room_id).await.unwrap();
    assert_eq!(scopes.len(), 1);
    assert!(!scopes[0].active && scopes[0].service_paused);
    assert!(f.transport.binding_scope(&f.binding).await.is_err());
    let e = event(&f, "paused-target", true, None);
    assert!(matches!(
        f.transport
            .ingest_routed(&f.binding, e.clone(), &f.facts, now)
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    let notices = f.transport.pause_notice_candidates("", now).await.unwrap();
    let notice = notices.iter().find(|n| n.binding_id == f.binding).unwrap();
    let original_id = notice.id.clone();
    let original_txn = notice.matrix_txn_id.clone();
    assert_eq!(notice.thread_root, e.event_id);
    assert!(matches!(
        f.transport
            .ingest_routed(&f.binding, e.clone(), &f.facts, now)
            .await
            .unwrap(),
        RouteResult::Duplicate { .. }
    ));
    let mut changed = e.clone();
    changed.body.push('!');
    assert!(matches!(
        f.transport
            .ingest_routed(&f.binding, changed, &f.facts, now)
            .await,
        Err(Error::Conflict("event_payload_changed"))
    ));
    let mut guard = f.transport.db.lock().await;
    let count = sql_query(
        "SELECT count(*) AS total FROM hagency_agent_v1.owner_events WHERE binding_id=$1",
    )
    .bind::<Text, _>(&f.binding)
    .get_result::<Count>(&mut *guard)
    .await
    .unwrap();
    drop(guard);
    assert_eq!(
        count.total, 0,
        "paused requests must not touch inference queue"
    );
    // Ordinary unaddressed messages, bots and edits produce no notice.
    for mut ignored in [
        event(&f, "ordinary", false, None),
        event(&f, "bot", true, None),
        event(&f, "edit", true, None),
    ] {
        if ignored.event_id == "$bot" {
            ignored.sender_mxid = f.facts.puppet_mxid.clone();
        }
        if ignored.event_id == "$edit" {
            ignored.is_edit = true;
        }
        let mut facts = f.facts.clone();
        facts.requester_mxid = ignored.sender_mxid.clone();
        assert!(matches!(
            f.transport
                .ingest_routed(&f.binding, ignored, &facts, now)
                .await
                .unwrap(),
            RouteResult::Ignored
        ));
    }
    // Per-requester throttle records the new event but does not spam the Room.
    f.transport
        .ingest_routed(
            &f.binding,
            event(&f, "throttled", true, None),
            &f.facts,
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .pause_notice_candidates("", now)
            .await
            .unwrap()
            .iter()
            .filter(|n| n.binding_id == f.binding)
            .count(),
        1
    );
    assert!(
        matches!(
            f.transport
                .claim_pause_notice(&original_id, &f.facts, now + 30_001)
                .await,
            Err(Error::Unavailable("matrix_state_unavailable"))
        ),
        "stale observations are transient, not permission denial"
    );
    let claimed = f
        .transport
        .claim_pause_notice(&original_id, &f.facts, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_pause_notice(&original_id, &f.facts, now)
            .await
            .is_err()
    );
    f.transport
        .confirm_pause_notice(
            &original_id,
            claimed.worker_token.as_deref().unwrap(),
            &Err(Error::Unavailable("temporary_network_failure")),
            now,
        )
        .await
        .unwrap();
    // Retry after an uncertain Matrix send retains the identical transaction ID.
    let retry = f
        .transport
        .claim_pause_notice(&original_id, &f.facts, now + 6000)
        .await
        .unwrap();
    assert_eq!(retry.matrix_txn_id, original_txn);
    f.transport
        .confirm_pause_notice(
            &original_id,
            retry.worker_token.as_deref().unwrap(),
            &Ok("$pause-feedback".into()),
            now + 6000,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_pause_notice(&original_id, &f.facts, now + 6000)
            .await
            .is_err()
    );
    let mut live = f.facts.clone();
    live.observed_at_ms = crate::api::now_ms();
    assert!(
        !f.domain
            .resume_binding(&f.p, &f.binding, &room_facts(&live), crate::api::now_ms())
            .await
            .unwrap()
            .owner_service_paused
    );
    assert!(
        matches!(
            f.transport
                .ingest_routed(&f.binding, e, &live, crate::api::now_ms())
                .await
                .unwrap(),
            RouteResult::Duplicate { .. }
        ),
        "resume must not infer an already noticed request"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_pause_feedback_is_fenced_by_membership_permissions_and_resume() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let binding = f.domain.binding(&f.p, &f.binding, now).await.unwrap();
    let mut lost = room_facts(&f.facts);
    lost.owner_in_room = false;
    assert!(
        f.domain
            .retire_membership_loss_trusted(&f.binding, binding.generation, &lost, now)
            .await
            .unwrap()
    );
    assert!(
        f.transport
            .routing_scopes(&f.facts.room_id)
            .await
            .unwrap()
            .is_empty(),
        "membership departure must not advertise a service pause"
    );
    assert!(
        f.domain
            .resume_binding(&f.p, &f.binding, &room_facts(&f.facts), now)
            .await
            .is_err()
    );
    let current = f.domain.binding(&f.p, &f.binding, now).await.unwrap();
    let mut departed = room_facts(&f.facts);
    departed.puppet_in_room = false;
    f.domain
        .confirm_left_trusted(&f.binding, current.generation, &departed, now)
        .await
        .unwrap();
    let rebound = f
        .domain
        .bind_room(
            &f.p,
            &f.agent,
            crate::domain::BindRoom {
                project_id: current.project_id.unwrap(),
                room_id: current.room_id,
                idempotency_key: "pause-feedback-rebind".into(),
            },
            &room_facts(&f.facts),
            now,
        )
        .await
        .unwrap();
    f.domain
        .activate_binding(
            &f.p,
            &f.binding,
            rebound.binding.generation,
            &room_facts(&f.facts),
            now,
        )
        .await
        .unwrap();
    f.domain
        .suspend_binding(&f.p, &f.binding, now)
        .await
        .unwrap();
    let mut denied = f.facts.clone();
    denied.requester_in_room = false;
    assert!(
        f.transport
            .ingest_routed(&f.binding, event(&f, "nonmember", true, None), &denied, now)
            .await
            .is_err()
    );
    f.transport
        .ingest_routed(
            &f.binding,
            event(&f, "pause-authorized", true, None),
            &f.facts,
            now,
        )
        .await
        .unwrap();
    let notices = f.transport.pause_notice_candidates("", now).await.unwrap();
    let n = notices.iter().find(|n| n.binding_id == f.binding).unwrap();
    // A fresh denial permanently cancels this notice; restoring membership alone
    // cannot replay it or expose feedback to a requester that lost access.
    assert!(
        f.transport
            .claim_pause_notice(&n.id, &denied, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_pause_notice(&n.id, &f.facts, now)
            .await
            .is_err()
    );
    // Another member's legitimate request gets its own rate bucket.
    let mut other = f.facts.clone();
    other.requester_mxid = "@another-requester:example.test".into();
    let mut e = event(&f, "cancel-on-resume", true, None);
    e.sender_mxid = other.requester_mxid.clone();
    f.transport
        .ingest_routed(&f.binding, e, &other, now)
        .await
        .unwrap();
    let candidates = f.transport.pause_notice_candidates("", now).await.unwrap();
    let n = candidates
        .iter()
        .find(|n| n.binding_id == f.binding)
        .unwrap();
    f.domain
        .resume_binding(&f.p, &f.binding, &room_facts(&f.facts), now)
        .await
        .unwrap();
    assert!(
        f.transport
            .claim_pause_notice(&n.id, &other, now)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_paused_owner_direct_notices_require_the_exact_owner_and_private_room() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let room = format!("!pause_direct_{}:example.test", secret_token());
    let direct = DeliveryFacts {
        requester_mxid: f.p.mxid.clone(),
        room_id: room.clone(),
        space_id: String::new(),
        owner_direct_valid: true,
        owner_in_space: false,
        room_in_space: false,
        ..f.facts.clone()
    };
    let created = f
        .domain
        .adopt_owner_direct(&f.p, &f.agent, &room, &room_facts(&direct), now)
        .await
        .unwrap();
    f.domain
        .verify_provisioning_trusted(
            &created.binding.id,
            created.binding.generation,
            &room_facts(&direct),
            true,
            now,
        )
        .await
        .unwrap();
    f.domain
        .suspend_binding(&f.p, &created.binding.id, now)
        .await
        .unwrap();
    let e = RoutedEvent {
        sender_mxid: f.p.mxid.clone(),
        room_id: room.clone(),
        mentioned_mxids: BTreeSet::new(),
        ..event(&f, "paused-private", false, None)
    };
    f.transport
        .ingest_routed(&created.binding.id, e.clone(), &direct, now)
        .await
        .unwrap();
    let ns = f.transport.pause_notice_candidates("", now).await.unwrap();
    let n = ns
        .iter()
        .find(|n| n.binding_id == created.binding.id)
        .unwrap();
    assert_eq!(n.thread_root, room, "private notice remains a normal DM");
    let mut outsider = direct.clone();
    outsider.requester_mxid = "@outsider:example.test".into();
    let mut outsiders_event = e.clone();
    outsiders_event.event_id = "$private-outsider".into();
    outsiders_event.sender_mxid = outsider.requester_mxid.clone();
    assert!(
        f.transport
            .ingest_routed(&created.binding.id, outsiders_event, &outsider, now)
            .await
            .is_err()
    );
    let mut public = direct.clone();
    public.owner_direct_valid = false;
    assert!(
        f.transport
            .claim_pause_notice(&n.id, &public, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .claim_pause_notice(&n.id, &direct, now)
            .await
            .is_err(),
        "privacy denial is permanent for this original event"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_thread_auto_reply_is_owner_opt_in_and_binding_scoped() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    enqueue(&f, "reply-policy-root").await;
    assert!(
        !f.domain
            .binding(&f.p, &f.binding, now)
            .await
            .unwrap()
            .thread_auto_reply
    );
    let route = |id, root| event(&f, id, false, root);
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                route("default-followup", Some("$reply-policy-root")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    // Another fixture has a different owner and Room binding.
    let other = fixture().await;
    assert!(
        f.domain
            .set_thread_auto_reply(&other.p, &f.binding, true, now)
            .await
            .is_err()
    );
    f.domain
        .set_thread_auto_reply(&f.p, &f.binding, true, now)
        .await
        .unwrap();
    assert!(
        f.domain
            .binding(&f.p, &f.binding, now)
            .await
            .unwrap()
            .thread_auto_reply
    );
    let reopened = DomainStore::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        "example.test",
        "_hagency_test_",
    )
    .await
    .unwrap();
    assert!(
        reopened
            .binding(&f.p, &f.binding, now)
            .await
            .unwrap()
            .thread_auto_reply
    );

    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                route("enabled-followup", Some("$reply-policy-root")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Queued { .. }
    ));
    let mut member_event = event(&f, "member-followup", false, Some("$reply-policy-root"));
    let mut member_facts = f.facts.clone();
    member_event.sender_mxid = "@another_member:example.test".into();
    member_facts.requester_mxid = member_event.sender_mxid.clone();
    assert!(matches!(
        f.transport
            .ingest_routed(&f.binding, member_event, &member_facts, now)
            .await
            .unwrap(),
        RouteResult::Queued { .. }
    ));
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                route("unknown-thread", Some("$other-root")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                route("ordinary-no-mention", None),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    assert!(matches!(
        other
            .transport
            .ingest_routed(
                &other.binding,
                event(&other, "isolated", false, Some("$reply-policy-root")),
                &other.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    f.domain
        .set_thread_auto_reply(&f.p, &f.binding, false, now)
        .await
        .unwrap();
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                route("disabled-followup", Some("$reply-policy-root")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Ignored
    ));
    assert!(matches!(
        f.transport
            .ingest_routed(
                &f.binding,
                event(&f, "mentioned-followup", true, Some("$reply-policy-root")),
                &f.facts,
                now
            )
            .await
            .unwrap(),
        RouteResult::Queued { .. }
    ));
}
