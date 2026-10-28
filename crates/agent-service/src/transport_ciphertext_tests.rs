use super::*;
use crate::appservice::Inbox;
use serde_json::{Value, json};
fn raw(f: &Fixture, event: &str) -> Value {
    json!({"type":"m.room.encrypted","event_id":format!("${event}"),"room_id":f.facts.room_id,"sender":f.facts.requester_mxid,"origin_server_ts":123,"content":{"algorithm":"m.megolm.v1.aes-sha2","ciphertext":"c2VhbGVk","session_id":"session"}})
}
async fn inbox() -> Inbox {
    Inbox::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        "h".repeat(64),
    )
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_ciphertext_retains_original_as_custody_after_compaction_and_never_enters_plain_poll()
 {
    let mut f = fixture().await;
    f.facts.encrypted = true;
    let now = crate::api::now_ms();
    let inbox = inbox().await;
    let event = raw(&f, "encrypted-original");
    let tx = secret_token();
    assert!(
        f.transport
            .ingest_ciphertext(&tx, &f.binding, &event, &f.facts, now)
            .await
            .is_err()
    );
    inbox
        .accept(&tx, json!({"events":[event]}), now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &event, &f.facts, now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &event, &f.facts, now)
        .await
        .unwrap();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let headers = f
        .transport
        .ciphertext_candidates(&f.p, &reference, &f.binding, 4, now)
        .await
        .unwrap();
    assert_eq!(headers.len(), 1);
    let id = &headers[0].id;
    let read = f
        .transport
        .read_ciphertext(&f.p, &reference, id, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(read.event, event);
    assert_eq!(read.received_at_ms, now);
    assert!(
        f.transport
            .event_candidates_with_attachments(&f.p, &reference, &f.binding, 10, now)
            .await
            .unwrap()
            .is_empty()
    );
    let mut puppet = event.clone();
    puppet["event_id"] = json!("$agent-reply");
    puppet["sender"] = json!(&f.facts.puppet_mxid);
    let puppet_tx = secret_token();
    inbox
        .accept(&puppet_tx, json!({"events":[puppet]}), now)
        .await
        .unwrap();
    let mut puppet_facts = f.facts.clone();
    puppet_facts.requester_mxid = f.facts.puppet_mxid.clone();
    f.transport
        .ingest_ciphertext(&puppet_tx, &f.binding, &puppet, &puppet_facts, now)
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .ciphertext_candidates(&f.p, &reference, &f.binding, 4, now)
            .await
            .unwrap()
            .len(),
        1
    );
    inbox.routed(&tx).await.unwrap();
    let reopened = TransportStore::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .read_ciphertext(&f.p, &reference, id, &f.facts, now)
            .await
            .unwrap()
            .source_digest,
        read.source_digest
    );
    let mut denied = f.facts.clone();
    denied.requester_in_room = false;
    assert!(
        reopened
            .read_ciphertext(&f.p, &reference, id, &denied, now)
            .await
            .is_err()
    );
    denied = f.facts.clone();
    denied.encrypted = false;
    assert!(
        reopened
            .read_ciphertext(&f.p, &reference, id, &denied, now)
            .await
            .is_err()
    );
    assert!(
        reopened
            .read_ciphertext(&f.second, &reference, id, &f.facts, now)
            .await
            .is_err()
    );
    let tx2 = secret_token();
    let mut changed = event.clone();
    changed["content"]["ciphertext"] = json!("changed");
    inbox
        .accept(&tx2, json!({"events":[changed]}), now)
        .await
        .unwrap();
    assert!(
        reopened
            .ingest_ciphertext(&tx2, &f.binding, &changed, &f.facts, now)
            .await
            .is_err()
    );
    let original = sql_query(
        "SELECT count(*) AS total FROM hagency_agent_v1.owner_events WHERE binding_id=$1",
    )
    .bind::<Text, _>(&f.binding)
    .get_result::<Count>(&mut *f.transport.db.lock().await.unwrap())
    .await
    .unwrap();
    assert_eq!(original.total, 0);
    f.auth
        .revoke_device(&f.credential, f.p.device_id.as_ref().unwrap(), now)
        .await
        .unwrap();
    assert!(
        reopened
            .read_ciphertext(&f.p, &reference, id, &f.facts, now)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_ciphertext_capacity_expiry_and_tombstone_cannot_rearm_or_mutate_source() {
    let mut f = fixture().await;
    f.facts.encrypted = true;
    f.transport.limits.max_owner_events = 1;
    let now = crate::api::now_ms();
    let inbox = inbox().await;
    let tx = secret_token();
    let first = raw(&f, "one");
    let second = raw(&f, "two");
    inbox
        .accept(&tx, json!({"events":[first,second]}), now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &first, &f.facts, now)
        .await
        .unwrap();
    assert!(matches!(
        f.transport
            .ingest_ciphertext(&tx, &f.binding, &second, &f.facts, now)
            .await,
        Err(Error::Unavailable("owner_queue_full"))
    ));
    assert!(inbox.pending().await.unwrap().iter().any(|t| t.id == tx));
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = f
        .transport
        .ciphertext_candidates(&f.p, &reference, &f.binding, 4, now)
        .await
        .unwrap()
        .remove(0)
        .id;
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.ciphertext_inbox SET source_digest='changed' WHERE id=$1"
        )
        .bind::<Text, _>(&id)
        .execute(&mut *f.transport.db.lock().await.unwrap())
        .await
        .is_err()
    );
    sql_query(
        "UPDATE hagency_agent_v1.ciphertext_inbox SET state='expired',event=NULL WHERE id=$1",
    )
    .bind::<Text, _>(&id)
    .execute(&mut *f.transport.db.lock().await.unwrap())
    .await
    .unwrap();
    assert!(
        f.transport
            .read_ciphertext(&f.p, &reference, &id, &f.facts, now)
            .await
            .is_err()
    );
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &first, &f.facts, now)
        .await
        .unwrap();
    assert!(
        f.transport
            .ciphertext_candidates(&f.p, &reference, &f.binding, 4, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.ciphertext_inbox SET state='pending',event=$2 WHERE id=$1"
        )
        .bind::<Text, _>(&id)
        .bind::<diesel::sql_types::Jsonb, _>(&first)
        .execute(&mut *f.transport.db.lock().await.unwrap())
        .await
        .is_err()
    );
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.ciphertext_inbox WHERE id=$1")
            .bind::<Text, _>(&id)
            .execute(&mut *f.transport.db.lock().await.unwrap())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_encrypted_prepare_ack_are_original_idempotent_and_plain_execution_stays_closed() {
    let mut f = fixture().await;
    f.facts.encrypted = true;
    let now = crate::api::now_ms();
    let inbox = inbox().await;
    let event = raw(&f, "prepare-ack");
    let tx = secret_token();
    inbox
        .accept(&tx, json!({"events":[event]}), now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &event, &f.facts, now)
        .await
        .unwrap();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let headers = f
        .transport
        .ciphertext_candidates(&f.p, &lease, &f.binding, 4, now)
        .await
        .unwrap();
    let c = f
        .transport
        .read_ciphertext(&f.p, &lease, &headers[0].id, &f.facts, now)
        .await
        .unwrap();
    let projection = encrypted::Projection {
        thread_root: Some("$original-root".into()),
        mentioned_mxids: [f.facts.puppet_mxid.clone()].into(),
    };
    let b = encrypted::Prepare {
        lease: lease.clone(),
        inbox_id: c.id.clone(),
        source_digest: c.source_digest.clone(),
        projection: projection.clone(),
    };
    let prepared = f
        .transport
        .prepare_ciphertext(&f.p, &b, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(prepared.dispatch.state, "encrypted_offered");
    assert_eq!(
        serde_json::from_str::<Value>(&prepared.dispatch.body).unwrap(),
        event
    );
    assert_eq!(
        f.transport
            .prepare_ciphertext(&f.p, &b, &f.facts, now)
            .await
            .unwrap()
            .dispatch
            .id,
        prepared.dispatch.id
    );
    assert!(
        f.transport
            .event_candidates_with_attachments(&f.p, &lease, &f.binding, 10, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        f.transport
            .claim_event(&f.p, &lease, &prepared.dispatch.id, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .acknowledge(&f.p, &lease, &prepared.dispatch.id, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .start_execution(
                &f.p,
                &lease,
                &prepared.dispatch.id,
                "exec_plain",
                &f.facts,
                now
            )
            .await
            .is_err()
    );
    let mut changed = b;
    changed.projection.thread_root = Some("$changed".into());
    assert!(
        f.transport
            .prepare_ciphertext(&f.p, &changed, &f.facts, now)
            .await
            .is_err()
    );
    let reference = encrypted::Reference {
        lease: lease.clone(),
        inbox_id: c.id.clone(),
        source_digest: c.source_digest.clone(),
    };
    assert!(
        f.transport
            .ciphertext_admission(&f.second, &reference, &f.facts, true, now)
            .await
            .is_err()
    );
    let mut denied = f.facts.clone();
    denied.requester_in_room = false;
    assert!(
        f.transport
            .ciphertext_admission(&f.p, &reference, &denied, true, now)
            .await
            .is_err()
    );
    for _ in 0..2 {
        let ack = f
            .transport
            .ciphertext_admission(&f.p, &reference, &f.facts, true, now)
            .await
            .unwrap();
        assert_eq!(ack.dispatch.state, "encrypted_acknowledged");
        assert_eq!(ack.dispatch.id, prepared.dispatch.id);
    }
    assert!(
        f.transport
            .ciphertext_candidates(&f.p, &lease, &f.binding, 4, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        sql_query("UPDATE hagency_agent_v1.owner_events SET state='acknowledged' WHERE id=$1")
            .bind::<Text, _>(&prepared.dispatch.id)
            .execute(&mut *f.transport.db.lock().await.unwrap())
            .await
            .is_err()
    );
    assert!(sql_query("UPDATE hagency_agent_v1.encrypted_dispatches SET source_digest='changed' WHERE dispatch_id=$1").bind::<Text,_>(&prepared.dispatch.id).execute(&mut *f.transport.db.lock().await.unwrap()).await.is_err());
    let mut replay = event.clone();
    replay["event_id"] = json!("$replay-header");
    replay["origin_server_ts"] = json!(124);
    replay["content"]["ciphertext"] = json!("c2VhbGVk=");
    let replay_tx = secret_token();
    inbox
        .accept(&replay_tx, json!({"events":[replay]}), now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&replay_tx, &f.binding, &replay, &f.facts, now)
        .await
        .unwrap();
    let rows = f
        .transport
        .ciphertext_candidates(&f.p, &lease, &f.binding, 4, now)
        .await
        .unwrap();
    let replay = f
        .transport
        .read_ciphertext(&f.p, &lease, &rows[0].id, &f.facts, now)
        .await
        .unwrap();
    let replay = encrypted::Prepare {
        lease: lease.clone(),
        inbox_id: replay.id,
        source_digest: replay.source_digest,
        projection: projection.clone(),
    };
    assert!(matches!(
        f.transport
            .prepare_ciphertext(&f.p, &replay, &f.facts, now)
            .await,
        Err(Error::Conflict("encrypted_ciphertext_replayed"))
    ));
    f.transport.release_lease(&f.p, &lease, now).await.unwrap();
    let next = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    assert!(next.epoch > lease.epoch);
    let b = encrypted::Prepare {
        lease: LeaseRef {
            agent_id: f.agent.clone(),
            epoch: next.epoch,
        },
        inbox_id: c.id,
        source_digest: c.source_digest,
        projection,
    };
    let again = f
        .transport
        .prepare_ciphertext(&f.p, &b, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(again.dispatch.id, prepared.dispatch.id);
    assert_eq!(again.dispatch.state, "encrypted_offered");
    assert_eq!(again.dispatch.dispatch_epoch, Some(next.epoch));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_encrypted_execution_and_original_publication_are_fenced_and_never_rearmed() {
    use super::encrypted_execution::{Action, validate_payload};
    let mut f = fixture().await;
    let now = crate::api::now_ms();
    f.facts.room_id = format!("!encrypted_direct_{}:example.test", secret_token());
    f.facts.space_id.clear();
    f.facts.encrypted = true;
    f.facts.owner_direct_valid = true;
    f.facts.requester_mxid = f.p.mxid.clone();
    let room_facts = super::room_facts(&f.facts);
    let bound = f
        .domain
        .adopt_owner_direct(&f.p, &f.agent, &f.facts.room_id, &room_facts, now)
        .await
        .unwrap();
    f.domain
        .activate_binding(
            &f.p,
            &bound.binding.id,
            bound.binding.generation,
            &room_facts,
            now,
        )
        .await
        .unwrap();
    f.binding = bound.binding.id;
    let inbox = inbox().await;
    let event = raw(&f, "encrypted-execution");
    let tx = secret_token();
    inbox
        .accept(&tx, json!({"events":[event]}), now)
        .await
        .unwrap();
    f.transport
        .ingest_ciphertext(&tx, &f.binding, &event, &f.facts, now)
        .await
        .unwrap();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let headers = f
        .transport
        .ciphertext_candidates(&f.p, &lease, &f.binding, 1, now)
        .await
        .unwrap();
    let source = f
        .transport
        .read_ciphertext(&f.p, &lease, &headers[0].id, &f.facts, now)
        .await
        .unwrap();
    let prepared = f
        .transport
        .prepare_ciphertext(
            &f.p,
            &encrypted::Prepare {
                lease: lease.clone(),
                inbox_id: source.id.clone(),
                source_digest: source.source_digest.clone(),
                projection: encrypted::Projection {
                    thread_root: None,
                    mentioned_mxids: BTreeSet::new(),
                },
            },
            &f.facts,
            now,
        )
        .await
        .unwrap();
    f.transport
        .ciphertext_admission(
            &f.p,
            &encrypted::Reference {
                lease: lease.clone(),
                inbox_id: source.id,
                source_digest: source.source_digest,
            },
            &f.facts,
            true,
            now,
        )
        .await
        .unwrap();
    let mut b = Action {
        lease: lease.clone(),
        dispatch_id: prepared.dispatch.id.clone(),
        execution_id: "exec_original_encrypted".into(),
        action: "start".into(),
        kind: None,
        request_id: None,
        payload: None,
    };
    let started = f
        .transport
        .encrypted_action(&f.p, &b, &f.facts, now)
        .await
        .unwrap();
    assert_eq!(started.0["newlyStarted"], true);
    assert_eq!(
        f.transport
            .encrypted_action(&f.p, &b, &f.facts, now)
            .await
            .unwrap()
            .0["newlyStarted"],
        false
    );
    assert!(
        f.transport
            .start_execution(&f.p, &lease, &b.dispatch_id, &b.execution_id, &f.facts, now)
            .await
            .is_err()
    );
    b.action = "authorize".into();
    assert!(
        f.transport
            .encrypted_action(&f.p, &b, &f.facts, now)
            .await
            .is_ok()
    );
    let mut denied = f.facts.clone();
    denied.requester_in_room = false;
    assert!(
        f.transport
            .encrypted_action(&f.p, &b, &denied, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .encrypted_action(&f.second, &b, &f.facts, now)
            .await
            .is_err()
    );
    let payload = json!({"content":{"algorithm":"m.megolm.v1.aes-sha2","ciphertext":"sealed","session_id":"original"},"shares":[]});
    assert!(
        validate_payload(
            &json!({"content":{"body":"plaintext"},"shares":[]}),
            &f.p.mxid
        )
        .is_err()
    );
    assert!(validate_payload(&json!({"content":payload["content"],"shares":[{"eventType":"m.room.encrypted","txnId":"original","messages":{"@foreign:example.test":{}}}]}),&f.p.mxid).is_err());
    b.action = "publish".into();
    b.kind = Some("reply".into());
    b.request_id = Some("reply".into());
    // Invalid envelopes must not irreversibly reserve this original request.
    // A valid SDK publication must still obtain the first send custody below.
    for session in [
        None,
        Some(json!(null)),
        Some(json!(42)),
        Some(json!("")),
        Some(json!("invalid\nvalue")),
    ] {
        let mut invalid = payload.clone();
        if let Some(session) = session {
            invalid["content"]["session_id"] = session;
        } else {
            invalid["content"]
                .as_object_mut()
                .unwrap()
                .remove("session_id");
        }
        b.payload = Some(invalid);
        assert!(matches!(
            f.transport.encrypted_action(&f.p, &b, &f.facts, now).await,
            Err(Error::Invalid("invalid_encrypted_publication"))
        ));
        let status = f
            .transport
            .encrypted_action(&f.p, &b.status(), &f.facts, now)
            .await
            .unwrap()
            .0;
        assert!(status["publication"].is_null());
        assert_eq!(
            TransportStore::dispatch(
                &mut f.transport.db.lock().await.unwrap(),
                &f.p,
                &b.dispatch_id
            )
            .await
            .unwrap()
            .state,
            "encrypted_running"
        );
    }
    b.payload = Some(payload.clone());
    let queued = f
        .transport
        .encrypted_action(&f.p, &b, &f.facts, now)
        .await
        .unwrap();
    assert!(queued.1);
    assert_eq!(queued.0["publication"]["state"], "possible");
    let id = queued.0["publication"]["id"].as_str().unwrap();
    assert!(
        !f.transport
            .encrypted_action(&f.p, &b, &f.facts, now)
            .await
            .unwrap()
            .1
    );
    b.payload.as_mut().unwrap()["content"]["ciphertext"] = json!("changed");
    assert!(
        f.transport
            .encrypted_action(&f.p, &b, &f.facts, now)
            .await
            .is_err()
    );
    b.payload = Some(payload);
    f.transport.release_lease(&f.p, &lease, now).await.unwrap();
    assert!(
        f.transport
            .encrypted_action(&f.p, &b.clone_for_check(), &f.facts, now)
            .await
            .is_err()
    );
    f.transport
        .encrypted_sent(id, "$actual-original-ciphertext-receipt")
        .await
        .unwrap();
    let status = f
        .transport
        .encrypted_action(&f.p, &b.status(), &f.facts, now)
        .await
        .unwrap()
        .0;
    assert_eq!(status["publication"]["state"], "sent");
    assert_eq!(
        status["publication"]["matrixEventId"],
        "$actual-original-ciphertext-receipt"
    );
    assert_eq!(
        TransportStore::dispatch(
            &mut f.transport.db.lock().await.unwrap(),
            &f.p,
            &b.dispatch_id
        )
        .await
        .unwrap()
        .state,
        "encrypted_completed"
    );
}
