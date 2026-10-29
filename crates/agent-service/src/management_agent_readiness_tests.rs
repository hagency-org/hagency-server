use super::*;
use crate::Error;

async fn read(f: &Fixture, p: &Principal, project: &str) -> AgentRequestReadiness {
    f.domain
        .management_agent_readiness(p, &f.policy, project)
        .await
        .unwrap()
}
async fn counts(f: &Fixture) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT json_build_array((SELECT count(*) FROM hagency_agent_v1.management_commands),(SELECT count(*) FROM hagency_agent_v1.management_agent_requests),(SELECT count(*) FROM hagency_agent_v1.management_agent_intents),(SELECT count(*) FROM hagency_agent_v1.management_agent_outbox),(SELECT count(*) FROM hagency_agent_v1.agents))::text AS payload")
        .get_result::<Payload>(&mut db).await.unwrap().payload
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_readiness_exact_pinned_context_readonly_roles_and_unknown_headroom() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let before = counts(&f).await;
    let current = read(&f, &f.outsider.p, &p.id).await;
    assert!(current.can_request && current.can_view_requests);
    assert!(current.blocking_reason.is_none());
    let c = current.request_context.unwrap();
    let b = s.current_binding.as_ref().unwrap();
    assert_eq!(c.setup_id, s.id);
    assert_eq!(c.setup_revision, s.revision);
    assert_eq!(c.setup_binding_revision, b.revision);
    assert_eq!(c.authority_revision, b.authority_revision);
    assert_eq!(c.allocation_id, b.allocation_id);
    assert_eq!(c.allocation_receipt_digest, b.allocation_receipt_digest);
    assert_eq!(c.allocation_revision, allocation(&f, &p).await.revision);
    assert!(c.requested_token_limit > 0);
    assert!(c.native_available_tokens.is_none());
    assert_eq!(c.tool_modes, ["none"]);
    assert!(c.capabilities.contains(&"coding".into()));
    for actor in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        let r = read(&f, actor, &p.id).await;
        assert!(!r.can_request);
        assert_eq!(r.can_view_requests, actor.user_id == admin.p.user_id);
        if !r.can_view_requests {
            assert!(r.request_context.is_none());
        }
    }
    assert_eq!(counts(&f).await, before);
    let outsider = actor(&f.auth, "unrelated_readiness").await;
    assert!(matches!(
        f.domain
            .management_agent_readiness(&outsider.p, &f.policy, &p.id)
            .await,
        Err(Error::NotFound(_))
    ));
    // The exact snapshot supplies request CAS; central request still observes
    // Matrix facts and revalidates every pin instead of trusting this read.
    let r = f
        .domain
        .request_managed_agent(
            &f.outsider.p,
            &f.policy,
            &p.id,
            RequestManagedAgent {
                command_id: "from_readiness".into(),
                expected_setup_revision: c.setup_revision,
                expected_setup_binding_revision: c.setup_binding_revision,
                expected_authority_revision: c.authority_revision,
                expected_allocation_revision: c.allocation_revision,
                definition: input(&f, &p, &s, "unused").await.definition,
                requested_tokens: 100,
            },
            &gw,
        )
        .await
        .unwrap();
    assert_eq!(r.state, "pending_approval");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_discovery_matches_exact_route_gate_and_http_is_closed() {
    let (mut f, p, _, _, _) = ready_agent().await;
    let initial = f.policy.clone();
    for (admit, setup, reservations) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
        (true, true, true),
    ] {
        f.policy = initial
            .clone()
            .with_agent_admission(admit)
            .with_project_setup(setup)
            .with_project_reservations(reservations);
        let (gw, _) = gateway(HashMap::new());
        let service = svc(&f, gw);
        let mut response =
            TestClient::get("https://example.test/hagency/api/v1/management/session")
                .add_header("host", "example.test", true)
                .bearer_auth(&f.outsider.token)
                .send(&service)
                .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let session: Value = response.take_json().await.unwrap();
        assert_eq!(
            session["features"]
                .as_array()
                .unwrap()
                .contains(&json!(AGENT_ADMISSION_FEATURE)),
            admit && reservations
        );
        let mut response = TestClient::get(format!(
            "https://example.test/hagency/api/v1/management/projects/{}/agent-readiness",
            p.id
        ))
        .add_header("host", "example.test", true)
        .bearer_auth(&f.outsider.token)
        .send(&service)
        .await;
        if admit && reservations {
            assert_eq!(response.status_code, Some(StatusCode::OK));
            let value: Value = response.take_json().await.unwrap();
            assert!(value["readiness"]["canRequest"].as_bool().unwrap());
            assert!(value["readiness"]["requestContext"]["nativeAvailableTokens"].is_null());
            assert!(value["readiness"]["requestContext"]["enrollmentId"].is_null());
        } else {
            assert_eq!(response.status_code, Some(StatusCode::SERVICE_UNAVAILABLE));
        }
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_readiness_drops_rotated_executor_and_stale_allocation() {
    for case in ["executor", "allocation", "authority"] {
        let (f, p, _, _, _) = ready_agent().await;
        assert!(read(&f, &f.outsider.p, &p.id).await.can_request);
        match case {
            "executor" => {
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
            }
            "allocation" => {
                let a = allocation(&f, &p).await;
                let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
                sql_query("UPDATE hagency_agent_v1.management_allocations SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1 WHERE id=$1").bind::<Text,_>(&a.id).execute(&mut db).await.unwrap();
            }
            _ => {
                invalidate_authority(&f, &p.id).await;
            }
        }
        let r = read(&f, &f.outsider.p, &p.id).await;
        assert!(!r.can_request, "{case}");
        assert!(r.request_context.is_none(), "{case}");
        assert!(r.blocking_reason.is_some(), "{case}");
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_readiness_missing_setup_has_no_zero_capacity_invention() {
    let (mut f, _, p, _, _, _) = live().await;
    f.policy = f.policy.with_agent_admission(true);
    let r = read(&f, &f.outsider.p, &p.id).await;
    assert!(!r.can_request);
    assert!(r.request_context.is_none());
    assert_eq!(
        r.blocking_reason.as_deref(),
        Some("matrix_setup_required")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn admission_project_filter_precedes_paging_and_cannot_reveal_other_roles() {
    let (f, p, s, admin, gw) = ready_agent().await;
    let mut first = None;
    for n in 0..18 {
        let r = f
            .domain
            .request_managed_agent(
                &f.outsider.p,
                &f.policy,
                &p.id,
                input(&f, &p, &s, &format!("paging_{n}")).await,
                &gw,
            )
            .await
            .unwrap();
        if first.is_none() {
            first = Some(r);
        }
    }
    let original = first.unwrap();
    let p2 = f
        .domain
        .request_management_project(
            &f.outsider.p,
            &f.policy,
            ProjectRequestInput {
                command_id: "second_for_filter".into(),
                contribution_id: p.contribution_id.clone(),
                name: "Other project".into(),
                purpose: "Read projection fixture".into(),
                requested_tokens: 100,
            },
        )
        .await
        .unwrap();
    let p2 = f
        .domain
        .decide_management_project(
            &f.admin.p,
            &f.policy,
            &p2.request.id,
            DecisionInput {
                command_id: "approve_second_filter".into(),
                expected_revision: p2.request.revision,
                decision: Decision::Approve,
            },
        )
        .await
        .unwrap();
    f.domain
        .assign_management_project_admin(
            &f.admin.p,
            &f.policy,
            &p2.id,
            AssignProjectAdmin {
                command_id: "assign_second_filter".into(),
                expected_authority_revision: 0,
                administrator_mxid: admin.p.mxid.clone(),
            },
        )
        .await
        .unwrap();
    // Seed one historical request projection whose setup is unavailable. This
    // tests pagination/visibility, not a claim of a second native reservation.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_requests(id,plan_id,setup_id,setup_binding_revision,allocation_id,allocation_receipt_digest,manager_user_id,domain_project_id,room_id,proposed_agent_id,definition,definition_digest,requested_tokens,period_key,administrator_user_id,authority_revision,state,revision,created_at_ms,expires_at_ms) SELECT 'zz_filtered_request',$2,setup_id,setup_binding_revision,allocation_id,allocation_receipt_digest,manager_user_id,domain_project_id,room_id,'zz_filtered_agent',definition,definition_digest,requested_tokens,period_key,administrator_user_id,authority_revision,state,revision,created_at_ms,expires_at_ms FROM hagency_agent_v1.management_agent_requests WHERE id=$1").bind::<Text,_>(&original.id).bind::<Text,_>(&p2.id).execute(&mut db).await.unwrap();
    let first = f
        .domain
        .management_agent_requests(&f.outsider.p, &f.policy, AgentRequestQuery::default())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 16);
    assert!(first.next_after.is_some());
    assert!(first.items.iter().all(|r| r.project_id == p.id));
    let filtered = f
        .domain
        .management_agent_requests(
            &f.outsider.p,
            &f.policy,
            AgentRequestQuery {
                project_id: Some(p2.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(filtered.items.len(), 1);
    assert!(filtered.next_after.is_none());
    assert_eq!(filtered.items[0].id, "zz_filtered_request");
    assert_eq!(
        filtered.items[0].project_name.as_deref(),
        Some("Other project")
    );
    let page = f
        .domain
        .management_agent_requests(
            &admin.p,
            &f.policy,
            AgentRequestQuery {
                project_id: Some(p.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 16);
    let next = f
        .domain
        .management_agent_requests(
            &admin.p,
            &f.policy,
            AgentRequestQuery {
                project_id: Some(p.id.clone()),
                after: page.next_after,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items.len(), 2);
    assert!(next.next_after.is_none());
    for actor in [&f.owner.p, &f.coordinator.p, &f.admin.p] {
        for id in [&p2.id, "missing_project"] {
            assert!(matches!(
                f.domain
                    .management_agent_requests(
                        actor,
                        &f.policy,
                        AgentRequestQuery {
                            project_id: Some(id.into()),
                            ..Default::default()
                        }
                    )
                    .await,
                Err(Error::NotFound(_))
            ));
        }
    }
    let mut old = serde_json::to_value(original).unwrap();
    old.as_object_mut().unwrap().remove("projectName");
    assert!(
        serde_json::from_value::<ManagedAgentRequest>(old)
            .unwrap()
            .project_name
            .is_none()
    );
    assert!(
        serde_json::from_value::<AgentRequestQuery>(
            json!({"includeProjects":true,"includeAllocations":true})
        )
        .is_ok()
    );
    assert!(serde_json::from_value::<AgentRequestQuery>(json!({"ownerMxid":"forged"})).is_err());
}
