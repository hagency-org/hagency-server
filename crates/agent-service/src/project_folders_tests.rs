//! Space project folders over the real HTTP routes: adoption stores one folder,
//! and a hosting computer looks it up per Room for the Agents it owns there.
use crate::{
    api::{App, now_ms},
    domain::{AdminFacts, CreateBoundAgent, DomainStore, RoomFacts},
    gateway::Gateway,
    identity::{Identity, Verifier},
    secret_token,
    store::{Principal, RegisterDevice, Store},
};
use diesel::{
    QueryableByName, sql_query,
    sql_types::{BigInt, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
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

type States = Arc<RwLock<BTreeMap<String, Vec<Value>>>>;

struct Person {
    session: String,
    device: String,
    p: Principal,
}

async fn person(store: &Store, name: &str) -> Person {
    let suffix = secret_token();
    let session = store
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: format!("{name}-{suffix}"),
                mxid: format!("@{name}_{suffix}:example.test"),
                client_id: "folder-test".into(),
                valid_until_ms: now_ms() + 600_000,
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let device = store
        .register_device(
            &session,
            RegisterDevice {
                installation_id: format!("{name}-host"),
                name: "Hosting computer".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let p = store.authenticate(&device, now_ms(), true).await.unwrap();
    Person { session, device, p }
}

async fn open() -> (String, Store, DomainStore) {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let store = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    (url, store, domain)
}

fn service(store: &Store, domain: &DomainStore, states: &States) -> Service {
    let reader = states.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let state = reader.read().unwrap().get(&room).cloned();
            Box::pin(async move { Ok(state.unwrap_or_default()) })
        }),
        "@_hagency_service:example.test".into(),
    );
    let issuer = Url::parse("https://example.test/auth/").unwrap();
    let origin = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("introspect").unwrap(),
        origin.clone(),
        "unused-test-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    Service::new(
        App::new(
            store.clone(),
            verifier,
            origin,
            issuer,
            "example.test".into(),
        )
        .with_domain(domain.clone(), gateway)
        .router(),
    )
}

