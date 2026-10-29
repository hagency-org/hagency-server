#[path = "management_agent_readiness_tests.rs"]
mod readiness;
#[path = "management_project_manager_tests.rs"]
mod manager_merge;
use super::*;
#[path = "management_agent_executor_tests.rs"]
mod executor;
use crate::management_agent_types::*;
use diesel::{QueryableByName, sql_types::Bool};
#[derive(QueryableByName)]
struct Check {
    #[diesel(sql_type=Bool)]
    matched: bool,
}
#[derive(QueryableByName)]
struct Payload {
    #[diesel(sql_type=Text)]
    payload: String,
}
async fn ready_agent() -> (Fixture, ProjectPlan, ProjectSetup, Actor, Gateway) {
    let (mut f, _, p, s, admin) = begun().await;
    f.policy = f.policy.with_agent_admission(true);
    let (ready, gw) = confirm_ok(&f, &s).await;
    (f, p, ready.setup.unwrap(), admin, gw)
}
async fn input(f: &Fixture, p: &ProjectPlan, s: &ProjectSetup, id: &str) -> RequestManagedAgent {
    RequestManagedAgent {
        command_id: id.into(),
        expected_setup_revision: s.revision,
        expected_setup_binding_revision: s.current_binding.as_ref().unwrap().revision,
        expected_authority_revision: s.current_binding.as_ref().unwrap().authority_revision,
        expected_allocation_revision: allocation(f, p).await.revision,
        definition: AgentDefinition {
            name: "Documentation helper".into(),
            capability: "coding".into(),
            description: "Text-only acceptance fixture; no coding tools".into(),
            tool_mode: "none".into(),
        },
        requested_tokens: 100,
    }
}
fn decide(r: &ManagedAgentRequest, id: &str) -> DecideManagedAgent {
    DecideManagedAgent {
        command_id: id.into(),
        expected_revision: r.revision,
        expected_authority_revision: r.authority_revision,
        decision: Decision::Approve,
    }
}
async fn request(
    f: &Fixture,
    p: &ProjectPlan,
    s: &ProjectSetup,
    gw: &Gateway,
) -> ManagedAgentRequest {
    f.domain
        .request_managed_agent(
            &f.outsider.p,
            &f.policy,
            &p.id,
            input(f, p, s, "request_agent").await,
            gw,
        )
        .await
        .unwrap()
}
pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    executor::remove_migration(db).await;
    db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_outbox,hagency_agent_v1.management_agent_intents,hagency_agent_v1.management_agent_decisions,hagency_agent_v1.management_agent_requests;DROP FUNCTION hagency_agent_v1.retain_managed_agent_admission();DROP TRIGGER fence_unimplemented_shared_agent ON hagency_agent_v1.agents;DROP FUNCTION hagency_agent_v1.fence_unimplemented_shared_agent();ALTER TABLE hagency_agent_v1.agents DROP COLUMN execution_kind;ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=7;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=7)").await.unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_manager_decides_no_identity_or_capacity_effect() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let i = input(&f, &p, &s, "disabled").await;
    assert_eq!(
        f.domain
            .request_managed_agent(
                &f.outsider.p,
                &f.policy.clone().with_agent_admission(false),
                &p.id,
                i.clone(),
                &gw
            )
            .await
            .unwrap_err()
            .to_string(),
        "agent_admission_unavailable"
    );
    for a in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert_eq!(
            f.domain
                .request_managed_agent(a, &f.policy, &p.id, i.clone(), &gw)
                .await
                .unwrap_err()
                .to_string(),
            "project_manager_required"
        );
    }
    let before = allocation(&f, &p).await;
    let r = request(&f, &p, &s, &gw).await;
    for a in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert_eq!(
            f.domain
                .decide_managed_agent(a, &f.policy, &r.id, decide(&r, "wrong_admin"), &gw)
                .await
                .unwrap_err()
                .to_string(),
            "assigned_project_administrator_required"
        );
    }
    let result = f
        .domain
        .decide_managed_agent(&admin.p, &f.policy, &r.id, decide(&r, "approve_agent"), &gw)
        .await
        .unwrap();
    assert_eq!(result.state, "approved_waiting_executor");
    assert!(!result.execution_available);
    assert!(!result.can_decide);
    assert_eq!(result.manager_mxid, f.outsider.p.mxid);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1) AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents WHERE request_id=$2 AND state='unavailable') AS matched").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
    let bytes = sql_query(
        "SELECT payload FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1",
    )
    .bind::<Text, _>(&r.id)
    .get_result::<Payload>(&mut db)
    .await
    .unwrap()
    .payload;
    let cmd: ManagedAgentReserveCommand = serde_json::from_str(&bytes).unwrap();
    cmd.validate("example.test").unwrap();
    assert!(sql_query("SELECT d.digest=$2 AND i.digest=$3 AS matched FROM hagency_agent_v1.management_agent_decisions d JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=d.request_id WHERE d.request_id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(&cmd.decision_digest).bind::<Text,_>(allocation_digest(&cmd).unwrap()).get_result::<Check>(&mut db).await.unwrap().matched);
    assert_eq!(cmd.manager_mxid, f.outsider.p.mxid);
    assert_eq!(cmd.decision_actor_mxid, admin.p.mxid);
    assert_eq!(cmd.executor_device_id, f.owner.p.device_id.clone().unwrap());
    assert_eq!(cmd.definition.tool_mode, "none");
    assert_eq!(allocation(&f, &p).await.revision, before.revision);
    let replay = f
        .domain
        .decide_managed_agent(&admin.p, &f.policy, &r.id, decide(&r, "approve_agent"), &gw)
        .await
        .unwrap();
    assert_eq!(replay.revision, result.revision);
    let mut conflict = decide(&r, "approve_agent");
    conflict.decision = Decision::Reject;
    assert_eq!(
        f.domain
            .decide_managed_agent(&admin.p, &f.policy, &r.id, conflict, &gw)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
    for actor in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        assert_eq!(
            f.domain
                .cancel_managed_agent_request(
                    actor,
                    &f.policy,
                    &r.id,
                    RevokeInput {
                        command_id: "wrong_cancel".into(),
                        expected_revision: result.revision
                    }
                )
                .await
                .unwrap_err()
                .to_string(),
            "project_manager_required"
        );
    }
    f.domain
        .cancel_managed_agent_request(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RevokeInput {
                command_id: "cancel_agent".into(),
                expected_revision: result.revision,
            },
        )
        .await
        .unwrap();
    let recovered = f
        .domain
        .recover_managed_agent_command(
            &admin.p,
            &f.policy,
            "approve_agent",
            RecoverCommandInput {
                operation: "agent.decide".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered["agentRequest"]["state"], "cancelled");
    assert!(recovered.get("executionAllowed").is_none());
    assert!(sql_query("SELECT state='fenced' AS matched FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_request_requires_current_matrix_control_and_bounded_offer() {
    let (f, p, s, _, gw) = ready_agent().await;
    let mut i = input(&f, &p, &s, "bad_offer").await;
    i.definition.capability = "unpublished".into();
    assert_eq!(
        f.domain
            .request_managed_agent(&f.outsider.p, &f.policy, &p.id, i, &gw)
            .await
            .unwrap_err()
            .to_string(),
        "agent_offer_unavailable"
    );
    let mut i = input(&f, &p, &s, "too_many_tokens").await;
    i.requested_tokens = 9_007_199_254_740_991;
    assert_eq!(
        f.domain
            .request_managed_agent(&f.outsider.p, &f.policy, &p.id, i, &gw)
            .await
            .unwrap_err()
            .to_string(),
        "agent_offer_unavailable"
    );
    let mut i = input(&f, &p, &s, "unsafe_number").await;
    i.requested_tokens = 9_007_199_254_740_992;
    assert_eq!(
        f.domain
            .request_managed_agent(&f.outsider.p, &f.policy, &p.id, i, &gw)
            .await
            .unwrap_err()
            .to_string(),
        "invalid_agent_number"
    );
    let mut map = states(&s, &confirm(&s));
    for events in map.values_mut() {
        for e in events {
            if e["type"] == "m.room.power_levels" {
                e["content"]["users"][&s.manager_mxid] = json!(0);
            }
        }
    }
    let (lost, _) = gateway(map);
    assert_eq!(
        f.domain
            .request_managed_agent(
                &f.outsider.p,
                &f.policy,
                &p.id,
                input(&f, &p, &s, "no_power").await,
                &lost
            )
            .await
            .unwrap_err()
            .to_string(),
        "room_administration_denied"
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests WHERE plan_id=$1) AS matched").bind::<Text,_>(&p.id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_manager_has_authority_without_an_assignment() {
    let (mut f, _, p, _, a, _) = live().await;
    f.policy = f.policy.with_agent_admission(true);
    let s = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a))
        .await
        .unwrap()
        .setup
        .unwrap();
    let (ready, gw) = confirm_ok(&f, &s).await;
    let s = ready.setup.unwrap();
    let r = request(&f, &p, &s, &gw).await;
    assert!(r.can_decide);
    let result = f
        .domain
        .decide_managed_agent(
            &f.outsider.p,
            &f.policy,
            &r.id,
            decide(&r, "assigned_manager_approves"),
            &gw,
        )
        .await
        .unwrap();
    assert_eq!(result.state, "approved_waiting_executor");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_http_closed_replay_visibility_and_no_executor_route() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let service = svc(&f, gw);
    let i = input(&f, &p, &s, "http_agent_request").await;
    let mut forged = serde_json::to_value(&i).unwrap();
    forged["ownerMxid"] = json!(f.owner.p.mxid);
    post(
        &service,
        &format!("projects/{}/agent-requests", p.id),
        &f.outsider.token,
        forged,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let response = post(
        &service,
        &format!("projects/{}/agent-requests", p.id),
        &f.outsider.token,
        serde_json::to_value(&i).unwrap(),
        StatusCode::OK,
    )
    .await;
    let r: ManagedAgentRequest = serde_json::from_value(response["agentRequest"].clone()).unwrap();
    let inbox = post(
        &service,
        "agent-requests/list",
        &admin.token,
        json!({"needsMyAction":true}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(inbox["items"][0]["id"], r.id);
    for a in [&f.owner, &f.coordinator, &f.admin] {
        let list = post(
            &service,
            "agent-requests/list",
            &a.token,
            json!({}),
            StatusCode::OK,
        )
        .await;
        assert!(list["items"].as_array().unwrap().is_empty());
    }
    let accepted = post(
        &service,
        &format!("agent-requests/{}/decide", r.id),
        &admin.token,
        serde_json::to_value(decide(&r, "http_approve")).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        accepted["agentRequest"]["state"],
        "approved_waiting_executor"
    );
    post(
        &service,
        "agent-executor-commands/poll",
        &f.owner.token,
        json!({}),
        StatusCode::NOT_FOUND,
    )
    .await;
    post(
        &service,
        "agent-commands/http_agent_request/recover",
        &f.outsider.token,
        json!({"operation":"agent.request"}),
        StatusCode::OK,
    )
    .await;
    let other = f
        .auth
        .register_device(
            &f.outsider.session,
            RegisterDevice {
                installation_id: "another-mac".into(),
                name: "Another".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let missing = post(
        &service,
        "agent-commands/http_agent_request/recover",
        &other.token,
        json!({"operation":"agent.request"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(missing["recorded"], false);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn separate_administrator_assignment_cannot_take_over_pending_agents() {
    let (f, p, s, manager, gw) = ready_agent().await;
    let pending = request(&f, &p, &s, &gw).await;
    let other = actor(&f.auth, "cannot_be_project_admin").await;
    let mut change = assign(&other);
    change.expected_authority_revision = 1;
    change.command_id = "separate_assignment_denied".into();
    assert_eq!(f.domain.assign_management_project_admin(&f.admin.p, &f.policy, &p.id, change).await.unwrap_err().to_string(), "project_manager_is_administrator");
    assert!(f.domain.management_agent_request(&other.p, &f.policy, &pending.id).await.is_err());
    let current = f.domain.management_agent_request(&manager.p, &f.policy, &pending.id).await.unwrap();
    assert_eq!(current.revision, pending.revision);
    assert!(current.can_approve);
    assert_eq!(f.domain.decide_managed_agent(&manager.p, &f.policy, &pending.id, decide(&current, "manager_approves"), &gw).await.unwrap().state, "approved_waiting_executor");
}

fn paused_gateway(
    s: &ProjectSetup,
) -> (Gateway, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let state = Arc::new(states(s, &confirm(s)));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let (e, r) = (entered.clone(), release.clone());
    let gw = Gateway::new(
        Arc::new(move |id| {
            let state = state.clone();
            let e = e.clone();
            let r = r.clone();
            let first = count.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    e.notify_one();
                    r.notified().await;
                }
                state
                    .get(&id)
                    .cloned()
                    .ok_or(crate::Error::NotFound("fixture_room_not_found"))
            })
        }),
        SERVICE.into(),
    );
    (gw, entered, release)
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_late_approval_cannot_cross_cancel_or_resource_revoke() {
    for revoke_resource in [false, true] {
        let (f, p, s, admin, gw) = ready_agent().await;
        let r = request(&f, &p, &s, &gw).await;
        let (paused, entered, resume) = paused_gateway(&s);
        let domain = f.domain.clone();
        let policy = f.policy.clone();
        let actor = f
            .auth
            .authenticate(&admin.token, now_ms(), true)
            .await
            .unwrap();
        let id = r.id.clone();
        let input = decide(&r, "inflight_approve");
        let pending = tokio::spawn(async move {
            domain
                .decide_managed_agent(&actor, &policy, &id, input, &paused)
                .await
        });
        entered.notified().await;
        if revoke_resource {
            let c = f
                .domain
                .contribution(&f.owner.p, &f.policy, &p.contribution_id)
                .await
                .unwrap();
            f.domain
                .revoke_contribution(
                    &f.owner.p,
                    &f.policy,
                    &p.contribution_id,
                    RevokeInput {
                        command_id: "revoke_while_approving".into(),
                        expected_revision: c.contribution.revision,
                    },
                )
                .await
                .unwrap();
        } else {
            f.domain
                .cancel_managed_agent_request(
                    &f.outsider.p,
                    &f.policy,
                    &r.id,
                    RevokeInput {
                        command_id: "cancel_while_approving".into(),
                        expected_revision: r.revision,
                    },
                )
                .await
                .unwrap();
        }
        resume.notify_one();
        assert!(pending.await.unwrap().is_err());
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_decisions WHERE request_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=$2) AS matched").bind::<Text,_>(&r.id).bind::<Text,_>(&r.proposed_agent_id).get_result::<Check>(&mut db).await.unwrap().matched);
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_decision_and_notice_rollback_together_and_expiry_is_closed() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let r = request(&f, &p, &s, &gw).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let name = format!("fail_agent_{}", &crate::hash(&secret_token())[..12]);
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.request_id='{}' AND NEW.request_revision>1 THEN RAISE EXCEPTION 'injected_agent_notice_failure'; END IF; RETURN NEW; END; $$;CREATE TRIGGER {name} BEFORE INSERT ON hagency_agent_v1.management_agent_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{name}()",r.id)).await.unwrap();
    assert!(
        f.domain
            .decide_managed_agent(&admin.p, &f.policy, &r.id, decide(&r, "atomic_agent"), &gw)
            .await
            .is_err()
    );
    assert!(sql_query("SELECT state='pending_approval' AND revision=1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_decisions WHERE request_id=$1) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1) AS matched FROM hagency_agent_v1.management_agent_requests WHERE id=$1").bind::<Text,_>(&r.id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute(&format!("DROP TRIGGER {name} ON hagency_agent_v1.management_agent_outbox;DROP FUNCTION hagency_agent_v1.{name}()")).await.unwrap();
    assert_eq!(
        f.domain
            .decide_managed_agent(&admin.p, &f.policy, &r.id, decide(&r, "atomic_agent"), &gw)
            .await
            .unwrap()
            .state,
        "approved_waiting_executor"
    );
    let expired = format!("expired_{}", secret_token());
    let agent = format!("expired_agent_{}", secret_token());
    sql_query("INSERT INTO hagency_agent_v1.management_agent_requests SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_agent_requests,to_jsonb(r)||jsonb_build_object('id',$2::text,'proposed_agent_id',$3::text,'state','pending_approval','revision',1,'created_at_ms',1,'expires_at_ms',2))).* FROM hagency_agent_v1.management_agent_requests r WHERE id=$1").bind::<Text,_>(&r.id).bind::<Text,_>(&expired).bind::<Text,_>(&agent).execute(&mut db).await.unwrap();
    let v = f
        .domain
        .management_agent_request(&admin.p, &f.policy, &expired)
        .await
        .unwrap();
    assert_eq!(v.state, "expired");
    assert!(!v.can_decide);
    assert_eq!(
        f.domain
            .decide_managed_agent(
                &admin.p,
                &f.policy,
                &expired,
                decide(&v, "expired_approve"),
                &gw
            )
            .await
            .unwrap_err()
            .to_string(),
        "agent_request_closed"
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn managed_agent_personal_owner_device_fk_and_scope_remain_permanent() {
    let (f, _, _, _, _) = ready_agent().await;
    let agent = personal_agent(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT execution_kind='personal' AND owner_user_id=$2 AND execution_device_id=$3 AS matched FROM hagency_agent_v1.agents WHERE id=$1").bind::<Text,_>(&agent.id).bind::<Text,_>(&f.outsider.p.user_id).bind::<Text,_>(f.outsider.p.device_id.as_ref().unwrap()).get_result::<Check>(&mut db).await.unwrap().matched);
    assert!(sql_query("UPDATE hagency_agent_v1.agents SET execution_device_id=$2,generation=generation+1 WHERE id=$1").bind::<Text,_>(&agent.id).bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.is_err());
    assert!(sql_query("UPDATE hagency_agent_v1.agents SET execution_kind='shared_project',execution_device_id=NULL,generation=generation+1 WHERE id=$1").bind::<Text,_>(&agent.id).execute(&mut db).await.is_err());
    assert!(sql_query("INSERT INTO hagency_agent_v1.agents(id,owner_user_id,puppet_mxid,display_name,state,execution_kind) VALUES($1,$2,$3,'No receipt','creating','shared_project')").bind::<Text,_>(format!("blocked_{}",secret_token())).bind::<Text,_>(&f.outsider.p.user_id).bind::<Text,_>(format!("@blocked_{}:example.test",secret_token())).execute(&mut db).await.is_err());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v7_managed_agent_migration_rolls_back_and_preserves_personal_identity() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_agent_admission_upgrade_{}",
        &crate::hash(&secret_token())[..12]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
        let auth=Store::open(url.as_str(),"example.test","https://example.test/auth/").await.unwrap();let domain=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();let owner=actor(&auth,"old_personal").await;
        let agent=domain.create_agent(&owner.p,CreateAgent{idempotency_key:"personal_before_upgrade".into(),display_name:"Original personal".into()},now_ms()).await.unwrap();
        let mut db=AsyncPgConnection::establish(url.as_str()).await.unwrap();remove_migration(&mut db).await;
        db.batch_execute("CREATE TABLE hagency_agent_v1.management_agent_requests (id text)").await.unwrap();assert!(DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.is_err());
        assert!(sql_query("SELECT version=7 AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='agents' AND column_name='execution_kind') AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
        db.batch_execute("DROP TABLE hagency_agent_v1.management_agent_requests").await.unwrap();for _ in 0..2 {DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();assert!(sql_query("SELECT execution_kind='personal' AND owner_user_id=$2 AND execution_device_id=$3 AND state='creating' AS matched FROM hagency_agent_v1.agents WHERE id=$1").bind::<Text,_>(&agent.id).bind::<Text,_>(&owner.p.user_id).bind::<Text,_>(owner.p.device_id.as_ref().unwrap()).get_result::<Check>(&mut db).await.unwrap().matched);}
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[test]
fn managed_agent_closed_canonical_fixture_and_wire_bounds() {
    let v: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-admission-v1.json"
    ))
    .unwrap();
    let cmd: ManagedAgentReserveCommand = serde_json::from_value(v["command"].clone()).unwrap();
    cmd.validate("example.test").unwrap();
    let receipt: ManagedAgentReservationReceipt =
        serde_json::from_value(v["receipt"].clone()).unwrap();
    receipt.validate_for(&cmd).unwrap();
    for (key, hash) in [
        ("definition", "definitionDigest"),
        ("decision", "decisionDigest"),
        ("command", "commandDigest"),
        ("receipt", "receiptDigest"),
    ] {
        assert_eq!(allocation_digest(&v[key]).unwrap(), v[hash]);
    }
    let mut bad = v["command"].clone();
    bad["ownerUserId"] = json!("forged");
    assert!(serde_json::from_value::<ManagedAgentReserveCommand>(bad).is_err());
    let mut bad = receipt.clone();
    bad.command_digest = "d".repeat(64);
    assert!(bad.validate_for(&cmd).is_err());
    let mut bad = cmd.clone();
    bad.definition.tool_mode = "shell".into();
    assert!(bad.validate("example.test").is_err());
    let mut maximum = cmd.clone();
    maximum.definition.description = "\"".repeat(2000);
    maximum.definition.name = "n".repeat(160);
    maximum.definition.capability = "c".repeat(80);
    maximum.definition_digest = allocation_digest(&maximum.definition).unwrap();
    maximum.validate("example.test").unwrap();
    assert!(serde_json::to_vec(&maximum).unwrap().len() < 16 * 1024);
    let page = vec![serde_json::to_value(&maximum).unwrap(); 16];
    assert!(serde_json::to_vec(&page).unwrap().len() < 96 * 1024);
    let mut refused = receipt;
    refused.state = "refused".into();
    refused.agent_reservation_id = None;
    refused.budget_revision = 0;
    refused.allocated_tokens = 0;
    refused.retained_tokens = 0;
    refused.reason = Some("project_capacity_unavailable".into());
    refused.validate_for(&cmd).unwrap();
}
