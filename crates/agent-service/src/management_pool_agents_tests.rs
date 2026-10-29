use super::*;
use crate::management_agent_types::AgentDefinition;
use crate::management_pool_types::*;

pub(super) async fn isolated_fixture() -> Fixture {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!("pool_case_{}", &crate::hash(&secret_token())[..16]);
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = url::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let mut f = fixture_at(url.to_string()).await;
    f.policy = f.policy.with_project_reservations(true);
    f
}
pub(super) async fn pool(f: &Fixture, manager: &Principal) -> Admission {
    let a = admitted(f).await;
    let mut g = grant_input(f, &a, manager).await;
    g.receipt.version = 2;
    g.receipt.admission_request_id = Some(a.request.id.clone());
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g)
        .await
        .unwrap();
    a
}
pub(super) fn input(a: &Admission, id: &str) -> RequestPoolAgent {
    RequestPoolAgent {
        command_id: id.into(),
        room_id: format!("!pool_{id}:test.local"),
        expected_pool_revision: a.contribution.revision,
        expected_grant_revision: 1,
        definition: AgentDefinition {
            name: "Direct pool helper".into(),
            capability: a.contribution.capabilities.iter().next().unwrap().clone(),
            description: "No project".into(),
            tool_mode: "none".into(),
        },
        requested_tokens: 100,
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn applicant_sees_two_owners_pools_but_only_selected_owner_can_approve() {
    let mut f = isolated_fixture().await;
    f.policy = f
        .policy
        .with_agent_admission(true)
        .with_agent_executor(true);
    let mut other = fixture_at(f.url.clone()).await;
    other.policy = other.policy.with_project_reservations(true);
    let a = pool(&f, &f.outsider.p).await;
    let b = pool(&other, &f.outsider.p).await;
    let mut items = Vec::new();
    let mut after = None;
    loop {
        let page = f
            .domain
            .resource_pools(
                &f.outsider.p,
                &f.policy,
                CatalogQuery {
                    after,
                    capability: None,
                },
            )
            .await
            .unwrap();
        items.extend(page.items);
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    for contribution in [&a, &b] {
        let entry = items
            .iter()
            .find(|p| p.id == contribution.contribution.id)
            .unwrap();
        assert!(entry.can_request);
        assert!(entry.native_available_tokens.is_none());
        assert_eq!(entry.owner_mxid, contribution.contribution.owner_mxid);
    }
    let request = f
        .domain
        .request_pool_agent(
            &f.outsider.p,
            &f.policy,
            &a.contribution.id,
            input(&a, "pool_request"),
        )
        .await
        .unwrap();
    assert_eq!(request.manager_mxid, f.outsider.p.mxid);
    assert_eq!(request.resource_owner_mxid, f.owner.p.mxid);
    assert!(
        serde_json::to_value(&request)
            .unwrap()
            .get("projectId")
            .is_none()
    );
    for wrong in [&f.admin.p, &f.outsider.p, &other.owner.p] {
        assert!(
            f.domain
                .decide_pool_agent(
                    wrong,
                    &f.policy,
                    &request.id,
                    DecidePoolAgent {
                        command_id: "wrong_owner_approval".into(),
                        expected_revision: 1,
                        decision: Decision::Approve,
                    }
                )
                .await
                .is_err()
        );
    }
    let queue = f
        .domain
        .pool_agent_requests(
            &f.coordinator.p,
            PoolAgentQuery {
                needs_my_action: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(queue.items.len(), 1);
    assert!(queue.items[0].can_decide);
    let approved = f
        .domain
        .decide_pool_agent(
            &f.coordinator.p,
            &f.policy,
            &request.id,
            DecidePoolAgent {
                command_id: "owner_approves_pool_agent".into(),
                expected_revision: 1,
                decision: Decision::Approve,
            },
        )
        .await
        .unwrap();
    assert_eq!(approved.state, "approved_waiting_executor");
    let delivery = f
        .domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap()
        .unwrap();
    assert!(delivery.execution_allowed);
    let command = serde_json::to_value(delivery.command).unwrap();
    assert_eq!(command["version"], 2);
    assert_eq!(command["decisionActorMxid"], f.owner.p.mxid);
    assert!(command.get("projectId").is_none());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    #[derive(QueryableByName)]
    struct Counts {
        #[diesel(sql_type=BigInt)]
        projects: i64,
        #[diesel(sql_type=BigInt)]
        allocations: i64,
    }
    let counts=sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.management_projects WHERE contribution_id=$1) AS projects,(SELECT count(*) FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id WHERE g.contribution_id=$1) AS allocations")
        .bind::<Text,_>(&a.contribution.id).get_result::<Counts>(&mut db).await.unwrap();
    assert_eq!((counts.projects, counts.allocations), (0, 0));
    let stopped = f
        .domain
        .stop_pool_agent(
            &f.coordinator.p,
            &request.id,
            RevokeInput {
                command_id: "owner_stops_supply".into(),
                expected_revision: approved.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(stopped.state, "revoked");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_agent_http_is_closed_and_retries_do_not_duplicate_requests() {
    let mut f = isolated_fixture().await;
    f.policy = f.policy.with_agent_admission(true);
    let a = pool(&f, &f.outsider.p).await;
    let service = service(&f, true);
    let path = format!("pools/{}/agents", a.contribution.id);
    let body = serde_json::to_value(input(&a, "http_pool_agent")).unwrap();
    let mut injected = body.clone();
    injected["administratorMxid"] = json!(f.outsider.p.mxid);
    post(
        &service,
        &path,
        &f.outsider.token,
        injected,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let first = post(
        &service,
        &path,
        &f.outsider.token,
        body.clone(),
        StatusCode::OK,
    )
    .await;
    let replay = post(&service, &path, &f.outsider.token, body, StatusCode::OK).await;
    assert_eq!(first["request"]["id"], replay["request"]["id"]);
    let id = first["request"]["id"].as_str().unwrap();
    post(
        &service,
        &format!("pool-agent-requests/{id}/decide"),
        &f.admin.token,
        json!({"commandId":"admin_cannot_approve","expectedRevision":1,"decision":"approve"}),
        StatusCode::NOT_FOUND,
    )
    .await;
    let result = post(
        &service,
        &format!("pool-agent-requests/{id}/decide"),
        &f.owner.token,
        json!({"commandId":"owner_can_approve","expectedRevision":1,"decision":"approve"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result["request"]["state"], "approved_waiting_executor");
}
