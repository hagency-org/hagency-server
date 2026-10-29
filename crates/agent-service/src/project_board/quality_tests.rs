use super::*;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_removed_attachment_bytes_release_quota_without_reviving_old_intents() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let (_, domain, alice, _, mut access) = fixture().await;
    let board = &domain.board;
    let created = board
        .mutate(
            &alice,
            &access,
            create("quota-task", "Attachment lifecycle"),
        )
        .await
        .unwrap();
    let task = created["task"]["id"].as_str().unwrap();
    let bytes = vec![17_u8; 2 * 1024 * 1024];
    let data = STANDARD.encode(&bytes);
    let upload = |key: &str| Mutation::UploadAttachment {
        task: task.into(),
        input: AttachmentUpload {
            idempotency_key: key.into(),
            filename: "fixture.bin".into(),
            content_type: "application/octet-stream".into(),
            data: data.clone(),
        },
    };
    let remove = |key: &str, attachment: &str| Mutation::RemoveAttachment {
        task: task.into(),
        input: AttachmentRemove {
            idempotency_key: key.into(),
            attachment: attachment.into(),
        },
    };
    let mut original = None;
    // Exceed the former lifetime 40MiB budget while keeping the active set empty.
    for n in 0..21 {
        access.observed = crate::api::now_ms();
        let result = board
            .mutate(&alice, &access, upload(&format!("turn-{n}")))
            .await
            .unwrap();
        let id = result["attachment"].as_str().unwrap();
        board
            .mutate(&alice, &access, remove(&format!("remove-{n}"), id))
            .await
            .unwrap();
        if n == 0 {
            original = Some(result);
        }
    }
    let original = original.unwrap();
    let original_id = original["attachment"].as_str().unwrap();
    let replay = board
        .mutate(&alice, &access, upload("turn-0"))
        .await
        .unwrap();
    assert_eq!(replay["attachment"], original["attachment"]);
    assert_eq!(replay["sequence"], original["sequence"]);
    assert_eq!(replay["replayed"], true);
    assert!(matches!(
        board.attachment(&alice, &access, task, original_id).await,
        Err(Error::NotFound("board_attachment_not_found"))
    ));
    let mut changed = upload("turn-0");
    if let Mutation::UploadAttachment { input, .. } = &mut changed {
        input.filename = "other.bin".into();
    }
    assert!(matches!(
        board.mutate(&alice, &access, changed).await,
        Err(Error::Conflict("board_idempotency_conflict"))
    ));
    let mut db = board.connection().await.unwrap();
    let tombstones = sql_query("SELECT jsonb_build_object('rows',count(*),'retainedBytes',coalesce(sum(octet_length(data)),0),'sizes',min(byte_size),'digests',bool_and(content_sha256=$3)) AS value FROM hagency_project_v1.attachments WHERE project_id=$1 AND task_id=$2 AND removed_at IS NOT NULL")
        .bind::<Text,_>(&access.project).bind::<Text,_>(task).bind::<Text,_>(hex::encode(Sha256::digest(&bytes)))
        .get_result::<Row>(&mut *db).await.unwrap().value;
    assert_eq!(tombstones["rows"], 21);
    assert_eq!(tombstones["retainedBytes"], 0);
    assert_eq!(tombstones["sizes"], bytes.len());
    assert_eq!(tombstones["digests"], true);
    drop(db);
    // The active count/byte bound still applies, and one removal frees one slot.
    let mut last_id = String::new();
    for n in 0..20 {
        access.observed = crate::api::now_ms();
        let result = board
            .mutate(&alice, &access, upload(&format!("active-{n}")))
            .await
            .unwrap();
        last_id = result["attachment"].as_str().unwrap().into();
    }
    assert!(matches!(
        board.mutate(&alice, &access, upload("overflow")).await,
        Err(Error::Conflict("board_attachment_limit"))
    ));
    board
        .mutate(&alice, &access, remove("free-active", &last_id))
        .await
        .unwrap();
    board
        .mutate(&alice, &access, upload("replacement"))
        .await
        .unwrap();
    assert_eq!(
        board.detail(&alice, &access, task).await.unwrap()["attachments"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    drop(domain);
    let reopened = DomainStore::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        "example.test",
        "_hagency_test_",
    )
    .await
    .unwrap();
    let replay = reopened
        .board
        .mutate(&alice, &access, upload("turn-0"))
        .await
        .unwrap();
    assert_eq!(replay["attachment"], original["attachment"]);
    assert!(matches!(
        reopened
            .board
            .attachment(&alice, &access, task, original_id)
            .await,
        Err(Error::NotFound("board_attachment_not_found"))
    ));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_v6_attachment_migration_retains_metadata_and_discards_only_removed_payloads() {
    use diesel::sql_types::Binary;
    use sha2::{Digest, Sha256};
    let (_, domain, alice, _, access) = fixture().await;
    let created = domain
        .board
        .mutate(
            &alice,
            &access,
            create("legacy-task", "Legacy attachment upgrade"),
        )
        .await
        .unwrap();
    let task = created["task"]["id"].as_str().unwrap();
    let bytes = b"legacy fixture bytes";
    let mut db = domain.board.connection().await.unwrap();
    // Reconstruct the exact v6 attachment shape inside a transaction. Roll back
    // the whole fixture afterward so other serial tests retain the v7 schema.
    let result = (*db).transaction::<(), Error, _>(async |db| {
        db.batch_execute("ALTER TABLE hagency_project_v1.attachments RENAME TO quality_original_attachments;
            ALTER INDEX hagency_project_v1.attachments_active_task RENAME TO quality_original_attachments_active_task;
            CREATE TABLE hagency_project_v1.attachments (
             id text CONSTRAINT quality_legacy_attachment_pk PRIMARY KEY, project_id text NOT NULL, task_id text NOT NULL,
             filename text NOT NULL, content_type text NOT NULL, data bytea NOT NULL CHECK(octet_length(data) BETWEEN 1 AND 2097152),
             created_by text NOT NULL, created_at bigint NOT NULL, removed_at bigint,
             FOREIGN KEY(task_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id));
            DELETE FROM hagency_project_v1.migrations WHERE version=7;").await?;
        for (id, removed) in [("legacy_active", None), ("legacy_removed", Some(7_i64))] {
            sql_query("INSERT INTO hagency_project_v1.attachments VALUES($1,$2,$3,'legacy.bin','application/octet-stream',$4,$5,1,$6)")
                .bind::<Text,_>(id).bind::<Text,_>(&access.project).bind::<Text,_>(task)
                .bind::<Binary,_>(bytes.as_slice()).bind::<Text,_>(&alice.mxid).bind::<Nullable<BigInt>,_>(removed)
                .execute(db).await?;
        }
        db.batch_execute(include_str!("schema_v7.sql")).await?;
        let rows = sql_query("SELECT jsonb_build_object('id',id,'size',byte_size,'digest',content_sha256,'bytes',octet_length(data),'removed',removed_at) AS value FROM hagency_project_v1.attachments ORDER BY id")
            .load::<Row>(db).await?;
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.value["size"], bytes.len());
            assert_eq!(row.value["digest"], hex::encode(Sha256::digest(bytes)));
        }
        assert_eq!(rows[0].value["bytes"], bytes.len());
        assert_eq!(rows[1].value["bytes"], Value::Null);
        assert_eq!(rows[1].value["removed"], 7);
        assert_eq!(sql_query("SELECT max(version)::bigint AS value FROM hagency_project_v1.migrations").get_result::<Number>(db).await?.value, 7);
        Err(Error::Conflict("migration_fixture_rollback"))
    }).await;
    assert!(matches!(
        result,
        Err(Error::Conflict("migration_fixture_rollback"))
    ));
    drop(db);
    // Also verify that normal startup accepts and preserves the upgraded shape.
    BoardStore::open(&std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_board_grouped_totals_and_summary_pages_preserve_filter_and_cursor_semantics() {
    let (_, domain, alice, bob, mut access) = fixture().await;
    let board = &domain.board;
    let mut tasks = Vec::new();
    let description = "d".repeat(30_000);
    for n in 0..8 {
        let state = match n {
            3 => "blocked",
            4 => "done",
            _ => "todo",
        };
        let result = board
            .mutate(
                &alice,
                &access,
                Mutation::Create(Create {
                    idempotency_key: format!("summary-{n}"),
                    state: state.into(),
                    fields: Fields {
                        title: if n == 6 {
                            "Unrelated".into()
                        } else {
                            format!("Match {n}")
                        },
                        description: description.clone(),
                        due_date: Some("2000-01-01".into()),
                        labels: if n % 2 == 0 {
                            vec!["scope".into()]
                        } else {
                            vec![]
                        },
                        assignee_mxid: Some(if n % 3 == 0 {
                            bob.mxid.clone()
                        } else {
                            alice.mxid.clone()
                        }),
                        checklist: (0..3)
                            .map(|i| ChecklistItem {
                                id: format!("item_{i}"),
                                title: format!("Step {i}"),
                                checked: i == 0,
                            })
                            .collect(),
                        ..Default::default()
                    },
                }),
            )
            .await
            .unwrap();
        let id = result["task"]["id"].as_str().unwrap().to_owned();
        if n == 7 {
            board
                .mutate(
                    &alice,
                    &access,
                    Mutation::Archive {
                        task: id.clone(),
                        input: Versioned {
                            idempotency_key: "summary-archive".into(),
                            expected_version: 1,
                        },
                    },
                )
                .await
                .unwrap();
        }
        tasks.push((id, state, n == 7));
    }
    let cases = [
        Query::default(),
        Query {
            query: Some("match".into()),
            ..Default::default()
        },
        Query {
            assignee: Some(bob.mxid.clone()),
            ..Default::default()
        },
        Query {
            label: Some("scope".into()),
            ..Default::default()
        },
        Query {
            overdue: true,
            ..Default::default()
        },
        Query {
            archived: true,
            ..Default::default()
        },
        Query {
            state: Some("blocked".into()),
            ..Default::default()
        },
        Query {
            query: Some("match".into()),
            label: Some("scope".into()),
            assignee: Some(alice.mxid.clone()),
            overdue: true,
            ..Default::default()
        },
    ];
    for mut query in cases {
        access.observed = crate::api::now_ms();
        query.limit = Some(2);
        let snapshot = board.board(&alice, &access, query.clone()).await.unwrap();
        for lane in snapshot["lanes"].as_array().unwrap() {
            let state = lane["state"].as_str().unwrap();
            assert_eq!(
                lane["taskCount"],
                tasks
                    .iter()
                    .filter(|(_, s, archived)| *s == state && !archived)
                    .count()
            );
            assert_eq!(
                lane["allTaskCount"],
                tasks.iter().filter(|(_, s, _)| *s == state).count()
            );
            if query.state.as_ref().is_some_and(|s| s != state) {
                assert_eq!(lane["total"], 0);
                assert!(lane["tasks"].as_array().unwrap().is_empty());
                continue;
            }
            let mut page_query = query.clone();
            page_query.state = Some(state.into());
            let expected = board
                .tasks(&alice, &access, page_query.clone())
                .await
                .unwrap();
            assert_eq!(lane["total"], expected["total"]);
            assert_eq!(lane["tasks"], expected["tasks"]);
            assert_eq!(lane["nextCursor"], expected["nextCursor"]);
            for task in lane["tasks"].as_array().unwrap() {
                assert!(task.get("description").is_none());
                assert!(task.get("checklist").is_none());
                assert_eq!(task["checklistTotal"], 3);
                assert_eq!(task["checklistDone"], 1);
            }
            if let Some(cursor) = lane["nextCursor"].as_str() {
                page_query.cursor = Some(cursor.into());
                page_query.sequence = lane["sequence"].as_i64();
                let next = board.tasks(&alice, &access, page_query).await.unwrap();
                let first_ids = lane["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| &t["id"])
                    .collect::<Vec<_>>();
                assert!(
                    next["tasks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|t| !first_ids.contains(&&t["id"]))
                );
            }
        }
    }
    let detail = board.detail(&alice, &access, &tasks[0].0).await.unwrap();
    assert_eq!(detail["task"]["description"], description);
    assert_eq!(detail["task"]["checklist"].as_array().unwrap().len(), 3);
}
