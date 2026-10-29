use super::*;
use crate::file_delivery::{Delivery, Manifest, Target};
use diesel::sql_types::{Binary, Json};

async fn database() -> (String, String, AsyncPgConnection) {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut admin = AsyncPgConnection::establish(&source).await.unwrap();
    let name = format!("hagency_scaling_{}", &secret_token()[..12]);
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = url::Url::parse(&source).unwrap();
    url.set_path(&name);
    (name, url.into(), admin)
}
async fn setup(f: &Fixture, bytes: &[u8]) -> (LeaseRef, Target, Manifest, Delivery) {
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, crate::api::now_ms())
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: lease.agent_id,
        epoch: lease.epoch,
    };
    let id = enqueue(f, "scaling-file").await;
    running(f, &lease, &id, "scaling-execution").await;
    use sha2::{Digest, Sha256};
    let manifest = Manifest {
        filename: "original.bin".into(),
        caption: None,
        bytes: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
    };
    let delivery = f
        .transport
        .prepare_file(
            &f.p,
            &lease,
            &id,
            "scaling-execution",
            "original",
            manifest.clone(),
            &f.facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let target = Target {
        dispatch_id: id,
        execution_id: "scaling-execution".into(),
        file_id: delivery.id.clone(),
    };
    (lease, target, manifest, delivery)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_terminal_file_history_does_not_consume_active_capacity_or_rearm() {
    let (name, url, mut admin) = database().await;
    {
        let f = fixture_at(&url).await;
        let (lease, target, manifest, original) = setup(&f, b"").await;
        let mut db = AsyncPgConnection::establish(&url).await.unwrap();
        let clone_rows = "INSERT INTO hagency_agent_v1.file_outbox SELECT (jsonb_populate_record(NULL::hagency_agent_v1.file_outbox,to_jsonb(o)||jsonb_build_object('id',o.id||'_'||i::text,'request_id',o.request_id||'_'||i::text))).* FROM hagency_agent_v1.file_outbox o CROSS JOIN generate_series(1,4095) i WHERE o.id=$1";
        sql_query(clone_rows)
            .bind::<Text, _>(&original.id)
            .execute(&mut db)
            .await
            .unwrap();
        sql_query(
            "UPDATE hagency_agent_v1.file_outbox SET state='cancelled' WHERE owner_user_id=$1",
        )
        .bind::<Text, _>(&f.p.user_id)
        .execute(&mut db)
        .await
        .unwrap();
        let next = f
            .transport
            .prepare_file(
                &f.p,
                &lease,
                &target.dispatch_id,
                &target.execution_id,
                "next",
                manifest.clone(),
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        assert_ne!(next.id, original.id);
        sql_query(clone_rows)
            .bind::<Text, _>(&next.id)
            .execute(&mut db)
            .await
            .unwrap();
        assert!(matches!(
            f.transport
                .prepare_file(
                    &f.p,
                    &lease,
                    &target.dispatch_id,
                    &target.execution_id,
                    "over-capacity",
                    manifest.clone(),
                    &f.facts,
                    crate::api::now_ms()
                )
                .await,
            Err(Error::Conflict("file_capacity_exceeded"))
        ));
        let replay = f
            .transport
            .prepare_file(
                &f.p,
                &lease,
                &target.dispatch_id,
                &target.execution_id,
                "original",
                manifest,
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        assert_eq!(replay.id, original.id);
        assert_eq!(replay.state, "cancelled");
        assert!(
            sql_query("DELETE FROM hagency_agent_v1.file_outbox WHERE id=$1")
                .bind::<Text, _>(&original.id)
                .execute(&mut db)
                .await
                .is_err()
        );
    }
    admin
        .batch_execute(&format!("DROP DATABASE {name}"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_file_chunks_are_append_only_and_unknown_retains_original_bytes() {
    let (name, url, mut admin) = database().await;
    {
        let f = fixture_at(&url).await;
        let bytes: Vec<u8> = (0..(crate::file_delivery::CHUNK_BYTES * 2 + 7))
            .map(|n| (n % 251) as u8)
            .collect();
        let (lease, target, _, _) = setup(&f, &bytes).await;
        for (i, chunk) in bytes.chunks(crate::file_delivery::CHUNK_BYTES).enumerate() {
            f.transport
                .stage_file_chunk(
                    &f.p,
                    &lease,
                    &target,
                    (i * crate::file_delivery::CHUNK_BYTES) as u64,
                    chunk,
                    &f.facts,
                    crate::api::now_ms(),
                )
                .await
                .unwrap();
        }
        let mut db = AsyncPgConnection::establish(&url).await.unwrap();
        let prefix = sql_query("SELECT octet_length(content)::bigint AS total FROM hagency_agent_v1.file_outbox WHERE id=$1").bind::<Text,_>(&target.file_id).get_result::<Count>(&mut db).await.unwrap();
        assert_eq!(
            prefix.total, 0,
            "normal staging must not rewrite a growing bytea"
        );
        let cross = crate::file_delivery::CHUNK_BYTES - 8;
        f.transport
            .stage_file_chunk(
                &f.p,
                &lease,
                &target,
                cross as u64,
                &bytes[cross..cross + 32],
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        for statement in [
            "UPDATE hagency_agent_v1.file_chunks SET content='bad'::bytea WHERE file_id=$1",
            "DELETE FROM hagency_agent_v1.file_chunks WHERE file_id=$1",
        ] {
            assert!(
                sql_query(statement)
                    .bind::<Text, _>(&target.file_id)
                    .execute(&mut db)
                    .await
                    .is_err()
            );
        }
        f.transport
            .queue_file(&f.p, &lease, &target, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        let claim = f
            .transport
            .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        assert_eq!(claim.bytes.as_deref(), Some(bytes.as_slice()));
        f.transport
            .confirm_file_upload(&target.file_id, &claim.token, None, crate::api::now_ms())
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
        assert_eq!(
            sql_query(
                "SELECT count(*) AS total FROM hagency_agent_v1.file_chunks WHERE file_id=$1"
            )
            .bind::<Text, _>(&target.file_id)
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .total,
            3
        );
        assert!(
            f.transport
                .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
                .await
                .is_err()
        );
    }
    admin
        .batch_execute(&format!("DROP DATABASE {name}"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_tiny_file_chunks_are_bounded_without_stranding_completion() {
    let (name, url, mut admin) = database().await;
    {
        let f = fixture_at(&url).await;
        let bytes = vec![b'a'; crate::file_delivery::MAX_CHUNKS + 1];
        let (lease, target, _, _) = setup(&f, &bytes).await;
        let mut db = AsyncPgConnection::establish(&url).await.unwrap();
        // Generated file IDs pass the same strict identifier validator as API
        // targets; this fixture loop retains both production SQL triggers.
        crate::key(&target.file_id).unwrap();
        db.batch_execute(&format!("DO $$ BEGIN FOR i IN 0..{} LOOP INSERT INTO hagency_agent_v1.file_chunks(file_id,byte_offset,content) VALUES('{}',i,'a'::bytea); UPDATE hagency_agent_v1.file_outbox SET received_bytes=i+1 WHERE id='{}'; END LOOP; END $$;",crate::file_delivery::MAX_CHUNKS-2,target.file_id,target.file_id)).await.unwrap();
        let offset = (crate::file_delivery::MAX_CHUNKS - 1) as u64;
        assert!(matches!(
            f.transport
                .stage_file_chunk(
                    &f.p,
                    &lease,
                    &target,
                    offset,
                    b"a",
                    &f.facts,
                    crate::api::now_ms()
                )
                .await,
            Err(Error::Invalid("file_chunk_limit"))
        ));
        f.transport
            .stage_file_chunk(
                &f.p,
                &lease,
                &target,
                offset,
                b"aa",
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        f.transport
            .queue_file(&f.p, &lease, &target, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        let claim = f
            .transport
            .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        assert_eq!(claim.bytes.as_deref(), Some(bytes.as_slice()));
        assert_eq!(
            sql_query(
                "SELECT count(*) AS total FROM hagency_agent_v1.file_chunks WHERE file_id=$1"
            )
            .bind::<Text, _>(&target.file_id)
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .total,
            crate::file_delivery::MAX_CHUNKS as i64
        );
    }
    admin
        .batch_execute(&format!("DROP DATABASE {name}"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_legacy_file_prefix_survives_additive_chunk_upgrade_and_terminal_cleanup() {
    let (name, url, mut admin) = database().await;
    {
        let f = fixture_at(&url).await;
        let (lease, target, _, _) = setup(&f, b"hello").await;
        let mut db = AsyncPgConnection::establish(&url).await.unwrap();
        db.batch_execute("DROP TRIGGER clear_terminal_file_chunks ON hagency_agent_v1.file_outbox; DROP TABLE hagency_agent_v1.file_chunks; DROP FUNCTION hagency_agent_v1.guard_file_chunk(); DROP FUNCTION hagency_agent_v1.clear_terminal_file_chunks(); DROP INDEX hagency_agent_v1.file_outbox_owner_active;").await.unwrap();
        let schema = include_str!("file_delivery/schema.sql");
        let start = schema
            .find("CREATE FUNCTION hagency_agent_v1.retain_file_outbox()")
            .unwrap();
        let end = schema.find("CREATE TRIGGER retain_file_outbox").unwrap();
        db.batch_execute(&schema[start..end].replacen(
            "CREATE FUNCTION",
            "CREATE OR REPLACE FUNCTION",
            1,
        ))
        .await
        .unwrap();
        sql_query(
            "UPDATE hagency_agent_v1.file_outbox SET content=$2,received_bytes=3 WHERE id=$1",
        )
        .bind::<Text, _>(&target.file_id)
        .bind::<Binary, _>(&b"hello"[..3])
        .execute(&mut db)
        .await
        .unwrap();
        let upgraded = TransportStore::open(&url, Limits::default()).await.unwrap();
        upgraded
            .stage_file_chunk(
                &f.p,
                &lease,
                &target,
                3,
                b"lo",
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        upgraded
            .stage_file_chunk(
                &f.p,
                &lease,
                &target,
                1,
                b"ell",
                &f.facts,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        upgraded
            .queue_file(&f.p, &lease, &target, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        let reopened = TransportStore::open(&url, Limits::default()).await.unwrap();
        let claim = reopened
            .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        assert_eq!(claim.bytes.as_deref(), Some(b"hello".as_slice()));
        reopened
            .confirm_file_upload(
                &target.file_id,
                &claim.token,
                Some("mxc://example.test/original"),
                crate::api::now_ms(),
            )
            .await
            .unwrap();
        let sending = reopened
            .claim_file(&target.file_id, &f.facts, crate::api::now_ms())
            .await
            .unwrap();
        reopened
            .confirm_file_sent(
                &target.file_id,
                &sending.token,
                Some("$original:example.test"),
            )
            .await
            .unwrap();
        assert_eq!(
            sql_query(
                "SELECT count(*) AS total FROM hagency_agent_v1.file_chunks WHERE file_id=$1"
            )
            .bind::<Text, _>(&target.file_id)
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .total,
            0
        );
        assert_eq!(
            reopened
                .file_delivery(&f.p, &target, crate::api::now_ms())
                .await
                .unwrap()
                .matrix_event_id
                .as_deref(),
            Some("$original:example.test")
        );
    }
    admin
        .batch_execute(&format!("DROP DATABASE {name}"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_pending_routing_cursor_wraps_with_permanent_history_and_partial_indexes() {
    let (name, url, mut admin) = database().await;
    {
        let f = fixture_at(&url).await;
        let _inbox = crate::appservice::Inbox::open(&url, secret_token())
            .await
            .unwrap();
        let mut db = AsyncPgConnection::establish(&url).await.unwrap();
        db.batch_execute("INSERT INTO hagency_agent_v1.inbound_transactions SELECT 'history_'||i::text,'review','{\"events\":[]}'::jsonb,0 FROM generate_series(1,100000)i; INSERT INTO hagency_agent_v1.routing_jobs SELECT id,'routed',0 FROM hagency_agent_v1.inbound_transactions; INSERT INTO hagency_agent_v1.inbound_transactions VALUES ('pending_a','review','{\"events\":[]}',1),('pending_b','review','{\"events\":[]}',2),('pending_c','review','{\"events\":[]}',3); INSERT INTO hagency_agent_v1.routing_jobs SELECT id,'pending',received_at_ms FROM hagency_agent_v1.inbound_transactions WHERE id LIKE 'pending_%'; ANALYZE hagency_agent_v1.routing_jobs;").await.unwrap();
        for (cursor, expected) in [
            ("pending_b", vec!["pending_c", "pending_a"]),
            ("pending_c", vec!["pending_a", "pending_b"]),
            ("", vec!["pending_a", "pending_b"]),
        ] {
            let rows = f
                .transport
                .routing_candidates_after(cursor, 2)
                .await
                .unwrap();
            assert_eq!(
                rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
        #[derive(diesel::QueryableByName)]
        struct Plan {
            #[diesel(sql_type=Json, column_name="QUERY PLAN")]
            value: serde_json::Value,
        }
        let plan=sql_query("EXPLAIN (FORMAT JSON) SELECT count(*) FROM hagency_agent_v1.routing_jobs WHERE state='pending'").get_result::<Plan>(&mut db).await.unwrap().value;
        assert!(plan.to_string().contains("routing_jobs_pending_"), "{plan}");
        assert_eq!(
            sql_query(
                "SELECT count(*) AS total FROM hagency_agent_v1.routing_jobs WHERE state='routed'"
            )
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .total,
            100000
        );
    }
    admin
        .batch_execute(&format!("DROP DATABASE {name}"))
        .await
        .unwrap();
}
