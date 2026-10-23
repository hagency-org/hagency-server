use super::*;
#[test]
fn board_fields_and_calendar_dates_are_validated() {
    for date in ["2024-02-29", "2000-02-29", "2026-10-23"] {
        assert!(validate_date(date).is_ok());
    }
    for date in [
        "1900-02-29",
        "2026-02-29",
        "2026-13-01",
        "0000-01-01",
        "2026-1-01",
        "2026-01-00",
        "2026-04-31",
    ] {
        assert!(validate_date(date).is_err(), "{date}");
    }
    let mut fields = Fields {
        title: "  Shared task  ".into(),
        labels: vec![" design ".into(), "design".into()],
        ..Default::default()
    };
    fields.validate().unwrap();
    assert_eq!(fields.title, "Shared task");
    assert_eq!(fields.labels, vec!["design"]);
    fields.title = " \n ".into();
    assert!(fields.validate().is_err());
}

async fn fixture() -> (Store, DomainStore, Principal, Principal, Access) {
    use crate::{domain::AdminFacts, identity::Identity, store::Store};
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = crate::secret_token();
    let now = crate::api::now_ms();
    async fn login(auth: &Store, name: String, now: i64) -> Principal {
        let grant = auth
            .sign_in(
                Identity {
                    issuer: "https://example.test/auth/".into(),
                    subject: name.clone(),
                    mxid: format!("@{name}:example.test"),
                    client_id: "board-test".into(),
                    valid_until_ms: now + 600_000,
                },
                now,
            )
            .await
            .unwrap();
        auth.authenticate(&grant.token, now, false).await.unwrap()
    }
    let alice = login(&auth, format!("alice_{suffix}"), now).await;
    let bob = login(&auth, format!("bob_{suffix}"), now).await;
    let space = format!("!board_{suffix}:example.test");
    let facts = AdminFacts {
        actor_mxid: alice.mxid.clone(),
        room_id: space.clone(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let project = domain
        .register_project(&alice, &space, &facts, now)
        .await
        .unwrap();
    let access = Access {
        project: project.id,
        space,
        actor: alice.mxid.clone(),
        members: BTreeSet::from([alice.mxid.clone(), bob.mxid.clone()]),
        observed: crate::api::now_ms(),
    };
    (auth, domain, alice, bob, access)
}
use crate::store::Store;
fn create(key: &str, title: &str) -> Mutation {
    Mutation::Create(Create {
        idempotency_key: key.into(),
        state: "todo".into(),
        fields: Fields {
            title: title.into(),
            ..Default::default()
        },
    })
}
fn movement(key: &str, task: &str, version: i64, state: &str) -> Mutation {
    Mutation::Move {
        task: task.into(),
        input: Move {
            idempotency_key: key.into(),
            expected_version: version,
            state: state.into(),
            expected_sequence: None,
            before: None,
            after: None,
            position: Position::Append,
        },
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_board_shared_storage_conflicts_replay_history_and_reopen() {
    let (_auth, domain, alice, bob, access) = fixture().await;
    let board = &domain.board;
    let empty = board
        .board(&alice, &access, Query::default())
        .await
        .unwrap();
    assert_eq!(empty["sequence"], 0);
    let first = board
        .mutate(&alice, &access, create("first", "Design the shared board"))
        .await
        .unwrap();
    let id = first["task"]["id"].as_str().unwrap();
    assert_eq!(first["sequence"], 1);
    let replay = board
        .mutate(&alice, &access, create("first", "Design the shared board"))
        .await
        .unwrap();
    assert_eq!(replay["task"], first["task"]);
    assert_eq!(replay["replayed"], true);
    assert!(matches!(
        board
            .mutate(&alice, &access, create("first", "Different payload"))
            .await,
        Err(Error::Conflict("board_idempotency_conflict"))
    ));
    let other = Access {
        actor: bob.mxid.clone(),
        members: access.members.clone(),
        project: access.project.clone(),
        space: access.space.clone(),
        observed: crate::api::now_ms(),
    };
    assert_eq!(
        board.detail(&bob, &other, id).await.unwrap()["task"]["title"],
        "Design the shared board"
    );
    let (a, b) = tokio::join!(
        board.mutate(&alice, &access, movement("move-a", id, 1, "in_progress")),
        board.mutate(&bob, &other, movement("move-b", id, 1, "blocked"))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(Error::Conflict("board_version_conflict"))
    ));
    let moved = board.detail(&bob, &other, id).await.unwrap();
    assert_eq!(moved["task"]["version"], 2);
    let comment = board
        .mutate(
            &bob,
            &other,
            Mutation::Comment {
                task: id.into(),
                input: Comment {
                    idempotency_key: "comment".into(),
                    body: "Looks good".into(),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(comment["sequence"], 3);
    let comments = board
        .history(&alice, &access, Some(id), 0, 1, true)
        .await
        .unwrap();
    assert_eq!(comments["items"][0]["body"], "Looks good");
    assert_eq!(comments["items"][0]["author"], bob.mxid);
    let archived = board
        .mutate(
            &alice,
            &access,
            Mutation::Archive {
                task: id.into(),
                input: Versioned {
                    idempotency_key: "archive".into(),
                    expected_version: 2,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(archived["task"]["version"], 3);
    assert_eq!(
        board.tasks(&bob, &other, Query::default()).await.unwrap()["total"],
        0
    );
    let archived_query = Query {
        archived: true,
        ..Default::default()
    };
    assert_eq!(
        board.tasks(&bob, &other, archived_query).await.unwrap()["total"],
        1
    );
    let restored = board
        .mutate(
            &bob,
            &other,
            Mutation::Restore {
                task: id.into(),
                input: Versioned {
                    idempotency_key: "restore".into(),
                    expected_version: 3,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(restored["task"]["version"], 4);
    let changes = board
        .history(&alice, &access, None, 0, 2, false)
        .await
        .unwrap();
    assert_eq!(changes["hasMore"], true);
    assert_eq!(changes["next"], 2);
    assert_eq!(changes["sequence"], 5);
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let reopened = BoardStore::open(&url).await.unwrap();
    assert_eq!(
        reopened.detail(&alice, &access, id).await.unwrap()["task"],
        restored["task"]
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_board_permission_revocation_and_cross_project_are_refused() {
    let (_auth, domain, alice, bob, access) = fixture().await;
    let board = &domain.board;
    let result = board
        .mutate(&alice, &access, create("one", "Private project task"))
        .await
        .unwrap();
    let id = result["task"]["id"].as_str().unwrap();
    let mut left = Access {
        actor: bob.mxid.clone(),
        project: access.project.clone(),
        space: access.space.clone(),
        members: BTreeSet::from([alice.mxid.clone()]),
        observed: crate::api::now_ms(),
    };
    assert!(matches!(
        board.detail(&bob, &left, id).await,
        Err(Error::Unauthorized(_))
    ));
    left.actor = alice.mxid.clone();
    left.project = "prj_missing".into();
    assert!(board.detail(&alice, &left, id).await.is_err());
    left.project = access.project.clone();
    left.observed = crate::api::now_ms() - 30_001;
    assert!(matches!(
        board
            .mutate(&alice, &left, create("stale", "Must not save"))
            .await,
        Err(Error::Unavailable("board_membership_expired"))
    ));
    let input = Mutation::Create(Create {
        idempotency_key: "assignee".into(),
        state: "todo".into(),
        fields: Fields {
            title: "Invalid assignee".into(),
            assignee_mxid: Some("@outsider:example.test".into()),
            ..Default::default()
        },
    });
    assert!(matches!(
        board.mutate(&alice, &access, input).await,
        Err(Error::Invalid("board_assignee_not_member"))
    ));
    let mut db = domain.db.lock().await.unwrap();
    sql_query("UPDATE hagency_agent_v1.sessions SET revoked=true WHERE id=$1")
        .bind::<Text, _>(&alice.session_id)
        .execute(&mut *db)
        .await
        .unwrap();
    drop(db);
    assert!(matches!(
        board.detail(&alice, &access, id).await,
        Err(Error::Unauthorized(_))
    ));
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_board_order_paging_filters_and_atomic_receipts() {
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    let mut ids = Vec::new();
    for i in 0..4 {
        let result = board
            .mutate(
                &alice,
                &access,
                create(&format!("create-{i}"), &format!("Task {i}")),
            )
            .await
            .unwrap();
        ids.push(result["task"]["id"].as_str().unwrap().to_owned());
    }
    let q = Query {
        state: Some("todo".into()),
        limit: Some(2),
        ..Default::default()
    };
    let first = board.tasks(&alice, &access, q.clone()).await.unwrap();
    assert_eq!(first["total"], 4);
    assert_eq!(first["tasks"].as_array().unwrap().len(), 2);
    let next = Query {
        cursor: Some(first["nextCursor"].as_str().unwrap().into()),
        sequence: Some(4),
        ..q.clone()
    };
    let second = board.tasks(&alice, &access, next.clone()).await.unwrap();
    assert_eq!(second["tasks"][0]["id"], ids[2]);
    assert_eq!(second["nextCursor"], Value::Null);
    let m = Move {
        idempotency_key: "reorder".into(),
        expected_version: 1,
        state: "todo".into(),
        expected_sequence: Some(4),
        before: Some(ids[1].clone()),
        after: Some(ids[0].clone()),
        position: Position::Between,
    };
    board
        .mutate(
            &alice,
            &access,
            Mutation::Move {
                task: ids[3].clone(),
                input: m,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        board.tasks(&alice, &access, next).await,
        Err(Error::Conflict("board_snapshot_changed"))
    ));
    let all = board
        .tasks(&alice, &access, Query::default())
        .await
        .unwrap();
    assert_eq!(all["tasks"][1]["id"], ids[3]);
    let filtered = board
        .tasks(
            &alice,
            &access,
            Query {
                query: Some("Task 2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["tasks"][0]["id"], ids[2]);
    let bad = Mutation::Move {
        task: ids[2].clone(),
        input: Move {
            idempotency_key: "bad-neighbour".into(),
            expected_version: 1,
            state: "todo".into(),
            expected_sequence: Some(5),
            before: Some(ids[3].clone()),
            after: None,
            position: Position::Between,
        },
    };
    assert!(matches!(
        board.mutate(&alice, &access, bad).await,
        Err(Error::Conflict("board_neighbours_changed"))
    ));
    assert_eq!(
        board
            .board(&alice, &access, Query::default())
            .await
            .unwrap()["sequence"],
        5
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_board_http_authentication_live_membership_and_typed_payloads() {
    use crate::{
        api::App,
        gateway::Gateway,
        identity::{Identity, Verifier},
    };
    use salvo::{
        Service,
        http::StatusCode,
        test::{ResponseExt, TestClient},
    };
    use std::sync::{Arc, RwLock};
    let (auth, domain, alice, bob, access) = fixture().await;
    async fn token(auth: &Store, p: &Principal) -> String {
        auth.sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: p.subject.clone(),
                mxid: p.mxid.clone(),
                client_id: "board-http-test".into(),
                valid_until_ms: crate::api::now_ms() + 600000,
            },
            crate::api::now_ms(),
        )
        .await
        .unwrap()
        .token
    }
    let alice_token = token(&auth, &alice).await;
    let bob_token = token(&auth, &bob).await;
    let members = Arc::new(RwLock::new(access.members.clone()));
    let state = members.clone();
    let space = access.space.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let state = state.clone();
            let space = space.clone();
            Box::pin(async move {
                if room != space {
                    return Err(Error::Unavailable("unknown_space"));
                }
                let mut events = vec![
                    json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
                ];
                events.extend(state.read().unwrap().iter().map(|m|json!({"type":"m.room.member","state_key":m,"content":{"membership":"join"}})));
                Ok(events)
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let verifier = Verifier::new(
        "https://example.test/auth/".parse().unwrap(),
        "https://example.test/auth/introspect".parse().unwrap(),
        "https://example.test/".parse().unwrap(),
        "fixture-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let app = App::new(
        auth,
        verifier,
        "https://example.test/".parse().unwrap(),
        "https://example.test/auth/".parse().unwrap(),
        "example.test".into(),
    )
    .with_domain(domain, gateway);
    let service = Service::new(app.router());
    let root = format!(
        "https://example.test/hagency/api/v1/projects/{}",
        access.project
    );
    let mut response=TestClient::post(format!("{root}/tasks")).add_header("host","example.test",true).bearer_auth(&alice_token).json(&json!({"idempotencyKey":"http-create","state":"todo","fields":{"title":"From real HTTP","labels":["ui"]}})).send(&service).await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let created = response.take_json::<Value>().await.unwrap();
    let id = created["task"]["id"].as_str().unwrap();
    let mut response = TestClient::get(format!("{root}/board?query=real&label=ui"))
        .add_header("host", "example.test", true)
        .bearer_auth(&bob_token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    assert_eq!(
        response.take_json::<Value>().await.unwrap()["lanes"][0]["total"],
        1
    );
    let response=TestClient::post(format!("{root}/tasks")).add_header("host","example.test",true).bearer_auth(&alice_token).json(&json!({"idempotencyKey":"forged","state":"todo","fields":{"title":"forged"},"mxid":alice.mxid})).send(&service).await;
    assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
    // Larger than the ordinary API body cap, but within the attachment-only cap.
    use base64::{Engine, engine::general_purpose::STANDARD};
    let data = STANDARD.encode(vec![42u8; 150_000]);
    let payload = json!({"idempotencyKey":"http-upload","filename":"fixture.bin","contentType":"application/octet-stream","data":data});
    let response = TestClient::post(format!("{root}/tasks/{id}/attachments"))
        .add_header("host", "example.test", true)
        .bearer_auth(&alice_token)
        .json(&payload)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let response = TestClient::post(format!("{root}/tasks/{id}/comments"))
        .add_header("host", "example.test", true)
        .bearer_auth(&alice_token)
        .json(&json!({"idempotencyKey":"oversize-comment","body":"x".repeat(150_000)}))
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::PAYLOAD_TOO_LARGE));
    let mut response = TestClient::get(format!("{root}/tasks/{id}"))
        .add_header("host", "example.test", true)
        .bearer_auth(&bob_token)
        .send(&service)
        .await;
    let detail = response.take_json::<Value>().await.unwrap();
    let attachment = detail["attachments"][0]["id"].as_str().unwrap();
    let response = TestClient::get(format!("{root}/tasks/{id}/attachments/{attachment}"))
        .add_header("host", "example.test", true)
        .bearer_auth(&bob_token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    members.write().unwrap().remove(&bob.mxid);
    for path in [
        format!("{root}/board"),
        format!("{root}/tasks/{id}"),
        format!("{root}/board/changes?after=0"),
        format!("{root}/board/notifications"),
        format!("{root}/tasks/{id}/attachments/{attachment}"),
    ] {
        let response = TestClient::get(path)
            .add_header("host", "example.test", true)
            .bearer_auth(&bob_token)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    }
    let response = TestClient::get(format!("{root}/board"))
        .add_header("host", "example.test", true)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let response = TestClient::get(format!("{root}/board?limit=1000"))
        .add_header("host", "example.test", true)
        .bearer_auth(&alice_token)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
    let disabled = Service::new(app.with_project_board_enabled(false).router());
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&disabled)
        .await;
    let discovery = response.take_json::<Value>().await.unwrap();
    assert!(
        !discovery["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == CAPABILITY)
    );
    let response = TestClient::get(format!("{root}/board"))
        .add_header("host", "example.test", true)
        .bearer_auth(&alice_token)
        .send(&disabled)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::NOT_FOUND));
}

#[test]
fn columns_and_checklists_have_bounded_stable_contracts() {
    let mut config = ConfigureColumns {
        idempotency_key: "columns".into(),
        expected_sequence: 0,
        columns: default_columns(),
    };
    config.validate().unwrap();
    config.columns[0].wip_limit = Some(0);
    assert!(config.validate().is_err());
    config.columns[0].wip_limit = None;
    for c in &mut config.columns {
        c.is_done = false;
    }
    assert!(config.validate().is_err());
    let mut fields = Fields {
        title: "Checklist".into(),
        checklist: vec![ChecklistItem {
            id: "one".into(),
            title: " First ".into(),
            checked: false,
        }],
        ..Default::default()
    };
    fields.validate().unwrap();
    assert_eq!(fields.checklist[0].title, "First");
    fields.checklist.push(fields.checklist[0].clone());
    assert!(fields.validate().is_err());
    // Empty added fields must preserve the v1 receipt digest for original retries.
    let wire = serde_json::to_value(Fields::default()).unwrap();
    assert!(wire.get("parentId").is_none());
    assert!(wire.get("checklist").is_none());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_custom_columns_wip_races_and_config_replay() {
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    let mut columns = default_columns();
    columns.insert(
        1,
        ColumnDefinition {
            id: "testing".into(),
            title: "验证".into(),
            wip_limit: Some(1),
            is_done: false,
        },
    );
    let config = Mutation::ConfigureColumns(ConfigureColumns {
        idempotency_key: "columns".into(),
        expected_sequence: 0,
        columns: columns.clone(),
    });
    let ack = board.mutate(&alice, &access, config.clone()).await.unwrap();
    assert_eq!(ack["sequence"], 1);
    assert_eq!(
        board.mutate(&alice, &access, config).await.unwrap()["replayed"],
        true
    );
    let create_in = |key: &str| {
        Mutation::Create(Create {
            idempotency_key: key.into(),
            state: "testing".into(),
            fields: Fields {
                title: key.into(),
                ..Default::default()
            },
        })
    };
    let (a, b) = tokio::join!(
        board.mutate(&alice, &access, create_in("a")),
        board.mutate(&alice, &access, create_in("b"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (task, error) = if let Ok(t) = a {
        (t, b.unwrap_err())
    } else {
        (b.unwrap(), a.unwrap_err())
    };
    assert!(matches!(error, Error::Conflict("board_wip_limit_reached")));
    let id = task["task"]["id"].as_str().unwrap();
    board
        .mutate(&alice, &access, movement("same", id, 1, "testing"))
        .await
        .unwrap();
    let todo = board
        .mutate(&alice, &access, create("todo", "Second"))
        .await
        .unwrap();
    let other = todo["task"]["id"].as_str().unwrap();
    assert!(matches!(
        board
            .mutate(&alice, &access, movement("full", other, 1, "testing"))
            .await,
        Err(Error::Conflict("board_wip_limit_reached"))
    ));
    let snapshot = board
        .board(
            &alice,
            &access,
            Query {
                query: Some("no-match".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(snapshot["lanes"][1]["total"], 0);
    assert_eq!(snapshot["lanes"][1]["taskCount"], 1);
    columns.remove(1);
    assert!(matches!(
        board
            .mutate(
                &alice,
                &access,
                Mutation::ConfigureColumns(ConfigureColumns {
                    idempotency_key: "remove-nonempty".into(),
                    expected_sequence: snapshot["sequence"].as_i64().unwrap(),
                    columns
                })
            )
            .await,
        Err(Error::Conflict("board_column_not_empty"))
    ));
    board
        .mutate(
            &alice,
            &access,
            Mutation::Archive {
                task: id.into(),
                input: Versioned {
                    idempotency_key: "archive".into(),
                    expected_version: 2,
                },
            },
        )
        .await
        .unwrap();
    board
        .mutate(&alice, &access, movement("admit", other, 1, "testing"))
        .await
        .unwrap();
    assert!(matches!(
        board
            .mutate(
                &alice,
                &access,
                Mutation::Restore {
                    task: id.into(),
                    input: Versioned {
                        idempotency_key: "restore-full".into(),
                        expected_version: 3
                    }
                }
            )
            .await,
        Err(Error::Conflict("board_wip_limit_reached"))
    ));
    let reopened = BoardStore::open(&std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let snapshot = reopened
        .board(&alice, &access, Query::default())
        .await
        .unwrap();
    assert_eq!(snapshot["lanes"][1]["title"], "验证");
    assert_eq!(snapshot["lanes"][1]["wipLimit"], 1);
    let mut ordered = BoardStore::columns(&mut board.connection().await.unwrap(), &access.project)
        .await
        .unwrap();
    ordered.push(ColumnDefinition {
        id: "temporary_empty".into(),
        title: "Temporary empty".into(),
        wip_limit: None,
        is_done: false,
    });
    let added = board
        .mutate(
            &alice,
            &access,
            Mutation::ConfigureColumns(ConfigureColumns {
                idempotency_key: "add-empty".into(),
                expected_sequence: snapshot["sequence"].as_i64().unwrap(),
                columns: ordered.clone(),
            }),
        )
        .await
        .unwrap();
    ordered.pop();
    board
        .mutate(
            &alice,
            &access,
            Mutation::ConfigureColumns(ConfigureColumns {
                idempotency_key: "delete-empty".into(),
                expected_sequence: added["sequence"].as_i64().unwrap(),
                columns: ordered,
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        board
            .board(&alice, &access, Query::default())
            .await
            .unwrap()["lanes"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_subtasks_checklist_versions_and_independent_completion() {
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    let parent = board
        .mutate(&alice, &access, create("parent", "Parent"))
        .await
        .unwrap();
    let parent_id = parent["task"]["id"].as_str().unwrap();
    let fields = Fields {
        title: "Child".into(),
        parent_id: Some(parent_id.into()),
        checklist: vec![ChecklistItem {
            id: "item".into(),
            title: "Verify actual UI".into(),
            checked: false,
        }],
        ..Default::default()
    };
    let child = board
        .mutate(
            &alice,
            &access,
            Mutation::Create(Create {
                idempotency_key: "child".into(),
                state: "todo".into(),
                fields: fields.clone(),
            }),
        )
        .await
        .unwrap();
    let id = child["task"]["id"].as_str().unwrap();
    let mut checked = fields.clone();
    checked.checklist[0].checked = true;
    let update = Mutation::Update {
        task: id.into(),
        input: Update {
            idempotency_key: "check".into(),
            expected_version: 1,
            fields: checked,
        },
    };
    let ack = board.mutate(&alice, &access, update.clone()).await.unwrap();
    assert_eq!(ack["task"]["checklistDone"], 1);
    assert_eq!(
        board.mutate(&alice, &access, update).await.unwrap()["replayed"],
        true
    );
    assert!(matches!(
        board
            .mutate(
                &alice,
                &access,
                Mutation::Update {
                    task: id.into(),
                    input: Update {
                        idempotency_key: "stale".into(),
                        expected_version: 1,
                        fields: fields.clone()
                    }
                }
            )
            .await,
        Err(Error::Conflict("board_version_conflict"))
    ));
    for invalid in [id, parent_id] {
        let mut nested = fields.clone();
        nested.parent_id = Some(invalid.into());
        assert!(matches!(
            board
                .mutate(
                    &alice,
                    &access,
                    Mutation::Update {
                        task: parent_id.into(),
                        input: Update {
                            idempotency_key: format!("cycle-{invalid}"),
                            expected_version: 1,
                            fields: nested
                        }
                    }
                )
                .await,
            Err(Error::Invalid("invalid_board_parent"))
        ));
    }
    let detail = board.detail(&alice, &access, parent_id).await.unwrap();
    assert_eq!(detail["subtasks"].as_array().unwrap().len(), 1);
    assert_eq!(detail["task"]["subtaskTotal"], 1);
    board
        .mutate(&alice, &access, movement("done", id, 2, "done"))
        .await
        .unwrap();
    let detail = board.detail(&alice, &access, parent_id).await.unwrap();
    assert_eq!(detail["task"]["subtaskDone"], 1);
    assert_eq!(detail["task"]["state"], "todo");
    let restored = board.detail(&alice, &access, id).await.unwrap();
    assert_eq!(restored["task"]["checklist"][0]["checked"], true);
}

#[test]
fn attachments_reject_invalid_content_names_and_sizes() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut input = AttachmentUpload {
        idempotency_key: "upload".into(),
        filename: "picture.png".into(),
        content_type: "image/png".into(),
        data: STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture"),
    };
    assert!(collaboration::attachment_bytes(&input).is_ok());
    input.filename = "../picture.png".into();
    assert!(collaboration::attachment_bytes(&input).is_err());
    input.filename = "picture.png".into();
    input.content_type = "image/jpeg".into();
    assert!(collaboration::attachment_bytes(&input).is_err());
    input.content_type = "application/octet-stream".into();
    input.data = STANDARD.encode(vec![0; 2 * 1024 * 1024 + 1]);
    assert!(collaboration::attachment_bytes(&input).is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_collaboration_notifications_diffs_attachments_and_authorization() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (_auth, domain, alice, bob, access) = fixture().await;
    let board = &domain.board;
    let bob_access = Access {
        project: access.project.clone(),
        space: access.space.clone(),
        actor: bob.mxid.clone(),
        members: access.members.clone(),
        observed: crate::api::now_ms(),
    };
    let created = board
        .mutate(
            &alice,
            &access,
            create("collaboration-create", "Collaboration test"),
        )
        .await
        .unwrap();
    let task = created["task"]["id"].as_str().unwrap();
    let fields = Fields {
        title: "Assigned collaboration test".into(),
        assignee_mxid: Some(bob.mxid.clone()),
        ..Default::default()
    };
    board
        .mutate(
            &alice,
            &access,
            Mutation::Update {
                task: task.into(),
                input: Update {
                    idempotency_key: "collaboration-assign".into(),
                    expected_version: 1,
                    fields,
                },
            },
        )
        .await
        .unwrap();
    let inbox = board.notifications(&bob, &bob_access, 0).await.unwrap();
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][0]["taskId"], task);
    assert!(
        inbox["items"][0]["details"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["field"] == "assigneeMxid" && f["after"] == bob.mxid)
    );
    assert_eq!(
        board.notifications(&alice, &access, 0).await.unwrap()["unread"],
        0
    );
    let sequence = inbox["items"][0]["sequence"].as_i64().unwrap();
    board
        .read_notification(
            &alice,
            &access,
            NotificationRead {
                sequence,
                all_before: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        board.notifications(&bob, &bob_access, 0).await.unwrap()["unread"],
        1
    );
    board
        .read_notification(
            &bob,
            &bob_access,
            NotificationRead {
                sequence,
                all_before: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        board.notifications(&bob, &bob_access, 0).await.unwrap()["unread"],
        0
    );
    let bytes = b"\x89PNG\r\n\x1a\nattachment fixture";
    let upload = || Mutation::UploadAttachment {
        task: task.into(),
        input: AttachmentUpload {
            idempotency_key: "collaboration-file".into(),
            filename: "design.png".into(),
            content_type: "image/png".into(),
            data: STANDARD.encode(bytes),
        },
    };
    board.mutate(&alice, &access, upload()).await.unwrap();
    assert_eq!(
        board.mutate(&alice, &access, upload()).await.unwrap()["replayed"],
        true
    );
    let detail = board.detail(&bob, &bob_access, task).await.unwrap();
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(detail["task"]["attachmentCount"], 1);
    let attachment = detail["attachments"][0]["id"].as_str().unwrap();
    assert_eq!(
        board
            .attachment(&bob, &bob_access, task, attachment)
            .await
            .unwrap()["data"],
        STANDARD.encode(bytes)
    );
    assert!(
        board
            .attachment(&bob, &bob_access, "other_task", attachment)
            .await
            .is_err()
    );
    let mut revoked = Access {
        project: access.project.clone(),
        space: access.space.clone(),
        actor: bob.mxid.clone(),
        members: access.members.clone(),
        observed: crate::api::now_ms(),
    };
    revoked.members.remove(&bob.mxid);
    assert!(
        board
            .attachment(&bob, &revoked, task, attachment)
            .await
            .is_err()
    );
    assert!(board.notifications(&bob, &revoked, 0).await.is_err());
    assert_eq!(
        board.notifications(&bob, &bob_access, 0).await.unwrap()["unread"],
        1
    );
    board
        .mutate(
            &alice,
            &access,
            Mutation::RemoveAttachment {
                task: task.into(),
                input: AttachmentRemove {
                    idempotency_key: "collaboration-remove".into(),
                    attachment: attachment.into(),
                },
            },
        )
        .await
        .unwrap();
    assert!(
        board
            .attachment(&bob, &bob_access, task, attachment)
            .await
            .is_err()
    );
    assert_eq!(
        board.detail(&bob, &bob_access, task).await.unwrap()["task"]["attachmentCount"],
        0
    );
}
