//! Real PostgreSQL and HTTP contract tests; no test input supplies Matrix facts.
#[path = "management_setup_service_tests.rs"]
mod service_join;
#[path = "management_agent_tests.rs"]
mod agents;
#[path = "management_setup_readiness_tests.rs"]
mod readiness;
#[path = "management_setup_recovery_tests.rs"]
mod recovery;
use super::*;
use crate::{
    domain::{BindRoom, CreateAgent},
    management_setup_types::*,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};
const SERVICE: &str = "@_hagency_service:example.test";
async fn live() -> (
    Fixture,
    Admission,
    ProjectPlan,
    ResourceGrantView,
    ProjectAllocation,
    Actor,
) {
    let (mut f, admission, plan, grant, allocation) = prepared().await;
    f.policy = f.policy.with_project_setup(true);
    accepted(&f, &allocation).await;
    let delivery = delivered(&f).await;
    let a = record(&f, &delivery, "reserved", 11).await.allocation;
    let admin = same_actor(&f.auth, &f.outsider).await;
    (f, admission, plan, grant, a, admin)
}

// Exercise stale authority snapshots during a migration without reintroducing
// a public administrator-reassignment operation.
async fn invalidate_authority(f: &Fixture, project: &str) -> ProjectReadiness {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.transaction::<_, diesel::result::Error, _>(async |db: &mut AsyncPgConnection| {
        sql_query("UPDATE hagency_agent_v1.management_project_authorities SET revision=revision+1 WHERE plan_id=$1").bind::<Text,_>(project).execute(db).await?;
        sql_query("UPDATE hagency_agent_v1.management_project_setups SET state='awaiting_matrix_setup',verified_at_ms=NULL,revision=revision+1 WHERE plan_id=$1 AND state<>'cancelled'").bind::<Text,_>(project).execute(db).await?;
        Ok(())
    }).await.unwrap();
    f.domain.management_project_setup(&f.admin.p, &f.policy, project).await.unwrap()
}

fn assign(a: &Actor) -> AssignProjectAdmin {
    AssignProjectAdmin {
        command_id: "assign_project_admin".into(),
        expected_authority_revision: 0,
        administrator_mxid: a.p.mxid.clone(),
    }
}

fn begin(p: &ProjectPlan, a: &ProjectAllocation) -> BeginProjectSetup {
    BeginProjectSetup {
        command_id: "begin_project_setup".into(),
        allocation_id: Some(a.id.clone()),
        expected_project_revision: p.revision,
        expected_allocation_revision: a.revision,
        expected_authority_revision: 1,
        mode: "create".into(),
    }
}

async fn begun() -> (Fixture, Admission, ProjectPlan, ProjectSetup, Actor) {
    let (f, c, p, _, a, admin) = live().await;
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let s = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
        .await
        .unwrap()
        .setup
        .unwrap();
    (f, c, p, s, admin)
}

fn confirm(s: &ProjectSetup) -> ConfirmProjectSetup {
    ConfirmProjectSetup {
        command_id: "confirm_project_setup".into(),
        expected_revision: s.revision,
        space_id: format!("!space_{}:example.test", s.id),
        room_id: format!("!room_{}:example.test", s.id),
    }
}

fn states(s: &ProjectSetup, input: &ConfirmProjectSetup) -> HashMap<String, Vec<Value>> {
    let mut map = HashMap::new();
    for (id, kind, command) in [
        (&input.space_id, "space", &s.space_creation_command_id),
        (&input.room_id, "room", &s.room_creation_command_id),
    ] {
        let mut events = vec![
            json!({"type":"m.room.create","state_key":"","sender":s.manager_mxid,"content":{"creator":s.manager_mxid,"type":if kind=="space"{"m.space"}else{""}}}),
            json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
            json!({"type":"m.room.power_levels","state_key":"","content":{"users":{s.manager_mxid.clone():100,SERVICE:50},"state_default":50,"invite":50}}),
            json!({"type":"im.hagency.management.project_setup","state_key":"","sender":s.manager_mxid,"content":{"version":1,"setupId":s.id,"projectId":s.project_id,"definitionDigest":s.definition_digest,"commandId":command,"owner":s.manager_mxid,"kind":kind,"spaceId":if kind=="space"{Value::Null}else{json!(input.space_id)}}}),
        ];
        for user in std::collections::BTreeSet::from([
            s.manager_mxid.as_str(),
            s.administrator_mxid.as_str(),
            SERVICE,
        ]) {
            events.push(
                json!({"type":"m.room.member","state_key":user,"content":{"membership":"join"}}),
            );
        }
        if kind == "space" {
            events.push(json!({"type":"m.space.child","state_key":input.room_id,"content":{"via":["example.test"]}}));
        } else {
            events.push(json!({"type":"m.space.parent","state_key":input.space_id,"content":{"via":["example.test"],"canonical":true}}));
        }
        map.insert(id.clone(), events);
    }
    map
}