/// A Space whose administrator is `admin`, with these child Rooms.
fn space(states: &States, space: &str, admin: &Principal, rooms: &[&str]) {
    let mut events = vec![
        json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
        json!({"type":"m.room.member","state_key":admin.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.power_levels","state_key":"","content":{"users":{admin.mxid.clone():100}}}),
    ];
    for room in rooms {
        events.push(
            json!({"type":"m.space.child","state_key":room,"content":{"via":["example.test"]}}),
        );
    }
    states.write().unwrap().insert(space.into(), events);
}

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

fn facts(p: &Principal, space: &str, room: &str, puppet: Option<&str>) -> RoomFacts {
    RoomFacts {
        owner_direct_valid: false,
        owner_mxid: p.mxid.clone(),
        room_id: room.into(),
        space_id: space.into(),
        observed_at_ms: now_ms(),
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: puppet.map(str::to_owned),
        puppet_in_room: puppet.is_some(),
        encrypted: false,
    }
}

async fn adopt(service: &Service, token: &str, body: Value) -> (StatusCode, Value) {
    let mut response = TestClient::post("https://example.test/hagency/api/v1/projects/adopt")
        .add_header("host", "example.test", true)
        .bearer_auth(token)
        .json(&body)
        .send(service)
        .await;
    let status = response.status_code.unwrap();
    (status, response.take_json::<Value>().await.unwrap())
}

async fn lookup(service: &Service, token: &str, query: &str) -> (StatusCode, Value) {
    let mut response = TestClient::get(format!(
        "https://example.test/hagency/api/v1/room-project-folder{query}"
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(token)
    .send(service)
    .await;
    let status = response.status_code.unwrap();
    (status, response.take_json::<Value>().await.unwrap())
}

fn for_room(room: &str) -> String {
    format!(
        "?roomId={}",
        percent_encoding::utf8_percent_encode(room, percent_encoding::NON_ALPHANUMERIC)
    )
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    count: i64,
}

async fn count(url: &str, query: &str, id: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(url).await.unwrap();
    sql_query(query)
        .bind::<Text, _>(id)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}

const FOLDERS: &str = "SELECT count(*) AS count FROM hagency_agent_v1.project_folders f JOIN hagency_agent_v1.projects p ON p.id=f.project_id WHERE p.space_id=$1";
const PROJECTS: &str = "SELECT count(*) AS count FROM hagency_agent_v1.projects WHERE space_id=$1";
const FOLDER_AUDIT: &str = "SELECT count(*) AS count FROM hagency_agent_v1.domain_audit WHERE operation='project.folder' AND object_id=$1";

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_space_adoption_keeps_one_project_folder() {
    let (url, store, domain) = open().await;
    let creator = person(&store, "folder_creator").await;
    let stranger = person(&store, "folder_stranger").await;
    let suffix = secret_token();
    let first = format!("!folder_first_{suffix}:example.test");
    let fresh = format!("!folder_fresh_{suffix}:example.test");
    let wide = format!("!folder_wide_{suffix}:example.test");
    let states = States::default();
    for id in [&first, &fresh, &wide] {
        space(&states, id, &creator.p, &[]);
    }
    let service = service(&store, &domain, &states);

    // Without the field the reply keeps exactly its original shape.
    let (status, plain) = adopt(&service, &creator.session, json!({"spaceId":first})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        plain.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["project"]
    );
    let project = plain["project"]["id"].as_str().unwrap().to_owned();
    assert_eq!(count(&url, FOLDERS, &first).await, 0);
    let (status, null) = adopt(
        &service,
        &creator.session,
        json!({"spaceId":first,"projectFolder":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(null, plain, "an explicit null is the field left out");

    // The first folder is kept; the same name again is accepted.
    for _ in 0..2 {
        let (status, stored) = adopt(
            &service,
            &creator.session,
            json!({"spaceId":first,"projectFolder":"Shared work"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            stored.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["project", "projectFolder"]
        );
        assert_eq!(stored["project"]["id"], project);
        assert_eq!(stored["projectFolder"], "Shared work");
    }
    // A different name is a conflict and changes nothing.
    let (status, conflict) = adopt(
        &service,
        &creator.session,
        json!({"spaceId":first,"projectFolder":"Other work"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict, json!({"code":"project_folder_conflict"}));
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    #[derive(QueryableByName)]
    struct Stored {
        #[diesel(sql_type=Text)]
        folder: String,
        #[diesel(sql_type=Text)]
        set_by_user_id: String,
    }
    let stored = sql_query(
        "SELECT folder,set_by_user_id FROM hagency_agent_v1.project_folders WHERE project_id=$1",
    )
    .bind::<Text, _>(&project)
    .get_result::<Stored>(&mut db)
    .await
    .unwrap();
    assert_eq!(stored.folder, "Shared work");
    assert_eq!(stored.set_by_user_id, creator.p.user_id);
    assert_eq!(
        count(&url, FOLDER_AUDIT, &project).await,
        1,
        "only storing the folder is audited"
    );
    // Without the field a later adoption leaves the stored folder alone.
    let (status, again) = adopt(&service, &creator.session, json!({"spaceId":first})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, plain);
    assert_eq!(count(&url, FOLDERS, &first).await, 1);

    // Invalid names are refused before anything is registered.
    let long = "a".repeat(65);
    for invalid in [
        "",
        ".git",
        "a/b",
        "a\\b",
        "a:b",
        " lead",
        "trail ",
        "nul\u{0}",
        "tab\there",
        long.as_str(),
    ] {
        let (status, refused) = adopt(
            &service,
            &creator.session,
            json!({"spaceId":fresh,"projectFolder":invalid}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid:?}");
        assert_eq!(refused, json!({"code":"invalid_project_folder"}));
    }
    for malformed in [
        json!({"spaceId":fresh,"projectFolder":7}),
        json!({"spaceId":fresh,"projectFolder":["Shared"]}),
        json!({"spaceId":fresh,"projectFolder":"Shared","folder":"Shared"}),
    ] {
        let (status, refused) = adopt(&service, &creator.session, malformed).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(refused, json!({"code":"invalid_arguments"}));
    }
    // Someone who cannot administer the Space cannot name its folder.
    let (status, _) = adopt(
        &service,
        &stranger.session,
        json!({"spaceId":fresh,"projectFolder":"Mine"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(count(&url, PROJECTS, &fresh).await, 0);
    assert_eq!(count(&url, FOLDERS, &fresh).await, 0);

    // The 64-character limit counts characters, here and in the table check.
    let name = "项".repeat(64);
    let (status, stored) = adopt(
        &service,
        &creator.session,
        json!({"spaceId":wide,"projectFolder":name}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["projectFolder"], name);
    assert_eq!(count(&url, FOLDERS, &wide).await, 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_room_project_folder_reaches_hosts_of_agents_in_the_room_only() {
    let (url, store, domain) = open().await;
    let owner = person(&store, "folder_owner").await;
    let stranger = person(&store, "folder_outsider").await;
    let suffix = secret_token();
    let shared = format!("!folder_space_{suffix}:example.test");
    let plain = format!("!plain_space_{suffix}:example.test");
    let room = format!("!folder_room_{suffix}:example.test");
    let empty = format!("!folder_empty_{suffix}:example.test");
    let unfoldered = format!("!plain_room_{suffix}:example.test");
    let states = States::default();
    space(&states, &shared, &owner.p, &[&room, &empty]);
    space(&states, &plain, &owner.p, &[&unfoldered]);
    let service = service(&store, &domain, &states);
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&service)
        .await;
    assert!(
        response.take_json::<Value>().await.unwrap()["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("space-project-folder-v1"))
    );

    let (status, adopted) = adopt(
        &service,
        &owner.session,
        json!({"spaceId":shared,"projectFolder":"Team work"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project = adopted["project"]["id"].as_str().unwrap().to_owned();
    let (status, other) = adopt(&service, &owner.session, json!({"spaceId":plain})).await;
    assert_eq!(status, StatusCode::OK);
    let other = other["project"]["id"].as_str().unwrap().to_owned();
    for (id, space_id, project_id) in [
        (&room, &shared, &project),
        (&empty, &shared, &project),
        (&unfoldered, &plain, &other),
    ] {
        domain
            .register_room(
                &owner.p,
                project_id,
                id,
                &admin(&owner.p, space_id, true, None),
                &admin(&owner.p, id, false, Some(space_id)),
                now_ms(),
            )
            .await
            .unwrap();
    }
    let bind = |id: &str, space_id: &str, key: &str| CreateBoundAgent {
        project_id: if space_id == shared {
            project.clone()
        } else {
            other.clone()
        },
        room_id: id.into(),
        display_name: "Coding teammate".into(),
        idempotency_key: key.into(),
    };
    let created = domain
        .create_bound_agent(
            &owner.p,
            bind(&room, &shared, "folder-room"),
            &facts(&owner.p, &shared, &room, None),
            now_ms(),
        )
        .await
        .unwrap();
    domain
        .create_bound_agent(
            &owner.p,
            bind(&unfoldered, &plain, "plain-room"),
            &facts(&owner.p, &plain, &unfoldered, None),
            now_ms(),
        )
        .await
        .unwrap();
    let visible = json!({"projectFolder":{"projectId":project,"folder":"Team work"}});
    let hidden = json!({"projectFolder":null});
    let expect = async |token: &str, room: &str, expected: &Value| {
        let (status, body) = lookup(&service, token, &for_room(room)).await;
        assert_eq!(status, StatusCode::OK, "{room}");
        assert_eq!(&body, expected, "{room}");
    };

    // A joining binding already counts, for the owner's session and device alike.
    expect(&owner.session, &room, &visible).await;
    let puppet = created.agent.puppet_mxid.clone();
    let active = domain
        .activate_binding(
            &owner.p,
            &created.binding.id,
            created.binding.generation,
            &facts(&owner.p, &shared, &room, Some(&puppet)),
            now_ms(),
        )
        .await
        .unwrap();
    for token in [&owner.session, &owner.device] {
        expect(token, &room, &visible).await;
    }
    // Every other case gives the same empty answer.
    for token in [&stranger.session, &stranger.device] {
        expect(token, &room, &hidden).await;
    }
    expect(&owner.device, &empty, &hidden).await;
    expect(&owner.device, &unfoldered, &hidden).await;
    expect(&owner.device, "!unknown_room:example.test", &hidden).await;

    // Only a well-formed single roomId is accepted.
    let encoded = for_room(&room);
    for query in [
        String::new(),
        "?roomId=".into(),
        "?roomId=not-a-room".into(),
        "?roomId=%40user%3Aexample.test".into(),
        "?roomId=%21spaced%20room%3Aexample.test".into(),
        format!("{encoded}&roomId=%21other%3Aexample.test"),
        format!("{encoded}&extra=1"),
        "?room=%21x%3Aexample.test".into(),
    ] {
        let (status, body) = lookup(&service, &owner.device, &query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(body, json!({"code":"invalid_room_id"}));
    }
    let (status, _) = lookup(&service, "not-a-credential", &encoded).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let response = TestClient::get(format!(
        "https://example.test/hagency/api/v1/room-project-folder{encoded}"
    ))
    .add_header("host", "example.test", true)
    .send(&service)
    .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));

    // An inactive Room or project hides the folder until it is active again.
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    for (switch, id) in [
        (
            "UPDATE hagency_agent_v1.rooms SET active=$2 WHERE room_id=$1",
            &room,
        ),
        (
            "UPDATE hagency_agent_v1.projects SET active=$2 WHERE id=$1",
            &project,
        ),
    ] {
        for active in [false, true] {
            sql_query(switch)
                .bind::<Text, _>(id)
                .bind::<diesel::sql_types::Bool, _>(active)
                .execute(&mut db)
                .await
                .unwrap();
            expect(
                &owner.device,
                &room,
                if active { &visible } else { &hidden },
            )
            .await;
        }
    }

    // A paused binding still counts; a leaving or left one does not.
    let suspended = domain
        .suspend_binding(&owner.p, &active.id, now_ms())
        .await
        .unwrap();
    assert_eq!(suspended.state, "suspended");
    expect(&owner.device, &room, &visible).await;
    let leaving = domain
        .leave_binding(&owner.p, &active.id, now_ms())
        .await
        .unwrap();
    expect(&owner.device, &room, &hidden).await;
    let left = domain
        .confirm_left(
            &owner.p,
            &leaving.id,
            leaving.generation,
            &RoomFacts {
                puppet_in_room: false,
                ..facts(&owner.p, &shared, &room, Some(&puppet))
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(left.state, "left");
    expect(&owner.device, &room, &hidden).await;

    // A revoked device credential no longer authenticates.
    store
        .revoke_device(
            &owner.session,
            owner.p.device_id.as_deref().unwrap(),
            now_ms(),
        )
        .await
        .unwrap();
    let (status, _) = lookup(&service, &owner.device, &encoded).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
