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
        member_names: Default::default(),
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
        member_names: Default::default(),
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
        member_names: Default::default(),
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
            archived: false,
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
        archived: false,
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
            state: None,
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
                        state: None,
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
                            state: None,
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
        member_names: Default::default(),
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
                    state: None,
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
    let uploaded = board.mutate(&alice, &access, upload()).await.unwrap();
    assert!(uploaded["attachment"].as_str().unwrap().starts_with("att_"));
    assert_eq!(
        uploaded["attachment"],
        board.mutate(&alice, &access, upload()).await.unwrap()["attachment"]
    );
    assert_eq!(
        board.mutate(&alice, &access, upload()).await.unwrap()["replayed"],
        true
    );
    let detail = board.detail(&bob, &bob_access, task).await.unwrap();
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(detail["task"]["attachmentCount"], 1);
    let attachment = detail["attachments"][0]["id"].as_str().unwrap();
    assert_eq!(uploaded["attachment"], attachment);
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
        member_names: Default::default(),
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

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_atomic_status_save_and_archived_column_restore_preserve_cards() {
    let (_auth, domain, alice, bob, mut access) = fixture().await;
    access
        .member_names
        .insert(alice.mxid.clone(), "Project coordinator".into());
    let board = &domain.board;
    let first = board
        .mutate(&alice, &access, create("first", "Original title"))
        .await
        .unwrap();
    let id = first["task"]["id"].as_str().unwrap().to_owned();
    let mut columns = default_columns();
    columns[1].wip_limit = Some(1);
    board
        .mutate(
            &alice,
            &access,
            Mutation::ConfigureColumns(ConfigureColumns {
                idempotency_key: "limits".into(),
                expected_sequence: 1,
                columns: columns.clone(),
            }),
        )
        .await
        .unwrap();
    let update = |key: &str, version, title: &str, state: &str| Mutation::Update {
        task: id.clone(),
        input: Update {
            idempotency_key: key.into(),
            expected_version: version,
            state: Some(state.into()),
            fields: Fields {
                title: title.into(),
                ..Default::default()
            },
        },
    };
    let mutation = update("atomic", 1, "Title and status together", "in_progress");
    let ack = board
        .mutate(&alice, &access, mutation.clone())
        .await
        .unwrap();
    assert_eq!(ack["task"]["state"], "in_progress");
    assert_eq!(ack["task"]["title"], "Title and status together");
    assert_eq!(ack["task"]["version"], 2);
    assert_eq!(
        board.mutate(&alice, &access, mutation).await.unwrap()["task"],
        ack["task"]
    );
    let other = Access {
        actor: bob.mxid.clone(),
        project: access.project.clone(),
        space: access.space.clone(),
        members: access.members.clone(),
        member_names: access.member_names.clone(),
        observed: access.observed,
    };
    assert!(matches!(
        board
            .mutate(&bob, &other, update("stale", 1, "Lost update", "done"))
            .await,
        Err(Error::Conflict("board_version_conflict"))
    ));
    let second = board
        .mutate(&alice, &access, create("second", "Another task"))
        .await
        .unwrap();
    let second_id = second["task"]["id"].as_str().unwrap();
    let full = Mutation::Update {
        task: second_id.into(),
        input: Update {
            idempotency_key: "full".into(),
            expected_version: 1,
            state: Some("in_progress".into()),
            fields: Fields {
                title: "Must not partially change".into(),
                ..Default::default()
            },
        },
    };
    assert!(matches!(
        board.mutate(&alice, &access, full).await,
        Err(Error::Conflict("board_wip_limit_reached"))
    ));
    let unchanged = board.detail(&alice, &access, second_id).await.unwrap();
    assert_eq!(unchanged["task"]["title"], "Another task");
    assert_eq!(unchanged["task"]["state"], "todo");
    assert_eq!(unchanged["task"]["version"], 1);
    // Archive a non-empty column, and reorder it in the same atomic configuration.
    columns[1].archived = true;
    columns.swap(0, 1);
    let snapshot = board
        .board(&alice, &access, Query::default())
        .await
        .unwrap();
    let seq = snapshot["sequence"].as_i64().unwrap();
    board
        .mutate(
            &alice,
            &access,
            Mutation::ConfigureColumns(ConfigureColumns {
                idempotency_key: "archive-list".into(),
                expected_sequence: seq,
                columns: columns.clone(),
            }),
        )
        .await
        .unwrap();
    let snapshot = board.board(&bob, &other, Query::default()).await.unwrap();
    assert_eq!(snapshot["lanes"][0]["state"], "in_progress");
    assert_eq!(snapshot["lanes"][0]["archived"], true);
    for (key, value) in snapshot["lanes"][0]["tasks"][0].as_object().unwrap() {
        assert_eq!(value, &ack["task"][key], "summary field {key}");
    }
    assert_eq!(
        board.detail(&bob, &other, &id).await.unwrap()["task"],
        ack["task"]
    );
    assert_eq!(snapshot["memberNames"][&alice.mxid], "Project coordinator");
    assert!(matches!(
        board
            .mutate(
                &alice,
                &access,
                update("archived-write", 2, "Forbidden", "done")
            )
            .await,
        Err(Error::Conflict("board_column_archived"))
    ));
    // Restore the exact ordered column; task metadata and version are unchanged.
    columns[0].archived = false;
    board
        .mutate(
            &alice,
            &access,
            Mutation::ConfigureColumns(ConfigureColumns {
                idempotency_key: "restore-list".into(),
                expected_sequence: snapshot["sequence"].as_i64().unwrap(),
                columns,
            }),
        )
        .await
        .unwrap();
    let restored = board.board(&bob, &other, Query::default()).await.unwrap();
    assert_eq!(restored["lanes"][0]["archived"], false);
    for (key, value) in restored["lanes"][0]["tasks"][0].as_object().unwrap() {
        assert_eq!(value, &ack["task"][key], "restored summary field {key}");
    }
    assert_eq!(
        board.detail(&bob, &other, &id).await.unwrap()["task"],
        ack["task"]
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_label_suggestions_ignore_filters_pages_and_archive_and_stay_bounded() {
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    let mut first = None;
    for page in 0..18 {
        let result = board
            .mutate(
                &alice,
                &access,
                Mutation::Create(Create {
                    idempotency_key: format!("labels-{page}"),
                    state: "todo".into(),
                    fields: Fields {
                        title: format!("catalog task {page}"),
                        labels: (page * 12..(page + 1) * 12)
                            .map(|i| format!("catalog-{i:03}"))
                            .collect(),
                        ..Default::default()
                    },
                }),
            )
            .await
            .unwrap();
        if page == 0 {
            first = result["task"]["id"].as_str().map(str::to_owned);
        }
    }
    board
        .mutate(
            &alice,
            &access,
            Mutation::Archive {
                task: first.unwrap(),
                input: Versioned {
                    idempotency_key: "archive-labels".into(),
                    expected_version: 1,
                },
            },
        )
        .await
        .unwrap();
    let snapshot = board
        .board(
            &alice,
            &access,
            Query {
                label: Some("no-matches".into()),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        snapshot["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["tasks"].as_array().unwrap().is_empty())
    );
    assert_eq!(snapshot["labelSuggestions"].as_array().unwrap().len(), 200);
    assert_eq!(snapshot["labelSuggestions"][0], "catalog-000");
    assert_eq!(snapshot["labelSuggestions"][199], "catalog-199");
    assert_eq!(snapshot["hasMoreLabels"], true);
    assert_eq!(snapshot["lanes"][0]["taskCount"], 17);
    assert_eq!(snapshot["lanes"][0]["allTaskCount"], 18);
    let archived = board
        .board(
            &alice,
            &access,
            Query {
                archived: true,
                label: Some("catalog-000".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(archived["lanes"][0]["total"], 1);
    assert_eq!(archived["labelSuggestions"], snapshot["labelSuggestions"]);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_multiple_boards_keep_tasks_receipts_and_cursors_isolated() {
    use super::catalog::NewBoard;
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    let original = board
        .mutate(
            &alice,
            &access,
            create("same-intent", "Original board task"),
        )
        .await
        .unwrap();
    let catalog = board.catalog(&alice, &access, None).await.unwrap();
    assert_eq!(catalog["boards"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["boards"][0]["name"], "Main Board");
    let input = || {
        Some(NewBoard {
            idempotency_key: "new-board".into(),
            name: "Planning".into(),
        })
    };
    let created = board.catalog(&alice, &access, input()).await.unwrap();
    let repeated = board.catalog(&alice, &access, input()).await.unwrap();
    assert_eq!(created, repeated);
    assert_eq!(created["boards"].as_array().unwrap().len(), 2);
    assert!(matches!(
        board
            .catalog(
                &alice,
                &access,
                Some(NewBoard {
                    idempotency_key: "new-board".into(),
                    name: "Changed intent".into()
                })
            )
            .await,
        Err(Error::Conflict("board_intent_conflict"))
    ));
    let scope = created["boards"][1]["id"].as_str().unwrap();
    board.resolve_board(&access.project, scope).await.unwrap();
    assert!(board.resolve_board("another-project", scope).await.is_err());
    let other = Access {
        project: scope.into(),
        space: access.space.clone(),
        actor: access.actor.clone(),
        members: access.members.clone(),
        member_names: access.member_names.clone(),
        observed: crate::api::now_ms(),
    };
    let task = board
        .mutate(&alice, &other, create("same-intent", "Second board task"))
        .await
        .unwrap();
    assert_eq!(task["task"]["number"], 1);
    assert_eq!(original["task"]["number"], 1);
    assert_ne!(task["task"]["id"], original["task"]["id"]);
    assert!(
        board
            .detail(&alice, &other, original["task"]["id"].as_str().unwrap())
            .await
            .is_err()
    );
    assert!(
        board
            .detail(&alice, &access, task["task"]["id"].as_str().unwrap())
            .await
            .is_err()
    );
    let summary = board.overview(&alice, &access).await.unwrap();
    assert_eq!(summary["stats"]["tasks"], 2);
    assert_eq!(summary["activity"].as_array().unwrap().len(), 2);
    let reopened = BoardStore::open(&std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened.catalog(&alice, &access, None).await.unwrap()["boards"],
        created["boards"]
    );
    assert_eq!(
        reopened
            .board(&alice, &other, Query::default())
            .await
            .unwrap()["lanes"][0]["tasks"][0]["title"],
        "Second board task"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_project_archive_requires_fresh_admin_and_restores_original_boards() {
    use crate::domain::AdminFacts;
    let (_auth, domain, alice, bob, access) = fixture().await;
    let original = domain
        .board
        .mutate(&alice, &access, create("before-archive", "Keep this task"))
        .await
        .unwrap();
    let revision = domain
        .project(&alice, &access.project, crate::api::now_ms())
        .await
        .unwrap()
        .revision;
    let mut facts = AdminFacts {
        actor_mxid: bob.mxid.clone(),
        room_id: access.space.clone(),
        observed_at_ms: crate::api::now_ms(),
        joined: true,
        can_manage_policy: false,
        is_space: true,
        linked_space_id: None,
    };
    assert!(
        domain
            .archive_project(
                &bob,
                &access.project,
                revision,
                true,
                &facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    facts.actor_mxid = alice.mxid.clone();
    facts.can_manage_policy = true;
    let archived = domain
        .archive_project(
            &alice,
            &access.project,
            revision,
            true,
            &facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(archived.archived);
    assert!(
        domain
            .projects(&alice, crate::api::now_ms())
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == access.project && p.archived)
    );
    assert_eq!(
        domain
            .board
            .board(&alice, &access, Query::default())
            .await
            .unwrap()["readOnly"],
        true
    );
    assert_eq!(
        domain
            .board
            .history(&alice, &access, None, 0, 10, false)
            .await
            .unwrap()["readOnly"],
        true
    );
    assert!(matches!(
        domain
            .board
            .mutate(&alice, &access, create("after-archive", "Refused"))
            .await,
        Err(Error::Conflict("project_archived"))
    ));
    assert!(
        domain
            .archive_project(
                &alice,
                &access.project,
                revision,
                false,
                &facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    let stale = AdminFacts {
        observed_at_ms: crate::api::now_ms() - 31_000,
        ..facts.clone()
    };
    assert!(
        domain
            .archive_project(
                &alice,
                &access.project,
                archived.revision,
                false,
                &stale,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    let restored = domain
        .archive_project(
            &alice,
            &access.project,
            archived.revision,
            false,
            &facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(!restored.archived);
    assert_eq!(
        domain
            .board
            .detail(&alice, &access, original["task"]["id"].as_str().unwrap())
            .await
            .unwrap()["task"],
        original["task"]
    );
    domain
        .board
        .mutate(&alice, &access, create("after-restore", "Allowed"))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_navigation_defaults_are_shared_versioned_authorized_and_scope_safe() {
    use super::catalog::{NewBoard, SetNavigationDefault};
    use crate::domain::AdminFacts;
    let (_auth, domain, alice, bob, access) = fixture().await;
    let board = &domain.board;
    let facts = AdminFacts {
        actor_mxid: alice.mxid.clone(),
        room_id: access.space.clone(),
        observed_at_ms: crate::api::now_ms(),
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let room_a = format!("!a_{}:example.test", crate::secret_token());
    let room_b = format!("!z_{}:example.test", crate::secret_token());
    for room in [&room_a, &room_b] {
        let rf = AdminFacts {
            room_id: room.clone(),
            is_space: false,
            linked_space_id: Some(access.space.clone()),
            ..facts.clone()
        };
        domain
            .register_room(
                &alice,
                &access.project,
                room,
                &facts,
                &rf,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
    }
    let catalog = board
        .catalog(
            &alice,
            &access,
            Some(NewBoard {
                idempotency_key: "default-board-test".into(),
                name: "A named board".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(catalog["boards"][0]["id"], access.project);
    assert_eq!(catalog["boards"][0]["isDefault"], true);
    let named = catalog["createdBoardId"].as_str().unwrap().to_owned();
    let input = |kind: &str, id: &str, revision| SetNavigationDefault {
        kind: kind.into(),
        target_id: id.into(),
        expected_revision: revision,
    };
    let mut denied = facts.clone();
    denied.actor_mxid = bob.mxid.clone();
    denied.can_manage_policy = false;
    let bob_access = Access {
        actor: bob.mxid.clone(),
        project: access.project.clone(),
        space: access.space.clone(),
        members: access.members.clone(),
        member_names: access.member_names.clone(),
        observed: crate::api::now_ms(),
    };
    assert!(matches!(
        board
            .set_navigation_default(&bob, &bob_access, input("board", &named, 0), &denied)
            .await,
        Err(Error::Unauthorized(_))
    ));
    let mut stale = facts.clone();
    stale.observed_at_ms -= 31_000;
    assert!(matches!(
        board
            .set_navigation_default(&alice, &access, input("board", &named, 0), &stale)
            .await,
        Err(Error::Unavailable(_))
    ));
    assert!(
        board
            .set_navigation_default(
                &alice,
                &access,
                input("board", "not_this_projects_board", 0),
                &facts
            )
            .await
            .is_err()
    );
    board
        .set_navigation_default(&alice, &access, input("board", &named, 0), &facts)
        .await
        .unwrap();
    let catalog = board.catalog(&bob, &bob_access, None).await.unwrap();
    assert_eq!(catalog["boards"][0]["id"], named);
    assert_eq!(catalog["navigationRevision"], 1);
    assert_eq!(
        catalog["boards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["isDefault"] == true)
            .count(),
        1
    );
    board
        .set_navigation_default(&alice, &access, input("room", &room_b, 1), &facts)
        .await
        .unwrap();
    assert!(matches!(
        board
            .set_navigation_default(&alice, &access, input("board", &access.project, 1), &facts)
            .await,
        Err(Error::Conflict("navigation_revision_conflict"))
    ));
    let navigation = board.navigation(&access.project).await.unwrap();
    assert_eq!(navigation["boardId"], named);
    assert_eq!(navigation["roomId"], room_b);
    let same = board
        .reconcile_room_default(&access.project, vec![room_a.clone(), room_b.clone()])
        .await
        .unwrap();
    assert_eq!(same["roomId"], room_b);
    assert_eq!(
        same["revision"], 2,
        "visibility filtering must not change a valid shared default"
    );
    let (_auth2, domain2, alice2, _bob2, access2) = fixture().await;
    let foreign = domain2
        .board
        .catalog(&alice2, &access2, None)
        .await
        .unwrap()["boards"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(matches!(
        board
            .set_navigation_default(&alice, &access, input("board", &foreign, 2), &facts)
            .await,
        Err(Error::NotFound(_))
    ));
    let (left, right) = tokio::join!(
        board.set_navigation_default(&alice, &access, input("board", &access.project, 2), &facts),
        board.set_navigation_default(&alice, &access, input("room", &room_a, 2), &facts)
    );
    assert_eq!(
        usize::from(left.is_ok()) + usize::from(right.is_ok()),
        1,
        "concurrent updates cannot overwrite one another or deadlock"
    );
    assert!(matches!(
        left.as_ref().err().or(right.as_ref().err()),
        Some(Error::Conflict("navigation_revision_conflict"))
    ));
    let reopened = BoardStore::open(&std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let saved = reopened.navigation(&access.project).await.unwrap();
    assert_eq!(saved["revision"], 3);
    let old_room = saved["roomId"].as_str().unwrap();
    let mut db = board.connection().await.unwrap();
    sql_query("UPDATE hagency_agent_v1.rooms SET active=false WHERE project_id=$1 AND room_id=$2")
        .bind::<Text, _>(&access.project)
        .bind::<Text, _>(old_room)
        .execute(&mut db)
        .await
        .unwrap();
    drop(db);
    let repaired = reopened.navigation(&access.project).await.unwrap();
    assert_ne!(
        repaired["roomId"], old_room,
        "inactive defaults fall back to an active registered Room"
    );
    assert_eq!(repaired["revision"], 4);
    let stored = board.navigation(&access.project).await.unwrap();
    assert_eq!(
        stored, repaired,
        "repair is saved once and does not drift on every read"
    );
    assert!(matches!(
        board
            .set_navigation_default(&alice, &access, input("room", old_room, 4), &facts)
            .await,
        Err(Error::NotFound(_))
    ));
    let revision = domain
        .project(&alice, &access.project, crate::api::now_ms())
        .await
        .unwrap()
        .revision;
    domain
        .archive_project(
            &alice,
            &access.project,
            revision,
            true,
            &facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(matches!(
        board
            .set_navigation_default(&alice, &access, input("board", &named, 3), &facts)
            .await,
        Err(Error::Conflict("project_archived"))
    ));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_room_default_repair_handles_zero_unlinked_and_new_children_without_drift() {
    use crate::domain::AdminFacts;
    let (_auth, domain, alice, _bob, access) = fixture().await;
    let board = &domain.board;
    board.catalog(&alice, &access, None).await.unwrap();
    assert!(
        board
            .reconcile_room_default(&access.project, vec![])
            .await
            .unwrap()["roomId"]
            .is_null()
    );
    let facts = AdminFacts {
        actor_mxid: alice.mxid.clone(),
        room_id: access.space.clone(),
        observed_at_ms: crate::api::now_ms(),
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let first = format!("!first_{}:example.test", crate::secret_token());
    let second = format!("!second_{}:example.test", crate::secret_token());
    for id in [&first, &second] {
        let rf = AdminFacts {
            room_id: id.clone(),
            is_space: false,
            linked_space_id: Some(access.space.clone()),
            ..facts.clone()
        };
        domain
            .register_room(
                &alice,
                &access.project,
                id,
                &facts,
                &rf,
                crate::api::now_ms(),
            )
            .await
            .unwrap();
    }
    let initial = board
        .reconcile_room_default(&access.project, vec![first.clone(), second.clone()])
        .await
        .unwrap();
    assert_eq!(initial["roomId"], first);
    let repaired = board
        .reconcile_room_default(&access.project, vec![second.clone()])
        .await
        .unwrap();
    assert_eq!(repaired["roomId"], second);
    assert_eq!(board.navigation(&access.project).await.unwrap(), repaired);
    let empty = board
        .reconcile_room_default(&access.project, vec![])
        .await
        .unwrap();
    assert!(empty["roomId"].is_null());
    assert_eq!(
        board.navigation(&access.project).await.unwrap(),
        empty,
        "other readers must not revive a Room with a removed Space link"
    );
    let new = board
        .reconcile_room_default(&access.project, vec![first.clone()])
        .await
        .unwrap();
    assert_eq!(new["roomId"], first);
    assert_eq!(
        board
            .reconcile_room_default(&access.project, vec![first])
            .await
            .unwrap(),
        new
    );
}
