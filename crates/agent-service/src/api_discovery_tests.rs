use crate::{
    api::{App, now_ms},
    domain::{AdminFacts, CreateBoundAgent, DomainStore, RoomFacts},
    gateway::Gateway,
    identity::{Identity, Verifier},
    secret_token,
    store::{Principal, Store},
};
use salvo::{
    Service,
    http::StatusCode,
    test::{ResponseExt, TestClient},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use url::Url;

fn admin(p: &Principal, room: &str, is_space: bool, parent: Option<&str>) -> AdminFacts {
    AdminFacts {
        actor_mxid: p.mxid.clone(),
        room_id: room.into(),
        observed_at_ms: now_ms(),
        joined: true,
        can_manage_policy: true,
        is_space,
        linked_space_id: parent.map(str::to_owned),
    }
}
async fn user(store: &Store, name: &str) -> (String, Principal) {
    let grant = store
        .sign_in(
            Identity {
                issuer: "https://example.test/_pasion/".into(),
                subject: name.into(),
                mxid: format!("@{name}:example.test"),
                client_id: "discovery-test".into(),
                valid_until_ms: now_ms() + 30000,
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = store
        .authenticate(&grant.token, now_ms(), false)
        .await
        .unwrap();
    let device = store
        .register_device(
            &grant.token,
            crate::store::RegisterDevice {
                installation_id: format!("create-{}", p.user_id),
                name: "Creation device".into(),
            },
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let p = store
        .authenticate(&device.token, crate::api::now_ms(), true)
        .await
        .unwrap();
    (grant.token, p)
}
fn member(mxid: &str) -> Value {
    json!({"type":"m.room.member","state_key":mxid,"content":{"membership":"join"}})
}
async fn get(service: &Service, path: &str, credential: &str, status: StatusCode) -> Value {
    let mut response = TestClient::get(format!("https://example.test{path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(credential)
        .send(service)
        .await;
    assert_eq!(response.status_code, Some(status));
    response.take_json::<Value>().await.unwrap()
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_room_discovery_and_roster_require_live_independent_memberships() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", "https://example.test/_pasion/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (owner_token, owner) = user(&store, &format!("owner_{suffix}")).await;
    let (caller_token, caller) = user(&store, &format!("member_{suffix}")).await;
    let (room_only_token, room_only) = user(&store, &format!("room_only_{suffix}")).await;
    let space = format!("!space_{suffix}:example.test");
    let room = format!("!room_{suffix}:example.test");
    let secret = format!("!secret_{suffix}:example.test");
    let project = domain
        .register_project(&owner, &space, &admin(&owner, &space, true, None), now_ms())
        .await
        .unwrap();
    for r in [&room, &secret] {
        domain
            .register_room(
                &owner,
                &project.id,
                r,
                &admin(&owner, &space, true, None),
                &admin(&owner, r, false, Some(&space)),
                now_ms(),
            )
            .await
            .unwrap();
    }
    let mut facts = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: owner.mxid.clone(),
        room_id: room.clone(),
        space_id: space.clone(),
        observed_at_ms: now_ms(),
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
            &owner,
            CreateBoundAgent {
                project_id: project.id.clone(),
                room_id: room.clone(),
                display_name: "Public Room Agent".into(),
                idempotency_key: "discovery".into(),
            },
            &facts,
            now_ms(),
        )
        .await
        .unwrap();
    facts.puppet_mxid = Some(created.agent.puppet_mxid.clone());
    facts.puppet_in_room = true;
    facts.observed_at_ms = now_ms();
    domain
        .activate_binding(
            &owner,
            &created.binding.id,
            created.binding.generation,
            &facts,
            now_ms(),
        )
        .await
        .unwrap();
    let states = Arc::new(RwLock::new(BTreeMap::from([
        (
            space.clone(),
            vec![
                json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
                member(&owner.mxid),
                member(&caller.mxid),
                json!({"type":"m.room.name","state_key":"","content":{"name":"Research Project"}}),
                json!({"type":"m.room.topic","state_key":"","content":{"topic":"Space topic"}}),
                json!({"type":"m.space.child","state_key":room,"content":{"via":["example.test"]}}),
                json!({"type":"m.space.child","state_key":secret,"content":{"via":["example.test"]}}),
            ],
        ),
        (
            room.clone(),
            vec![
                member(&owner.mxid),
                member(&caller.mxid),
                member(&room_only.mxid),
                member(&created.agent.puppet_mxid),
                json!({"type":"m.room.name","state_key":"","content":{"name":"Discussion"}}),
            ],
        ),
        (secret.clone(), vec![member(&owner.mxid)]),
    ])));
    let reader = states.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let state = reader.read().unwrap().get(&room).cloned();
            Box::pin(async move { Ok(state.unwrap_or_default()) })
        }),
        "@_hagency_service:example.test".into(),
    );
    let issuer = Url::parse("https://example.test/_pasion/").unwrap();
    let origin = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("introspect").unwrap(),
        origin.clone(),
        "unused-test-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let app = App::new(
        store.clone(),
        verifier,
        origin,
        issuer,
        "example.test".into(),
    )
    .with_domain(domain.clone(), gateway);
    let service = Service::new(app.router());
    let reply_policy = format!(
        "https://example.test/api/hagency/v1/bindings/{}/reply-policy",
        created.binding.id
    );
    assert!(
        !domain
            .binding(&owner, &created.binding.id, now_ms())
            .await
            .unwrap()
            .thread_auto_reply
    );
    for invalid in [
        json!({}),
        json!({"threadAutoReply":"true"}),
        json!({"threadAutoReply":true,"extra":1}),
    ] {
        let response = TestClient::put(&reply_policy)
            .add_header("host", "example.test", true)
            .bearer_auth(&owner_token)
            .json(&invalid)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
    }
    let response = TestClient::put(&reply_policy)
        .add_header("host", "example.test", true)
        .bearer_auth(&caller_token)
        .json(&json!({"threadAutoReply":true}))
        .send(&service)
        .await;
    assert_ne!(response.status_code, Some(StatusCode::OK));
    for enabled in [true, false] {
        let mut response = TestClient::put(&reply_policy)
            .add_header("host", "example.test", true)
            .bearer_auth(&owner_token)
            .json(&json!({"threadAutoReply":enabled}))
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response.take_json::<Value>().await.unwrap()["binding"]["threadAutoReply"],
            enabled
        );
        assert_eq!(
            domain
                .binding(&owner, &created.binding.id, now_ms())
                .await
                .unwrap()
                .thread_auto_reply,
            enabled
        );
    }

    let list = format!("/api/hagency/v1/projects/{}/rooms", project.id);
    let roster = format!(
        "{list}/{}/agents",
        percent_encoding::utf8_percent_encode(&room, percent_encoding::NON_ALPHANUMERIC)
    );
    // Being a member/Agent owner is not a policy-administrator grant. UI hints
    // follow the same actual Matrix thresholds as the write endpoints.
    let project_state = format!("/api/hagency/v1/projects/{}/service-state", project.id);
    let room_state = format!(
        "{list}/{}/service-state",
        percent_encoding::utf8_percent_encode(&room, percent_encoding::NON_ALPHANUMERIC)
    );
    assert_eq!(
        get(&service, &project_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        false
    );
    assert_eq!(
        get(&service, &room_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        false
    );
    let policy_path = format!(
        "https://example.test/api/hagency/v1/projects/{}/creation-policy",
        project.id
    );
    let denied = TestClient::put(&policy_path).add_header("host","example.test",true).bearer_auth(&caller_token)
        .json(&json!({"expectedRevision":project.revision,"policy":{"defaultAllow":false,"allow":[],"deny":[]}})).send(&service).await;
    assert_eq!(denied.status_code, Some(StatusCode::UNAUTHORIZED));
    assert_eq!(
        domain
            .project(&caller, &project.id, now_ms())
            .await
            .unwrap()
            .revision,
        project.revision
    );
    let power = json!({"type":"m.room.power_levels","state_key":"","content":{"users":{caller.mxid.clone():100},"state_default":50}});
    {
        let mut states = states.write().unwrap();
        states.get_mut(&space).unwrap().push(power.clone());
        states.get_mut(&room).unwrap().push(power);
    }
    assert_eq!(
        get(&service, &project_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        true
    );
    assert_eq!(
        get(&service, &room_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        true
    );
    // Independent Room power revocation does not revoke Space administration.
    states
        .write()
        .unwrap()
        .get_mut(&room)
        .unwrap()
        .retain(|e| e["type"] != "m.room.power_levels");
    assert_eq!(
        get(&service, &room_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        false
    );
    assert_eq!(
        get(&service, &project_state, &caller_token, StatusCode::OK).await["canManagePolicy"],
        true
    );
    let visible = get(&service, &list, &caller_token, StatusCode::OK).await;
    assert_eq!(visible["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(visible["rooms"][0]["roomId"], room);
    assert_eq!(visible["rooms"][0]["name"], "Discussion");
    assert!(visible["rooms"][0]["topic"].is_null());
    let projects = get(
        &service,
        "/api/hagency/v1/projects",
        &caller_token,
        StatusCode::OK,
    )
    .await;
    let project_view = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == project.id)
        .unwrap();
    assert_eq!(project_view["name"], "Research Project");
    assert_eq!(project_view["topic"], "Space topic");
    states
        .write()
        .unwrap()
        .get_mut(&space)
        .unwrap()
        .iter_mut()
        .find(|event| event["type"] == "m.room.name")
        .unwrap()["content"]["name"] = json!("Renamed Project");
    let renamed = get(
        &service,
        "/api/hagency/v1/projects",
        &caller_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        renamed["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == project.id)
            .unwrap()["name"],
        "Renamed Project"
    );
    assert!(
        get(
            &service,
            "/api/hagency/v1/projects",
            &room_only_token,
            StatusCode::OK
        )
        .await["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["id"] != project.id)
    );

    let roster_result = get(&service, &roster, &caller_token, StatusCode::OK).await;
    assert_eq!(roster_result["agents"][0]["agentId"], created.agent.id);
    assert_eq!(roster_result["agents"][0]["ownerMxid"], owner.mxid);
    assert!(roster_result["agents"][0].get("ownerUserId").is_none());
    let secret_roster = format!(
        "{list}/{}/agents",
        percent_encoding::utf8_percent_encode(&secret, percent_encoding::NON_ALPHANUMERIC)
    );
    get(
        &service,
        &secret_roster,
        &caller_token,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    get(&service, &list, &room_only_token, StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        get(&service, &roster, &room_only_token, StatusCode::OK).await["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Leaving the child Room is effective on the next request, without waiting
    // for a background membership cache or depending on Space membership.
    states
        .write()
        .unwrap()
        .get_mut(&room)
        .unwrap()
        .retain(|event| event["state_key"] != caller.mxid);
    assert!(
        get(&service, &list, &caller_token, StatusCode::OK).await["rooms"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    get(&service, &roster, &caller_token, StatusCode::UNAUTHORIZED).await;
    store.sign_out(&room_only_token).await.unwrap();
    get(
        &service,
        &roster,
        &room_only_token,
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

// Test-only network fixture. Abort even when an assertion unwinds: a failed
// readiness test must not leave an independently running homeserver/probe task.
struct AbortFixture(tokio::task::JoinHandle<()>);
impl Drop for AbortFixture {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn readiness_matrix_fixture(
    probe_room: String,
    probe_event: String,
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> (Url, tokio::sync::oneshot::Receiver<()>, AbortFixture) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let (sent, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut sent = Some(sent);
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0u8; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&chunk[..n]);
                assert!(bytes.len() <= 65536);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(end + 4);
                }
            };
            let Some(header_end) = header_end else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let line = headers.lines().next().unwrap().to_owned();
            let length = headers
                .lines()
                .find_map(|h| {
                    h.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length <= 65536);
            while bytes.len() < header_end + length {
                let mut chunk = [0u8; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result = if line.starts_with("GET /_matrix/client/v3/account/whoami?") {
                json!({"user_id":"@_hagency_service:example.test"})
            } else if line.starts_with("POST /_matrix/client/v3/createRoom?") {
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(body["preset"], "private_chat");
                json!({"room_id":probe_room})
            } else if line.starts_with("PUT /_matrix/client/v3/rooms/")
                && line.contains("/send/m.room.message/")
            {
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(body["msgtype"], "m.notice");
                assert_eq!(body["body"], "Hagency service readiness probe");
                if let Some(sent) = sent.take() {
                    let _ = sent.send(());
                }
                json!({"event_id":probe_event})
            } else {
                panic!("unexpected Matrix operation in readiness fixture");
            };
            let body = result.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (origin, received, AbortFixture(task))
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_startup_readiness_guard_blocks_create_bind_before_any_side_effect() {
    use diesel_async::{AsyncConnection, AsyncPgConnection, SimpleAsyncConnection};
    // The production readiness-room singleton and inbox are global scans, so
    // give this test its own additional disposable database.
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!("hagency_ready_{}", &crate::hash(&secret_token())[..16]);
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let outcome = tokio::spawn(async move { readiness_guard_exercise(url.as_str()).await }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    outcome.unwrap();
}

async fn readiness_guard_exercise(url: &str) {
    use crate::{appservice::Inbox, matrix_client::MatrixClient};
    use diesel::{sql_query, sql_types::BigInt};
    use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let store = Store::open(url, "example.test", "https://example.test/_pasion/")
        .await
        .unwrap();
    let domain = DomainStore::open(url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (credential, owner) = user(&store, "readiness_owner").await;
    let (foreign_credential, _) = user(&store, "readiness_foreign_owner").await;
    let space = "!readiness_space:example.test";
    let first = "!readiness_first:example.test";
    let second = "!readiness_second:example.test";
    let project = domain
        .register_project(&owner, space, &admin(&owner, space, true, None), now_ms())
        .await
        .unwrap();
    for room in [first, second] {
        domain
            .register_room(
                &owner,
                &project.id,
                room,
                &admin(&owner, space, true, None),
                &admin(&owner, room, false, Some(space)),
                now_ms(),
            )
            .await
            .unwrap();
    }
    let mut f = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: owner.mxid.clone(),
        room_id: first.into(),
        space_id: space.into(),
        observed_at_ms: now_ms(),
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: None,
        puppet_in_room: false,
        encrypted: false,
    };
    let existing = domain
        .create_bound_agent(
            &owner,
            CreateBoundAgent {
                project_id: project.id.clone(),
                room_id: first.into(),
                display_name: "existing valid bind target".into(),
                idempotency_key: "fixture-existing".into(),
            },
            &f,
            now_ms(),
        )
        .await
        .unwrap();
    f.puppet_mxid = Some(existing.agent.puppet_mxid.clone());
    f.puppet_in_room = true;
    domain
        .activate_binding(
            &owner,
            &existing.binding.id,
            existing.binding.generation,
            &f,
            now_ms(),
        )
        .await
        .unwrap();
    let device = store
        .register_device(
            &credential,
            crate::store::RegisterDevice {
                installation_id: "http-instance-readiness".into(),
                name: "HTTP owner device".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let gateway_calls = Arc::new(AtomicUsize::new(0));
    let counter = gateway_calls.clone();
    let owner_mxid = owner.mxid.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            counter.fetch_add(1, Ordering::SeqCst);
            let owner_mxid = owner_mxid.clone();
            Box::pin(async move {
                let state = if room == space {
                    vec![
                        member(&owner_mxid),
                        json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
                        json!({"type":"m.space.child","state_key":first,"content":{"via":["example.test"]}}),
                        json!({"type":"m.space.child","state_key":second,"content":{"via":["example.test"]}}),
                    ]
                } else {
                    vec![
                        member(&owner_mxid),
                        member("@_hagency_service:example.test"),
                        json!({"type":"m.room.create","state_key":"","content":{}}),
                    ]
                };
                Ok(state)
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let matrix_calls = Arc::new(AtomicUsize::new(0));
    let probe_room = "!readiness_probe:example.test";
    let probe_event = "$readiness_guard_probe";
    let (matrix_origin, sent, _matrix_task) =
        readiness_matrix_fixture(probe_room.into(), probe_event.into(), matrix_calls.clone()).await;
    let issuer = Url::parse("https://example.test/_pasion/").unwrap();
    let public = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("introspect").unwrap(),
        public.clone(),
        "unused-test-verifier".into(),
        "example.test".into(),
    )
    .unwrap();
    // Exactly the production builder path: binding Matrix client resets readiness.
    let app = App::new(store, verifier, public, issuer, "example.test".into())
        .with_domain(domain.clone(), gateway)
        .with_matrix_client(
            MatrixClient::new(
                matrix_origin,
                "fixture-as-token-only-01234567890123456789".into(),
                "example.test".into(),
            )
            .unwrap(),
        );
    assert!(!app.startup_ready());
    let inbox = Inbox::open(url, "fixture-hs-token-readiness-only-0123456789".into())
        .await
        .unwrap();
    let service = Service::new(
        salvo::Router::new()
            .push(app.router())
            .push(app.readiness_router())
            .push(inbox.router()),
    );
    let absent = get(
        &service,
        "/api/hagency/v1/commands/agent.create/ready-create",
        &credential,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(absent["code"], "command_not_found");
    let create =
        json!({"displayName":"new accepted only after readiness","idempotencyKey":"ready-create"});
    let bind = json!({"projectId":project.id,"roomId":second,"idempotencyKey":"ready-bind"});
    let bind_path = format!("/api/hagency/v1/agents/{}/bindings", existing.agent.id);
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    #[derive(diesel::QueryableByName, Debug, PartialEq)]
    struct Counts {
        #[diesel(sql_type=BigInt)]
        agents: i64,
        #[diesel(sql_type=BigInt)]
        bindings: i64,
        #[diesel(sql_type=BigInt)]
        commands: i64,
        #[diesel(sql_type=BigInt)]
        audit: i64,
    }
    let query = "SELECT (SELECT count(*) FROM hagency_agent_v1.agents) AS agents,(SELECT count(*) FROM hagency_agent_v1.bindings) AS bindings,(SELECT count(*) FROM hagency_agent_v1.domain_commands) AS commands,(SELECT count(*) FROM hagency_agent_v1.domain_audit) AS audit";
    let before = sql_query(query)
        .get_result::<Counts>(&mut db)
        .await
        .unwrap();
    assert_eq!(before.agents, 1);
    assert_eq!(before.bindings, 1);
    assert_eq!(before.commands, 2);
    for (path, body) in [
        ("/api/hagency/v1/agents", &create),
        (bind_path.as_str(), &bind),
    ] {
        let mut response = TestClient::post(format!("https://example.test{path}"))
            .add_header("host", "example.test", true)
            .bearer_auth(if path == "/api/hagency/v1/agents" {
                &device.token
            } else {
                &credential
            })
            .json(body)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(
            response.take_json::<Value>().await.unwrap()["code"],
            "appservice_startup_not_ready"
        );
    }
    let response = TestClient::post("https://example.test/api/hagency/v1/agents")
        .add_header("host", "example.test", true)
        .bearer_auth(&credential)
        .json(&create)
        .send(&service)
        .await;
    assert_eq!(
        response.status_code,
        Some(StatusCode::UNAUTHORIZED),
        "a user session cannot select an execution device for creation"
    );
    let response = TestClient::post("https://example.test/api/hagency/v1/agents")
        .add_header("host", "example.test", true)
        .bearer_auth("not-a-real-session")
        .json(&create)
        .send(&service)
        .await;
    assert_eq!(
        response.status_code,
        Some(StatusCode::UNAUTHORIZED),
        "503 must not substitute for actual principal authentication"
    );
    assert_eq!(
        sql_query(query)
            .get_result::<Counts>(&mut db)
            .await
            .unwrap(),
        before
    );
    assert_eq!(gateway_calls.as_ref().load(Ordering::SeqCst), 0);
    assert_eq!(matrix_calls.as_ref().load(Ordering::SeqCst), 0);
    assert!(
        domain
            .command_status(&owner, "agent.create", "ready-create", now_ms())
            .await
            .is_err()
    );
    assert!(
        domain
            .command_status(&owner, "agent.bind", "ready-bind", now_ms())
            .await
            .is_err()
    );
    let existing_after = domain
        .agent(&owner, &existing.agent.id, now_ms())
        .await
        .unwrap();
    assert_eq!(existing_after.state, "active");
    assert_eq!(existing_after.generation, existing.agent.generation);
    let state = get(
        &service,
        "/api/hagency/v1/readiness",
        &credential,
        StatusCode::OK,
    )
    .await;
    assert_eq!(state["startupRoundtripConfirmed"], false);
    let mut ready_response = TestClient::get("https://example.test/readyz")
        .send(&service)
        .await;
    assert_eq!(
        ready_response.status_code,
        Some(StatusCode::SERVICE_UNAVAILABLE)
    );
    assert_eq!(
        ready_response.take_json::<Value>().await.unwrap()["startupRoundtripConfirmed"],
        false
    );
    // Production worker must still wait after successful Matrix send until its
    // exact event has reached the authenticated, durable AS transaction route.
    let _probe = AbortFixture(app.start_matrix_readiness(inbox).unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(8), sent)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !app.startup_ready(),
        "Matrix send alone must not establish AS readiness"
    );
    let mut as_response=TestClient::put("https://example.test/_matrix/app/v1/transactions/readiness_guard_txn").bearer_auth("fixture-hs-token-readiness-only-0123456789").json(&json!({"events":[{"event_id":probe_event,"room_id":probe_room,"sender":"@_hagency_service:example.test","type":"m.room.message","content":{"msgtype":"m.notice","body":"Hagency service readiness probe"}}]})).send(&service).await;
    assert_eq!(as_response.status_code, Some(StatusCode::OK));
    assert_eq!(as_response.take_json::<Value>().await.unwrap(), json!({}));
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !app.startup_ready() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let calls_after_probe = matrix_calls.as_ref().load(Ordering::SeqCst);
    assert!(calls_after_probe >= 3);
    for (path, body) in [
        ("/api/hagency/v1/agents", &create),
        (bind_path.as_str(), &bind),
    ] {
        let mut response = TestClient::post(format!("https://example.test{path}"))
            .add_header("host", "example.test", true)
            .bearer_auth(if path == "/api/hagency/v1/agents" {
                &device.token
            } else {
                &credential
            })
            .json(body)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::ACCEPTED));
        let result = response.take_json::<Value>().await.unwrap();
        assert_eq!(result["commandState"], "pending");
        if path == "/api/hagency/v1/agents" {
            assert!(result["creation"].get("binding").is_none());
            assert_eq!(result["creation"]["agent"]["state"], "creating");
        } else {
            assert_eq!(result["creation"]["binding"]["state"], "joining");
        }
        assert_eq!(result["creation"]["agent"]["ownerUserId"], owner.user_id);
    }
    let known = get(
        &service,
        "/api/hagency/v1/commands/agent.create/ready-create",
        &credential,
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(known["creation"]["agent"]["ownerUserId"], owner.user_id);
    assert!(known["creation"].get("binding").is_none());
    let foreign = get(
        &service,
        "/api/hagency/v1/commands/agent.create/ready-create",
        &foreign_credential,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(
        foreign["code"], "command_not_found",
        "foreign command existence must be indistinguishable from missing own key"
    );
    let devices = get(
        &service,
        "/api/hagency/v1/devices",
        &credential,
        StatusCode::OK,
    )
    .await;
    assert!(
        devices["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == device.device_id)
    );
    assert!(devices["devices"][0].get("token").is_none());
    let _transport =
        crate::transport::TransportStore::open(url, crate::transport::Limits::default())
            .await
            .unwrap();
    let device_path = format!(
        "/api/hagency/v1/agents/{}/execution-device",
        existing.agent.id
    );
    let response = TestClient::put(format!("https://example.test{device_path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(&credential)
        .json(&json!({"expectedGeneration":existing.agent.generation}))
        .send(&service)
        .await;
    assert_eq!(
        response.status_code,
        Some(StatusCode::UNAUTHORIZED),
        "assignment requires the current device credential"
    );
    let mut response = TestClient::put(format!("https://example.test{device_path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(&device.token)
        .json(&json!({"expectedGeneration":existing.agent.generation}))
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let assigned = response.take_json::<Value>().await.unwrap()["agent"].clone();
    assert_eq!(assigned["executionDeviceId"], device.device_id);
    let response = TestClient::put(format!("https://example.test{device_path}"))
        .add_header("host", "example.test", true)
        .bearer_auth(&device.token)
        .json(&json!({"expectedGeneration":existing.agent.generation}))
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::CONFLICT));
    get(
        &service,
        &format!(
            "/api/hagency/v1/agents/{}/execution-instance",
            existing.agent.id
        ),
        &credential,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let direct_path = format!("/api/hagency/v1/agents/{}/owner-direct", existing.agent.id);
    let direct = get(&service, &direct_path, &credential, StatusCode::OK).await;
    assert!(direct["ownerDirectRoomId"].is_null() && direct["binding"].is_null());
    let after = sql_query(query)
        .get_result::<Counts>(&mut db)
        .await
        .unwrap();
    assert_eq!(after.agents, before.agents + 1);
    assert_eq!(after.bindings, before.bindings + 1);
    assert_eq!(after.commands, before.commands + 2);
    assert_eq!(
        matrix_calls.as_ref().load(Ordering::SeqCst),
        calls_after_probe,
        "HTTP commits durable desired state; it does not provision synchronously"
    );
    let response = TestClient::get("https://example.test/readyz")
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_public_discovery_identifies_hagency_without_granting_authority() {
    let db = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&db, "example.test", "https://example.test/_pasion/")
        .await
        .unwrap();
    let issuer = Url::parse("https://example.test/_pasion/").unwrap();
    let public = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("oauth2/introspect").unwrap(),
        public.clone(),
        "unused-test-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let service =
        Service::new(App::new(store, verifier, public, issuer, "example.test".into()).router());
    let mut reply = TestClient::get("https://example.test/api/hagency/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&service)
        .await;
    assert_eq!(reply.status_code, Some(StatusCode::OK));
    assert_eq!(reply.headers()["cache-control"], "no-store");
    assert!(!reply.headers().contains_key("set-cookie"));
    let value = reply.take_json::<Value>().await.unwrap();
    assert_eq!(value["product"], "hagency-server");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["protocolVersion"], 3);
    assert_eq!(value["homeserver"], "https://example.test/");
    assert_eq!(value["issuer"], "https://example.test/_pasion/");
    assert_eq!(
        value["capabilities"],
        json!([
            "pasion-oauth",
            "owner-agent-appservice-v1",
            "projects-matrix-spaces-v1",
            "device-execution-v1",
            "execution-history-v1",
            "global-agent-identity-v2",
            "execution-device-v1",
            "owner-direct-v1",
            "processing-reaction-v1"
        ])
    );
    assert!(value.get("token").is_none());
    let reply = TestClient::get("https://example.test/api/hagency/v1/projects")
        .add_header("host", "example.test", true)
        .send(&service)
        .await;
    assert_eq!(reply.status_code, Some(StatusCode::UNAUTHORIZED));
    let reply = TestClient::get("https://other.test/api/hagency/v1/discovery")
        .add_header("host", "other.test", true)
        .send(&service)
        .await;
    assert_eq!(reply.status_code, Some(StatusCode::UNAUTHORIZED));
}
