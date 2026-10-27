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
