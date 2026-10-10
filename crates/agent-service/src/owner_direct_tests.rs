use super::*;
use crate::gateway::Gateway;
use serde_json::json;
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_owner_direct_is_private_independent_and_executes_only_owner_with_assigned_device()
{
    let f = fixture().await;
    let now = crate::api::now_ms();
    let room = format!("!direct_{}:example.test", crate::secret_token());
    let states = std::sync::Arc::new(std::sync::RwLock::new(vec![
        json!({"type":"m.room.create","state_key":"","content":{}}),
        json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
        json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
        json!({"type":"m.room.member","state_key":f.p.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":f.facts.puppet_mxid,"content":{"membership":"invite"}}),
    ]));
    let state = states.clone();
    let expected = room.clone();
    let gateway = Gateway::new(
        std::sync::Arc::new(move |room| {
            let state = state.clone();
            let expected = expected.clone();
            Box::pin(async move {
                assert_eq!(room, expected, "direct never reads a synthetic Space");
                Ok(state.read().unwrap().clone())
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    states
        .write()
        .unwrap()
        .iter_mut()
        .find(|e| e["type"] == "m.room.history_visibility")
        .unwrap()["content"]["history_visibility"] = json!("world_readable");
    let public_facts = gateway
        .room(&room, "", &f.p.mxid, Some(&f.facts.puppet_mxid))
        .await
        .unwrap();
    assert!(!public_facts.owner_direct_valid);
    assert!(
        f.domain
            .adopt_owner_direct(&f.p, &f.agent, &room, &public_facts, now)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .agent(&f.p, &f.agent, now)
            .await
            .unwrap()
            .owner_direct_room_id
            .is_none()
    );
    states
        .write()
        .unwrap()
        .iter_mut()
        .find(|e| e["type"] == "m.room.history_visibility")
        .unwrap()["content"]["history_visibility"] = json!("joined");
    states.write().unwrap().push(json!({"type":"m.room.member","state_key":"@third:example.test","content":{"membership":"invite"}}));
    let third_facts = gateway
        .room(&room, "", &f.p.mxid, Some(&f.facts.puppet_mxid))
        .await
        .unwrap();
    assert!(!third_facts.owner_direct_valid);
    assert!(
        f.domain
            .adopt_owner_direct(&f.p, &f.agent, &room, &third_facts, now)
            .await
            .is_err()
    );
    states
        .write()
        .unwrap()
        .retain(|e| e["state_key"] != "@third:example.test");
    let facts = gateway
        .room(&room, "", &f.p.mxid, Some(&f.facts.puppet_mxid))
        .await
        .unwrap();
    assert!(facts.owner_direct_valid);
    assert!(!facts.puppet_in_room);
    assert!(!facts.service_can_invite);
    let created = f
        .domain
        .adopt_owner_direct(&f.p, &f.agent, &room, &facts, now)
        .await
        .unwrap();
    assert_eq!(created.binding.scope_kind, "owner_direct");
    assert!(created.binding.project_id.is_none());
    assert_eq!(
        created.agent.owner_direct_room_id.as_deref(),
        Some(room.as_str())
    );
    f.domain
        .verify_provisioning_trusted(
            &created.binding.id,
            created.binding.generation,
            &facts,
            false,
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .binding(&f.p, &created.binding.id, now)
            .await
            .unwrap()
            .state,
        "joining"
    );
    states
        .write()
        .unwrap()
        .iter_mut()
        .find(|e| e["state_key"] == f.facts.puppet_mxid)
        .unwrap()["content"]["membership"] = json!("join");
    let joined = gateway
        .room(&room, "", &f.p.mxid, Some(&f.facts.puppet_mxid))
        .await
        .unwrap();
    f.domain
        .verify_provisioning_trusted(
            &created.binding.id,
            created.binding.generation,
            &joined,
            true,
            now,
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .projects(&f.p, now)
            .await
            .unwrap()
            .iter()
            .all(|p| p.space_id != room),
        "owner-direct must not create a synthetic Project"
    );
    // A real Room may later be linked beneath a Space, but a permanent
    // owner-direct binding must never be silently returned as Project scope.
    let project = f
        .domain
        .projects(&f.p, now)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.space_id == f.facts.space_id)
        .unwrap();
    let space_admin = AdminFacts {
        actor_mxid: f.p.mxid.clone(),
        room_id: project.space_id.clone(),
        observed_at_ms: crate::api::now_ms(),
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let room_admin = AdminFacts {
        room_id: room.clone(),
        is_space: false,
        linked_space_id: Some(project.space_id.clone()),
        ..space_admin.clone()
    };
    f.domain
        .register_room(&f.p, &project.id, &room, &space_admin, &room_admin, now)
        .await
        .unwrap();
    let project_facts = RoomFacts {
        space_id: project.space_id.clone(),
        owner_in_space: true,
        room_in_space: true,
        service_can_invite: true,
        owner_direct_valid: false,
        ..joined.clone()
    };
    assert!(matches!(
        f.domain
            .bind_room(
                &f.p,
                &f.agent,
                crate::domain::BindRoom {
                    project_id: project.id,
                    room_id: room.clone(),
                    idempotency_key: "direct-must-not-morph".into()
                },
                &project_facts,
                now
            )
            .await,
        Err(Error::Conflict("room_binding_scope_conflict"))
    ));
    let delivery = gateway
        .delivery(&room, "", &f.p.mxid, &f.p.mxid, &f.facts.puppet_mxid)
        .await
        .unwrap();
    let direct_event = |id: &str| RoutedEvent {
        event_id: format!("${id}"),
        room_id: room.clone(),
        sender_mxid: f.p.mxid.clone(),
        body: "A direct owner request without a mention".into(),
        mentioned_mxids: Default::default(),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    };
    assert!(
        f.domain
            .set_thread_auto_reply(&f.p, &created.binding.id, true, now)
            .await
            .is_err(),
        "Room reply policy is not applicable to private chat"
    );
    let queued = f
        .transport
        .ingest_routed(
            &created.binding.id,
            direct_event("owner-direct"),
            &delivery,
            now,
        )
        .await
        .unwrap();
    let RouteResult::Queued { dispatch_id: id } = queued else {
        panic!("owner direct request did not queue")
    };
    let acquired = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: acquired.epoch,
    };
    let first = f
        .transport
        .claim_event(&f.p, &lease, &id, &delivery, now)
        .await
        .unwrap();
    assert_eq!(
        first.thread_root, room,
        "top-level owner DM context is Room scoped"
    );
    let RouteResult::Queued {
        dispatch_id: second_id,
    } = f
        .transport
        .ingest_routed(
            &created.binding.id,
            direct_event("owner-direct-second"),
            &delivery,
            now,
        )
        .await
        .unwrap()
    else {
        panic!("second DM was not queued")
    };
    let second = f
        .transport
        .claim_event(&f.p, &lease, &second_id, &delivery, now)
        .await
        .unwrap();
    assert_eq!(second.thread_root, first.thread_root);
    let mut explicit = direct_event("owner-direct-thread");
    explicit.thread_root = Some("$explicit-thread".into());
    let RouteResult::Queued {
        dispatch_id: thread_id,
    } = f
        .transport
        .ingest_routed(&created.binding.id, explicit, &delivery, now)
        .await
        .unwrap()
    else {
        panic!("explicit DM thread was not queued")
    };
    let threaded = f
        .transport
        .claim_event(&f.p, &lease, &thread_id, &delivery, now)
        .await
        .unwrap();
    assert_eq!(threaded.thread_root, "$explicit-thread");
    f.transport
        .acknowledge(&f.p, &lease, &id, now)
        .await
        .unwrap();
    f.transport
        .start_execution(&f.p, &lease, &id, "direct-execution", &delivery, now)
        .await
        .unwrap();
    states
        .write()
        .unwrap()
        .iter_mut()
        .find(|e| e["type"] == "m.room.history_visibility")
        .unwrap()["content"]["history_visibility"] = json!("world_readable");
    let unsafe_delivery = gateway
        .delivery(&room, "", &f.p.mxid, &f.p.mxid, &f.facts.puppet_mxid)
        .await
        .unwrap();
    assert!(!unsafe_delivery.owner_direct_valid);
    assert!(
        f.transport
            .submit_reply(
                &f.p,
                &lease,
                SubmitReply {
                    dispatch_id: id.clone(),
                    execution_id: "direct-execution".into(),
                    body: "Old reply".into()
                },
                &unsafe_delivery,
                now
            )
            .await
            .is_err()
    );
    let fenced = f
        .domain
        .binding(&f.p, &created.binding.id, now)
        .await
        .unwrap();
    assert_eq!(fenced.state, "leaving");
    assert!(fenced.generation > created.binding.generation);
    states
        .write()
        .unwrap()
        .iter_mut()
        .find(|e| e["type"] == "m.room.history_visibility")
        .unwrap()["content"]["history_visibility"] = json!("joined");
    let safe = gateway
        .room(&room, "", &f.p.mxid, Some(&f.facts.puppet_mxid))
        .await
        .unwrap();
    assert!(
        f.domain
            .resume_binding(&f.p, &created.binding.id, &safe, now)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .adopt_owner_direct(&f.p, &f.agent, &room, &safe, now)
            .await
            .is_err()
    );
    let mut departed = safe.clone();
    departed.puppet_in_room = false;
    f.domain
        .confirm_left_trusted(&created.binding.id, fenced.generation, &departed, now)
        .await
        .unwrap();
    let rebound = f
        .domain
        .adopt_owner_direct(&f.p, &f.agent, &room, &safe, now)
        .await
        .unwrap();
    assert_eq!(rebound.binding.state, "joining");
    assert_eq!(rebound.binding.generation, fenced.generation + 1);
    f.domain
        .verify_provisioning_trusted(
            &created.binding.id,
            rebound.binding.generation,
            &safe,
            true,
            now,
        )
        .await
        .unwrap();
    let delivery = gateway
        .delivery(&room, "", &f.p.mxid, &f.p.mxid, &f.facts.puppet_mxid)
        .await
        .unwrap();
    assert!(
        f.transport
            .start_execution(&f.p, &lease, &id, "direct-execution", &delivery, now)
            .await
            .is_err()
    );
    let RouteResult::Queued { dispatch_id: next } = f
        .transport
        .ingest_routed(
            &created.binding.id,
            direct_event("owner-direct-fresh"),
            &delivery,
            now,
        )
        .await
        .unwrap()
    else {
        panic!("fresh direct request did not queue")
    };
    f.transport
        .claim_event(&f.p, &lease, &next, &delivery, now)
        .await
        .unwrap();
    f.transport
        .acknowledge(&f.p, &lease, &next, now)
        .await
        .unwrap();
    f.transport
        .start_execution(&f.p, &lease, &next, "direct-new-execution", &delivery, now)
        .await
        .unwrap();
    let reply = f
        .transport
        .submit_reply(
            &f.p,
            &lease,
            SubmitReply {
                dispatch_id: next.clone(),
                execution_id: "direct-new-execution".into(),
                body: "Fresh direct reply".into(),
            },
            &delivery,
            now,
        )
        .await
        .unwrap();
    let intent = f
        .transport
        .claim_reply(&reply.id, &delivery, now)
        .await
        .unwrap();
    assert_eq!(intent.room_id, room);
    assert_eq!(intent.thread_root, room);
    assert_eq!(intent.puppet_mxid, f.facts.puppet_mxid);
    let uncertain = f
        .transport
        .confirm_reply(
            &intent.id,
            intent.worker_token.as_deref().unwrap(),
            None,
            now,
        )
        .await
        .unwrap();
    assert_eq!(uncertain.state, "unknown");
    let reconciled = f
        .transport
        .reconcile_known_reply(
            &f.p,
            &lease,
            SubmitReply {
                dispatch_id: next,
                execution_id: "direct-new-execution".into(),
                body: "Fresh direct reply".into(),
            },
            &delivery,
            now,
        )
        .await
        .unwrap();
    assert_eq!(reconciled.matrix_txn_id, intent.matrix_txn_id);
    assert_eq!(reconciled.thread_root, room);
    assert_eq!(reconciled.body, intent.body);
    // A trusted observation of a flat send uses the same logical Room context.
    let sent = f
        .transport
        .reconcile_reply_sent(
            &intent.id,
            &crate::transport::ObservedReply {
                event_id: "$direct-sent".into(),
                matrix_txn_id: intent.matrix_txn_id.clone(),
                sender_mxid: intent.puppet_mxid.clone(),
                room_id: room.clone(),
                thread_root: room.clone(),
                body: intent.body.clone(),
                observed_at_ms: now,
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(sent.state, "sent");
    f.transport
        .confirm_reply(
            &intent.id,
            intent.worker_token.as_deref().unwrap(),
            Some("$direct-sent"),
            now,
        )
        .await
        .unwrap();
    let mut outsider = direct_event("outsider-direct");
    outsider.sender_mxid = "@outsider:example.test".into();
    let outsider_facts = gateway
        .delivery(
            &room,
            "",
            &f.p.mxid,
            &outsider.sender_mxid,
            &f.facts.puppet_mxid,
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .ingest_routed(&created.binding.id, outsider, &outsider_facts, now)
            .await
            .is_err()
    );
}