fn gateway(map: HashMap<String, Vec<Value>>) -> (Gateway, Arc<AtomicUsize>) {
    let map = Arc::new(map);
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    (
        Gateway::new(
            Arc::new(move |room| {
                let map = map.clone();
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    map.get(&room)
                        .cloned()
                        .ok_or(crate::Error::NotFound("fixture_room_not_found"))
                })
            }),
            SERVICE.into(),
        ),
        count,
    )
}

fn svc(f: &Fixture, gateway: Gateway) -> Service {
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
            f.auth.clone(),
            verifier,
            origin,
            issuer,
            "example.test".into(),
        )
        .with_domain(f.domain.clone(), gateway)
        .with_management(f.policy.clone())
        .router(),
    )
}

async fn read(f: &Fixture, p: &ProjectPlan) -> ProjectReadiness {
    f.domain
        .management_project_setup(&f.outsider.p, &f.policy, &p.id)
        .await
        .unwrap()
}

async fn confirm_ok(f: &Fixture, s: &ProjectSetup) -> (ProjectReadiness, Gateway) {
    let i = confirm(s);
    let (gw, _) = gateway(states(s, &i));
    let result = f
        .domain
        .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i, &gw)
        .await
        .unwrap();
    (result, gw)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_manager_authority_is_automatic_pinned_and_default_off() {
    let (f, _, p, _, a, admin) = live().await;
    assert_eq!(f.domain.recover_management_setup_command(&f.admin.p, &f.policy, "never_assigned", RecoverCommandInput { operation: "project.admin.assign".into() }).await.unwrap_err().to_string(), "project_administrator_assignment_retired");
    let disabled = f.policy.clone().with_project_setup(false);
    assert_eq!(
        f.domain
            .management_project_setup(&f.outsider.p, &disabled, &p.id)
            .await
            .unwrap_err()
            .to_string(),
        "project_setup_unavailable"
    );
    for actor in [&f.owner.p, &f.coordinator.p, &f.outsider.p, &admin.p] {
        assert_eq!(
            f.domain
                .assign_management_project_admin(actor, &f.policy, &p.id, assign(&admin))
                .await
                .unwrap_err()
                .to_string(),
            "management_administrator_required"
        );
    }
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let visible = f
        .domain
        .management_project(&admin.p, &f.policy, &p.id)
        .await
        .unwrap();
    assert_eq!(visible.revision, p.revision);
    assert!(!visible.request.can_decide);
    for actor in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert_eq!(
            f.domain
                .begin_management_project_setup(actor, &f.policy, &p.id, begin(&p, &a))
                .await
                .unwrap_err()
                .to_string(),
            "project_manager_required"
        );
    }
    let first = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
        .await
        .unwrap();
    assert!(first.capacity_ready && !first.matrix_ready);
    assert!(first.can_confirm);
    assert!(!first.can_assign_administrator);
    let mut replacement = assign(&f.coordinator);
    replacement.command_id = "replace_admin".into();
    replacement.expected_authority_revision = 1;
    assert_eq!(f.domain.assign_management_project_admin(&f.admin.p, &f.policy, &p.id, replacement).await.unwrap_err().to_string(), "project_manager_is_administrator");
    let unchanged = read(&f, &p).await;
    assert_eq!(unchanged.administrator.unwrap().mxid, p.manager_mxid);
    assert_eq!(unchanged.setup.unwrap().revision, first.setup.unwrap().revision);
    assert_eq!(
        f.domain
            .management_project(&f.outsider.p, &f.policy, &p.id)
            .await
            .unwrap()
            .revision,
        p.revision
    );
    assert_eq!(allocation(&f, &p).await.revision, a.revision);
    let replay = f
        .domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    assert!(replay.setup.is_some());
    let recovered = f.domain.recover_management_setup_command(&f.admin.p, &f.policy, "assign_project_admin", RecoverCommandInput { operation: "project.admin.assign".into() }).await.unwrap();
    assert_eq!(recovered["recorded"], true);
    assert_eq!(recovered["readiness"]["administrator"]["mxid"], p.manager_mxid);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_http_uses_paired_trusted_snapshots_and_exact_recovery_without_refetch() {
    let (f, _, p, s, _) = begun().await;
    let input = confirm(&s);
    let (gw, calls) = gateway(states(&s, &input));
    let service = svc(&f, gw);
    let path = format!("project-setups/{}/confirm", s.id);
    let mut forged = serde_json::to_value(&input).unwrap();
    forged["matrixReady"] = json!(true);
    post(
        &service,
        &path,
        &f.outsider.token,
        forged,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(calls.as_ref().load(Ordering::SeqCst), 0);
    let response = post(
        &service,
        &path,
        &f.outsider.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(response["readiness"]["setup"]["spaceId"], input.space_id);
    assert_eq!(response["readiness"]["matrixReady"], true);
    // Two coherent room snapshots for each service preflight, then two for
    // final confirmation. Saved replay below must not repeat any of them.
    assert_eq!(calls.as_ref().load(Ordering::SeqCst), 6);
    post(
        &service,
        &path,
        &f.outsider.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(calls.as_ref().load(Ordering::SeqCst), 6);
    let recovered = post(
        &service,
        &format!("project-setup-commands/{}/recover", input.command_id),
        &f.outsider.token,
        json!({"operation":"project.setup.confirm"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["recorded"], true);
    let mut changed = input.clone();
    changed.space_id = "!different:example.test".into();
    post(
        &service,
        &path,
        &f.outsider.token,
        serde_json::to_value(changed).unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_project_setups SET verified_at_ms=verified_at_ms-31000 WHERE id=$1").bind::<Text,_>(&s.id).execute(&mut db).await.unwrap();
    let latest = read(&f, &p).await;
    assert!(!latest.matrix_ready);
    assert_eq!(
        latest.blocking_reason.as_deref(),
        Some("matrix_verification_required")
    );
    let token = f
        .auth
        .register_device(
            &f.outsider.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Restart".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let recovered = post(
        &service,
        &format!("project-setup-commands/{}/recover", input.command_id),
        &token,
        json!({"operation":"project.setup.confirm"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["recorded"], true);
    assert_eq!(recovered["readiness"]["matrixReady"], false);
    let token = f
        .auth
        .register_device(
            &f.outsider.session,
            RegisterDevice {
                installation_id: "different".into(),
                name: "Other".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let recovered = post(
        &service,
        &format!("project-setup-commands/{}/recover", input.command_id),
        &token,
        json!({"operation":"project.setup.confirm"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["recorded"], false);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_rejects_bad_matrix_facts_before_recording_domain_ids() {
    let (f, _, p, s, _) = begun().await;
    let input = confirm(&s);
    for case in 0..9 {
        let mut map = states(&s, &input);
        let rs = map.get_mut(&input.room_id).unwrap();
        match case {
            0=>rs.iter_mut().find(|e|e["type"]=="m.room.member"&&e["state_key"]==s.administrator_mxid).unwrap()["content"]["membership"]=json!("invite"),
            1=>rs.iter_mut().find(|e|e["type"]=="im.hagency.management.project_setup").unwrap()["content"]["commandId"]=json!("other"),
            2=>rs.push(json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}})),
            3=>rs.iter_mut().find(|e|e["type"]=="m.room.join_rules").unwrap()["content"]["join_rule"]=json!("public"),
            4=>rs.iter_mut().find(|e|e["type"]=="m.room.member"&&e["state_key"]==SERVICE).unwrap()["content"]["membership"]=json!("leave"),
            5=>rs.retain(|e|e["type"]!="m.space.parent"),
            6=>rs.iter_mut().find(|e|e["type"]=="m.room.create").unwrap()["sender"]=json!("@stranger:example.test"),
            7=>map.get_mut(&input.space_id).unwrap().retain(|e|e["type"]!="m.space.child"),
            _=>{rs.iter_mut().find(|e|e["type"]=="m.room.member"&&e["state_key"]==s.manager_mxid).unwrap()["content"]["membership"]=json!("leave");rs.retain(|e|e["type"]!="im.hagency.management.project_setup");},
        }
        let (gw, _) = gateway(map);
        let error = f
            .domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, input.clone(), &gw)
            .await
            .unwrap_err();
        if case == 8 {
            assert!(
                matches!(error, crate::Error::Unauthorized(_)),
                "must authorize before detailed proof errors: {error}"
            );
        }
        let latest = read(&f, &p).await;
        assert!(!latest.matrix_ready);
        assert!(latest.setup.unwrap().domain_project_id.is_none());
    }
    confirm_ok(&f, &s).await;
}

fn admin_facts(
    p: &Principal,
    room: &str,
    space: bool,
    parent: Option<&str>,
) -> crate::domain::AdminFacts {
    crate::domain::AdminFacts {
        actor_mxid: p.mxid.clone(),
        room_id: room.into(),
        observed_at_ms: now_ms(),
        joined: true,
        can_manage_policy: true,
        is_space: space,
        linked_space_id: parent.map(str::to_owned),
    }
}

fn room_facts(p: &Principal, room: &str, space: &str, puppet: &str) -> crate::domain::RoomFacts {
    crate::domain::RoomFacts {
        owner_mxid: p.mxid.clone(),
        room_id: room.into(),
        space_id: space.into(),
        observed_at_ms: now_ms(),
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: Some(puppet.into()),
        puppet_in_room: false,
        encrypted: false,
        owner_direct_valid: true,
    }
}

async fn personal_agent(f: &Fixture) -> crate::domain::Agent {
    let a = f
        .domain
        .create_agent(
            &f.outsider.p,
            CreateAgent {
                display_name: "Personal agent".into(),
                idempotency_key: "personal_setup_test".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    f.domain
        .confirm_identity_provisioned(&a.id, a.generation)
        .await
        .unwrap();
    a
}

async fn personal_project(f: &Fixture, space: &str, room: &str) -> crate::domain::Project {
    let sf = admin_facts(&f.outsider.p, space, true, None);
    let p = f
        .domain
        .register_project(&f.outsider.p, space, &sf, now_ms())
        .await
        .unwrap();
    f.domain
        .register_room(
            &f.outsider.p,
            &p.id,
            room,
            &sf,
            &admin_facts(&f.outsider.p, room, false, Some(space)),
            now_ms(),
        )
        .await
        .unwrap();
    p
}

async fn bind(
    f: &Fixture,
    agent: &crate::domain::Agent,
    project: &str,
    space: &str,
    room: &str,
    key: &str,
) -> crate::Result<crate::domain::AgentBinding> {
    f.domain
        .bind_room(
            &f.outsider.p,
            &agent.id,
            BindRoom {
                project_id: project.into(),
                room_id: room.into(),
                idempotency_key: key.into(),
            },
            &room_facts(&f.outsider.p, room, space, &agent.puppet_mxid),
            now_ms(),
        )
        .await
}

#[derive(QueryableByName)]
struct Matched {
    #[diesel(sql_type=diesel::sql_types::Bool)]
    matched: bool,
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn all_managed_children_reject_generic_and_direct_bindings_while_personal_projects_work() {
    let (f, _, _, s, _) = begun().await;
    let (done, gw) = confirm_ok(&f, &s).await;
    let ready = done.setup.unwrap();
    let project = ready.domain_project_id.unwrap();
    let space = ready.space_id.unwrap();
    let room = ready.room_id.unwrap();
    let a = personal_agent(&f).await;
    let error = bind(&f, &a, &project, &space, &room, "bypass_project")
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "managed_agent_admission_required");
    let mut response = TestClient::post(format!(
        "https://example.test/hagency/api/v1/agents/{}/bindings",
        a.id
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(&f.outsider.session)
    .json(&json!({"projectId":project,"roomId":room,"idempotencyKey":"http_bypass"}))
    .send(&svc(&f, gw))
    .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let body: Value = response.take_json().await.unwrap();
    assert!(
        body.to_string()
            .contains("managed_agent_admission_required")
    );
    let secondary = format!("!secondary_{}:example.test", s.id);
    f.domain
        .register_room(
            &f.outsider.p,
            &project,
            &secondary,
            &admin_facts(&f.outsider.p, &space, true, None),
            &admin_facts(&f.outsider.p, &secondary, false, Some(&space)),
            now_ms(),
        )
        .await
        .unwrap();
    for candidate in [&room, &secondary] {
        assert_eq!(
            f.domain
                .adopt_owner_direct(
                    &f.outsider.p,
                    &a.id,
                    candidate,
                    &room_facts(&f.outsider.p, candidate, "", &a.puppet_mxid),
                    now_ms()
                )
                .await
                .unwrap_err()
                .to_string(),
            "managed_agent_admission_required"
        );
        assert_eq!(
            bind(&f, &a, &project, &space, candidate, "bypass_child")
                .await
                .unwrap_err()
                .to_string(),
            "managed_agent_admission_required"
        );
    }
    f.domain
        .set_project_policy(
            &f.outsider.p,
            &project,
            1,
            Default::default(),
            &admin_facts(&f.outsider.p, &space, true, None),
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(
        bind(&f, &a, &project, &space, &room, "bypass_policy")
            .await
            .unwrap_err()
            .to_string(),
        "managed_agent_admission_required"
    );
    let direct_events = vec![
        json!({"type":"m.room.create","state_key":"","content":{}}),
        json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
        json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
        json!({"type":"m.room.member","state_key":f.outsider.p.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":a.puppet_mxid,"content":{"membership":"join"}}),
    ];
    let (direct_gateway, _) = gateway(HashMap::from([(secondary.clone(), direct_events)]));
    let mut response = TestClient::post(format!(
        "https://example.test/hagency/api/v1/agents/{}/owner-direct",
        a.id
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(&f.outsider.session)
    .json(&json!({"roomId":secondary}))
    .send(&svc(&f, direct_gateway))
    .await;
    assert_eq!(response.status_code, Some(StatusCode::UNAUTHORIZED));
    let body: Value = response.take_json().await.unwrap();
    assert!(
        body.to_string()
            .contains("managed_agent_admission_required")
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.rooms WHERE room_id=$1")
            .bind::<Text, _>(&secondary)
            .execute(&mut db)
            .await
            .is_err()
    );
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.management_project_setups WHERE id=$1")
            .bind::<Text, _>(&s.id)
            .execute(&mut db)
            .await
            .is_err()
    );
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.management_project_setups SET definition_digest=$2 WHERE id=$1"
        )
        .bind::<Text, _>(&s.id)
        .bind::<Text, _>("f".repeat(64))
        .execute(&mut db)
        .await
        .is_err()
    );
    for candidate in [&room, &secondary] {
        assert!(sql_query("INSERT INTO hagency_agent_v1.bindings(id,agent_id,room_id,scope_kind,state) VALUES($1,$2,$3,'owner_direct','active')").bind::<Text,_>(format!("bnd_{}",secret_token())).bind::<Text,_>(&a.id).bind::<Text,_>(candidate).execute(&mut db).await.is_err());
    }
    let other_space = format!("!personal_space_{}:example.test", s.id);
    let other_room = format!("!personal_room_{}:example.test", s.id);
    let other = personal_project(&f, &other_space, &other_room).await;
    let personal = bind(
        &f,
        &a,
        &other.id,
        &other_space,
        &other_room,
        "personal_still_works",
    )
    .await
    .unwrap();
    assert_eq!(personal.agent.owner_user_id, f.outsider.p.user_id);
    assert_eq!(personal.agent.execution_device_id, f.outsider.p.device_id);
    // A caller cannot move an existing personal binding into a protected child.
    assert!(
        sql_query("UPDATE hagency_agent_v1.bindings SET room_id=$2,project_id=$3 WHERE id=$1")
            .bind::<Text, _>(&personal.binding.id)
            .bind::<Text, _>(&secondary)
            .bind::<Text, _>(&project)
            .execute(&mut db)
            .await
            .is_err()
    );
    // Nor register a pre-bound owner-direct room as a new managed child.
    let direct = format!("!direct_{}:example.test", s.id);
    f.domain
        .adopt_owner_direct(
            &f.outsider.p,
            &a.id,
            &direct,
            &room_facts(&f.outsider.p, &direct, "", &a.puppet_mxid),
            now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .register_room(
                &f.outsider.p,
                &project,
                &direct,
                &admin_facts(&f.outsider.p, &space, true, None),
                &admin_facts(&f.outsider.p, &direct, false, Some(&space)),
                now_ms()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn preexisting_bindings_and_concurrent_personal_binding_cannot_enter_a_managed_project() {
    for race in [false, true] {
        let (f, _, _, s, _) = begun().await;
        let i = confirm(&s);
        let (gw, _) = gateway(states(&s, &i));
        let project = personal_project(&f, &i.space_id, &i.room_id).await;
        let a = personal_agent(&f).await;
        if race {
            let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
                .await
                .unwrap();
            let (bound, ready) = tokio::join!(
                bind(&f, &a, &project.id, &i.space_id, &i.room_id, "racing_bind"),
                other.confirm_management_project_setup(
                    &f.outsider.p,
                    &f.policy,
                    &s.id,
                    i.clone(),
                    &gw
                )
            );
            assert_eq!(usize::from(bound.is_ok()) + usize::from(ready.is_ok()), 1);
        } else {
            bind(&f, &a, &project.id, &i.space_id, &i.room_id, "prior_bind")
                .await
                .unwrap();
            assert_eq!(
                f.domain
                    .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i, &gw)
                    .await
                    .unwrap_err()
                    .to_string(),
                "managed_project_has_bindings"
            );
        }
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_setups s JOIN hagency_agent_v1.bindings b ON b.project_id=s.domain_project_id OR b.room_id=s.room_id WHERE s.id=$1) AS matched").bind::<Text,_>(&s.id).get_result::<Matched>(&mut db).await.unwrap().matched);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_requires_current_capacity_active_assigned_admin_and_exact_revision() {
    let (f, _, p, _, a, admin) = live().await;
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let mut stale = begin(&p, &a);
    stale.expected_allocation_revision -= 1;
    assert_eq!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, stale)
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let s = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
        .await
        .unwrap()
        .setup
        .unwrap();
    let i = confirm(&s);
    let (gw, _) = gateway(states(&s, &i));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.users SET active=false WHERE id=$1")
        .bind::<Text, _>(&admin.p.user_id)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i.clone(), &gw)
            .await
            .unwrap_err()
            .to_string(),
        "authorization_expired"
    );
    sql_query("UPDATE hagency_agent_v1.users SET active=true WHERE id=$1")
        .bind::<Text, _>(&admin.p.user_id)
        .execute(&mut db)
        .await
        .unwrap();
    // Known native receipt is insufficient once its exact execution generation rotates.
    f.auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Rotated".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i, &gw)
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
    assert!(!read(&f, &p).await.capacity_ready);
    let (mut pending, _, plan, _, _) = prepared().await;
    pending.policy = pending.policy.with_project_setup(true);
    let admin = same_actor(&pending.auth, &pending.outsider).await;
    pending
        .domain
        .assign_management_project_admin(
            &pending.admin.p,
            &pending.policy,
            &plan.id,
            assign(&admin),
        )
        .await
        .unwrap();
    let a = allocation(&pending, &plan).await;
    assert_eq!(
        pending
            .domain
            .begin_management_project_setup(
                &pending.outsider.p,
                &pending.policy,
                &plan.id,
                begin(&plan, &a)
            )
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn revoked_contribution_cannot_become_ready_even_when_matrix_reads_are_in_flight() {
    let (f, c, p, s, _) = begun().await;
    let i = confirm(&s);
    let state = Arc::new(states(&s, &i));
    let started = Arc::new(tokio::sync::Barrier::new(3));
    let release = Arc::new(tokio::sync::Barrier::new(3));
    let gateway = Gateway::new(
        Arc::new({
            let started = started.clone();
            let release = release.clone();
            move |room| {
                let state = state.clone();
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    started.wait().await;
                    release.wait().await;
                    Ok(state.get(&room).unwrap().clone())
                })
            }
        }),
        SERVICE.into(),
    );
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let manager = f
        .auth
        .authenticate(&f.outsider.token, now_ms(), true)
        .await
        .unwrap();
    let policy = f.policy.clone();
    let sid = s.id.clone();
    let checking = tokio::spawn(async move {
        other
            .confirm_management_project_setup(&manager, &policy, &sid, i, &gateway)
            .await
    });
    started.wait().await;
    f.domain
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &c.contribution.id,
            RevokeInput {
                command_id: "revoke_during_matrix_read".into(),
                expected_revision: c.contribution.revision,
            },
        )
        .await
        .unwrap();
    release.wait().await;
    assert_eq!(
        checking.await.unwrap().unwrap_err().to_string(),
        "live_reservation_required"
    );
    let latest = read(&f, &p).await;
    assert!(!latest.capacity_ready && !latest.matrix_ready);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_outbox_failure_rolls_back_link_domain_rows_and_command_receipt() {
    let (f, _, p, s, _) = begun().await;
    let i = confirm(&s);
    let (gw, _) = gateway(states(&s, &i));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let function = format!("fail_setup_{}", &crate::hash(&secret_token())[..16]);
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_setup_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.outsider.p.mxid)).await.unwrap();
    assert!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i.clone(), &gw)
            .await
            .is_err()
    );
    let latest = read(&f, &p).await;
    assert!(!latest.matrix_ready);
    assert!(latest.setup.unwrap().domain_project_id.is_none());
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.projects WHERE space_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.rooms WHERE room_id=$2) AS matched").bind::<Text,_>(&i.space_id).bind::<Text,_>(&i.room_id).get_result::<Matched>(&mut db).await.unwrap().matched);
    let recovered = f
        .domain
        .recover_management_setup_command(
            &f.outsider.p,
            &f.policy,
            &i.command_id,
            RecoverCommandInput {
                operation: "project.setup.confirm".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered["recorded"], false);
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.{function}();")).await.unwrap();
    assert!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, i, &gw)
            .await
            .unwrap()
            .matrix_ready
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v5_setup_migration_is_atomic_and_preserves_grants_parent_receipts_and_personal_agents() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_setup_upgrade_{}",
        &crate::hash(&secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move {
        let auth = Store::open(url.as_str(), "example.test", "https://example.test/auth/").await.unwrap();
        let owner = actor(&auth, "legacy_grant_owner").await;
        let policy = ManagementPolicy::new(owner.p.mxid.clone(), "example.test").unwrap().with_project_reservations(true).with_parent_settlement(true);
        let mut db = AsyncPgConnection::establish(url.as_str()).await.unwrap();
        for schema in [
            include_str!("management_schema.sql"),
            include_str!("management_verification_schema.sql"),
            include_str!("management_projects_schema.sql"),
            include_str!("management_allocations_schema.sql"),
            include_str!("management_parents_schema.sql"),
        ] {
            db.batch_execute(schema).await.unwrap();
        }
        let fixture: Value = serde_json::from_str(include_str!("../tests/fixtures/management-allocation-v1.json")).unwrap();
        let mut native: NativeGrantReceipt = serde_json::from_value(fixture["grant"].clone()).unwrap();
        native.contribution_id = "legacy_parent_contribution".into();
        native.owner_mxid = owner.p.mxid.clone();
        native.coordinator_mxid = owner.p.mxid.clone();
        native.executor_device_id = owner.p.device_id.clone().unwrap();
        native.executor_generation = owner.p.device_generation.unwrap();
        let bytes = serde_json::to_string(&native).unwrap();
        let digest = allocation_digest(&native).unwrap();
        let now = now_ms();
        db.transaction::<_, diesel::result::Error, _>(async |db: &mut AsyncPgConnection| {
            sql_query("INSERT INTO hagency_agent_v1.management_contributions(id,owner_user_id,executor_device_id,executor_generation,resource_id,resource_revision,label,runtime,capabilities,coordinator_mxid,state,revision,request_id,created_at_ms,updated_at_ms) VALUES($1,$2,$3,$4,$5,$6,'Original native grant','codex','[\"coding\"]',$7,'admitted',2,'legacy_parent_request',$8,$8)")
                .bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).bind::<BigInt,_>(native.executor_generation).bind::<Text,_>(&native.resource_id).bind::<Text,_>(&native.resource_revision).bind::<Text,_>(&native.coordinator_mxid).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_requests(id,kind,contribution_id,requester_user_id,approver_mxid,state,revision,created_at_ms,expires_at_ms,decided_at_ms) VALUES('legacy_parent_request','resource_admission',$1,$2,$3,'approved',2,$4,$5,$4)")
                .bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&owner.p.mxid).bind::<BigInt,_>(now).bind::<BigInt,_>(now+600000).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,'revoked',$8,$9,$10)")
                .bind::<Text,_>(&native.grant_id).bind::<Text,_>(&native.contribution_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).bind::<BigInt,_>(native.executor_generation).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).bind::<Text,_>(&native.period_key).bind::<BigInt,_>(native.period_end_ms).bind::<BigInt,_>(now).execute(db).await?;
            Ok(())
        }).await.unwrap();
        db.batch_execute(include_str!("domain_schema.sql")).await.unwrap();
        db.batch_execute("INSERT INTO hagency_agent_v1.projects(id,space_id,creation_policy) VALUES('legacy_personal','!legacy:example.test','{}')").await.unwrap();
        sql_query("INSERT INTO hagency_agent_v1.agents(id,owner_user_id,execution_device_id,puppet_mxid,display_name,state) VALUES('legacy_agent',$1,$2,'@legacy_agent:example.test','Personal','active')").bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).execute(&mut db).await.unwrap();
        let parent:Value=serde_json::from_str(include_str!("../tests/fixtures/management-parent-settlement-v1.json")).unwrap();
        let parent_bytes=serde_json::to_string(&parent["receipt"]).unwrap();
        db.transaction::<_,diesel::result::Error,_>(async |db:&mut AsyncPgConnection| {
            sql_query("INSERT INTO hagency_agent_v1.management_parent_settlements(grant_id,state,close_command_id,latest_command_id,receipt,receipt_digest) VALUES($1,'closed','legacy_close','legacy_close',$2,$3)").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&parent_bytes).bind::<Text,_>(parent["receiptDigest"].as_str().unwrap()).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.management_parent_commands(id,grant_id,owner_user_id,executor_device_id,executor_generation,operation,command,digest,state,receipt,receipt_digest) VALUES('legacy_close',$1,$2,$3,$4,'grant.close',$5,$6,'recorded',$7,$8)").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).bind::<BigInt,_>(native.executor_generation).bind::<Text,_>(parent["command"].to_string()).bind::<Text,_>(parent["commandDigest"].as_str().unwrap()).bind::<Text,_>(&parent_bytes).bind::<Text,_>(parent["receiptDigest"].as_str().unwrap()).execute(db).await?; Ok(())
        }).await.unwrap();
        // Conflict on the second CREATE proves the first table and version
        // update roll back while the old native receipt stays byte-identical.
        db.batch_execute("CREATE TABLE hagency_agent_v1.management_project_setups (id text)").await.unwrap();
        assert!(DomainStore::open(url.as_str(), "example.test", "_hagency_test_").await.is_err());
        #[derive(QueryableByName)]
        struct State { #[diesel(sql_type=diesel::sql_types::Bool)] matched: bool }
        assert!(sql_query("SELECT version=5 AND to_regclass('hagency_agent_v1.management_project_authorities') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
        assert!(sql_query("SELECT receipt=$2 AND receipt_digest=$3 AS matched FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).get_result::<State>(&mut db).await.unwrap().matched);
        db.batch_execute("DROP TABLE hagency_agent_v1.management_project_setups").await.unwrap();
        for _ in 0..2 {
            let domain = DomainStore::open(url.as_str(), "example.test", "_hagency_test_").await.unwrap();
            let _ = (&domain,&policy);
            assert!(sql_query("SELECT receipt=$2 AS matched FROM hagency_agent_v1.management_parent_settlements WHERE grant_id=$1").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&parent_bytes).get_result::<State>(&mut db).await.unwrap().matched);
            assert!(sql_query("SELECT owner_user_id=$1 AND execution_device_id=$2 AND state='active' AS matched FROM hagency_agent_v1.agents WHERE id='legacy_agent'").bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(&native.executor_device_id).get_result::<State>(&mut db).await.unwrap().matched);
            assert!(sql_query("SELECT version=29 AS matched FROM hagency_agent_v1.management_deployment").get_result::<State>(&mut db).await.unwrap().matched);
            assert!(sql_query("SELECT receipt=$2 AND receipt_digest=$3 AS matched FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&native.grant_id).bind::<Text,_>(&bytes).bind::<Text,_>(&digest).get_result::<State>(&mut db).await.unwrap().matched);
        }
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn expired_grant_parent_close_and_project_revoke_block_setup_without_erasing_evidence() {
    for cause in 0..3 {
        let (mut f, _, p, g, a, admin) = live().await;
        f.policy = f.policy.with_parent_settlement(true);
        f.domain
            .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
            .await
            .unwrap();
        let s = f
            .domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
            .await
            .unwrap()
            .setup
            .unwrap();
        let input = confirm(&s);
        let (gateway, _) = gateway(states(&s, &input));
        match cause {
            0 => {
                let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
                sql_query("UPDATE hagency_agent_v1.management_resource_grants SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1").bind::<Text,_>(&g.id).execute(&mut db).await.unwrap();
            }
            1 => {
                f.domain
                    .close_management_parent(
                        &f.owner.p,
                        &f.policy,
                        &g.id,
                        crate::management_parent_types::ParentCloseInput {
                            command_id: "close_before_setup".into(),
                            expected_grant_revision: g.revision,
                        },
                    )
                    .await
                    .unwrap();
            }
            _ => {
                f.domain
                    .revoke_management_project(
                        &f.outsider.p,
                        &f.policy,
                        &p.id,
                        RevokeInput {
                            command_id: "revoke_before_setup".into(),
                            expected_revision: p.revision,
                        },
                    )
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            f.domain
                .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, input, &gateway)
                .await
                .unwrap_err()
                .to_string(),
            "live_reservation_required"
        );
        let latest = read(&f, &p).await;
        assert!(!latest.capacity_ready && !latest.can_confirm);
        assert!(latest.setup.unwrap().domain_project_id.is_none());
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE allocation_id=$1 AND receipt IS NOT NULL) AS matched").bind::<Text,_>(&a.id).get_result::<Matched>(&mut db).await.unwrap().matched);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn begin_outbox_failure_preserves_manager_authority_and_rolls_back_stage_identity() {
    let (f, _, p, _, a, admin) = live().await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let function = format!("fail_setup_stage_{}", &crate::hash(&secret_token())[..16]);
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_setup_stage_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.outsider.p.mxid)).await.unwrap();
    let authority = read(&f, &p).await.administrator.unwrap();
    assert_eq!(authority.mxid, admin.p.mxid);
    assert_eq!(authority.revision, 1);
    assert!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
            .await
            .is_err()
    );
    assert!(read(&f, &p).await.setup.is_none());
    let recovery = f
        .domain
        .recover_management_setup_command(
            &f.outsider.p,
            &f.policy,
            "begin_project_setup",
            RecoverCommandInput {
                operation: "project.setup.begin".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovery["recorded"], false);
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox;DROP FUNCTION hagency_agent_v1.{function}()")).await.unwrap();
    assert!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
            .await
            .unwrap()
            .setup
            .is_some()
    );
}
