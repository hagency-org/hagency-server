use super::*;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn setup_discovery_requires_explicit_policy_and_preserves_cleanup_without_reservations() {
    let (mut f, _, p, _, _, _) = live().await;
    let policy = f.policy.clone();
    for (setup, reservations) in [(false, false), (false, true), (true, true), (true, false)] {
        f.policy = policy
            .clone()
            .with_project_setup(setup)
            .with_project_reservations(reservations);
        let (gw, _) = gateway(HashMap::new());
        let mut response =
            TestClient::get("https://example.test/hagency/api/v1/management/session")
                .add_header("host", "example.test", true)
                .bearer_auth(&f.outsider.token)
                .send(&svc(&f, gw))
                .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let value: Value = response.take_json().await.unwrap();
        let features = value["features"].as_array().unwrap();
        assert!(!features.contains(&json!(PROJECT_SETUP_FEATURE)));
        assert!(!features.contains(&json!(crate::management_agent_types::AGENT_CAPABILITY)));
        if !setup && !reservations {
            assert_eq!(
                features,
                &[
                    json!("resource-owner-coordinator-v1"),
                    json!(crate::management_agent_cleanup_types::AGENT_CLEANUP_FEATURE),
                    json!(crate::management_agent_settlement_types::AGENT_SETTLEMENT_FEATURE),
                    json!(crate::management_shared_intake_types::SHARED_RUN_CONTROL_FEATURE)
                ]
            );
        }
        if setup && !reservations {
            let current = read(&f, &p).await;
            assert!(current.eligible_allocation.is_none());
            assert!(!current.can_begin && !current.can_confirm && !current.can_rebase);
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn eligible_allocation_is_exact_http_begin_input_and_old_receipts_still_decode() {
    let (f, _, p, _, a, admin) = live().await;
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let assigned = read(&f, &p).await;
    let eligible = assigned.eligible_allocation.as_ref().unwrap();
    assert_eq!(eligible.id, a.id);
    assert_eq!(eligible.revision, a.revision);
    assert_eq!(eligible.project_revision, p.revision);
    assert_eq!(
        eligible.receipt_digest,
        allocation_digest(a.balance.as_ref().unwrap()).unwrap()
    );
    assert!(assigned.can_begin && assigned.capacity_ready && !assigned.can_rebase);
    let (gw, _) = gateway(HashMap::new());
    let mut response = TestClient::get(format!(
        "https://example.test/hagency/api/v1/management/projects/{}/setup",
        p.id
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(&f.outsider.token)
    .send(&svc(&f, gw))
    .await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let value: Value = response.take_json().await.unwrap();
    let current: ProjectReadiness = serde_json::from_value(value["readiness"].clone()).unwrap();
    assert_eq!(current.eligible_allocation, assigned.eligible_allocation);
    let mut old = value["readiness"].clone();
    old.as_object_mut().unwrap().remove("eligibleAllocation");
    assert!(
        serde_json::from_value::<ProjectReadiness>(old)
            .unwrap()
            .eligible_allocation
            .is_none()
    );
    let eligible = current.eligible_allocation.unwrap();
    let input = BeginProjectSetup {
        command_id: "begin_from_readiness".into(),
        allocation_id: Some(eligible.id.clone()),
        expected_project_revision: eligible.project_revision,
        expected_allocation_revision: eligible.revision,
        expected_authority_revision: current.administrator.unwrap().revision,
        mode: "create".into(),
    };
    let mut stale = input.clone();
    stale.expected_allocation_revision += 1;
    assert_eq!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, stale)
            .await
            .unwrap_err()
            .to_string(),
        "management_revision_changed"
    );
    let begun = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, input)
        .await
        .unwrap();
    assert_eq!(begun.eligible_allocation, Some(eligible.clone()));
    let binding = begun.setup.unwrap().current_binding.unwrap();
    assert_eq!(binding.allocation_id, eligible.id);
    assert_eq!(binding.allocation_receipt_digest, eligible.receipt_digest);
    let disabled = f
        .domain
        .management_project_setup(
            &f.outsider.p,
            &f.policy.clone().with_project_reservations(false),
            &p.id,
        )
        .await
        .unwrap();
    assert!(disabled.eligible_allocation.is_none());
    assert!(!disabled.capacity_ready && !disabled.can_begin && !disabled.can_rebase);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn eligible_allocation_drops_expired_revoked_superseded_and_unconfirmed_capacity() {
    for case in [
        "allocation_expired",
        "grant_expired",
        "grant_revoked",
        "resource_revoked",
        "project_superseded",
        "executor_rotated",
    ] {
        let (f, c, p, g, a, admin) = live().await;
        f.domain
            .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
            .await
            .unwrap();
        assert!(read(&f, &p).await.eligible_allocation.is_some());
        match case {
            "allocation_expired" | "grant_expired" | "project_superseded" => {
                let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
                let (query, id) = match case {
                    "allocation_expired" => (
                        "UPDATE hagency_agent_v1.management_allocations SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1",
                        &a.id,
                    ),
                    "grant_expired" => (
                        "UPDATE hagency_agent_v1.management_resource_grants SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1000 WHERE id=$1",
                        &g.id,
                    ),
                    _ => (
                        "UPDATE hagency_agent_v1.management_projects SET revision=revision+1 WHERE id=$1",
                        &p.id,
                    ),
                };
                sql_query(query)
                    .bind::<Text, _>(id)
                    .execute(&mut db)
                    .await
                    .unwrap();
            }
            "grant_revoked" => {
                f.domain
                    .revoke_management_grant(
                        &f.owner.p,
                        &f.policy,
                        &g.id,
                        RevokeInput {
                            command_id: "revoke_eligible_grant".into(),
                            expected_revision: g.revision,
                        },
                    )
                    .await
                    .unwrap();
            }
            "resource_revoked" => {
                f.domain
                    .revoke_contribution(
                        &f.owner.p,
                        &f.policy,
                        &c.contribution.id,
                        RevokeInput {
                            command_id: "revoke_eligible_resource".into(),
                            expected_revision: c.contribution.revision,
                        },
                    )
                    .await
                    .unwrap();
            }
            _ => {
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
        }
        let current = read(&f, &p).await;
        assert!(current.eligible_allocation.is_none(), "{case}");
        assert!(!current.capacity_ready && !current.can_begin && !current.can_rebase);
    }
    let (mut f, _, p, _, _) = prepared().await;
    f.policy = f.policy.with_project_setup(true);
    let current = read(&f, &p).await;
    assert!(current.eligible_allocation.is_none());
    assert!(!current.capacity_ready);
    let a = allocation(&f, &p).await;
    accepted(&f, &a).await;
    let current = read(&f, &p).await;
    assert!(current.eligible_allocation.is_none());
    assert!(!current.capacity_ready);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn begin_never_substitutes_a_replacement_allocation_with_the_same_revision() {
    let (f, c, p, g, original, admin) = live().await;
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let selected = read(&f, &p).await.eligible_allocation.unwrap();
    let stale = begin(&p, &original);
    f.domain
        .revoke_management_grant(
            &f.owner.p,
            &f.policy,
            &g.id,
            RevokeInput {
                command_id: "replace_selected_grant".into(),
                expected_revision: g.revision,
            },
        )
        .await
        .unwrap();
    let mut input = grant_input(&f, &c, &f.outsider.p).await;
    input.command_id = "publish_begin_replacement".into();
    input.receipt.ledger_revision = 20;
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, input)
        .await
        .unwrap();
    let replacement = allocation(&f, &p).await;
    f.domain
        .decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &replacement.request_id,
            approve("accept_begin_replacement", 1),
        )
        .await
        .unwrap();
    for revision in 21..=22 {
        let delivery = delivered(&f).await;
        let state = if delivery.command.operation == "project.release" {
            "released"
        } else {
            "reserved"
        };
        record(&f, &delivery, state, revision).await;
    }
    let replacement = allocation(&f, &p).await;
    assert_eq!(replacement.state, "reserved");
    assert_ne!(replacement.id, selected.id);
    assert_eq!(replacement.revision, selected.revision);
    assert_eq!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, stale)
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
    let current = read(&f, &p).await;
    assert!(current.setup.is_none());
    assert_eq!(current.eligible_allocation.unwrap().id, replacement.id);
    let begun = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &replacement))
        .await
        .unwrap();
    assert_eq!(begun.setup.unwrap().allocation_id, replacement.id);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn unpinned_legacy_begin_can_only_replay_its_original_recorded_receipt() {
    let (f, _, p, _, a, admin) = live().await;
    f.domain
        .assign_management_project_admin(&f.admin.p, &f.policy, &p.id, assign(&admin))
        .await
        .unwrap();
    let input = begin(&p, &a);
    let mut old = input.clone();
    old.allocation_id = None;
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct OldBegin<'a> {
        command_id: &'a str,
        expected_project_revision: i64,
        expected_allocation_revision: i64,
        expected_authority_revision: i64,
        mode: &'a str,
    }
    let legacy = OldBegin {
        command_id: &old.command_id,
        expected_project_revision: old.expected_project_revision,
        expected_allocation_revision: old.expected_allocation_revision,
        expected_authority_revision: old.expected_authority_revision,
        mode: &old.mode,
    };
    let exact_old_wire = serde_json::to_string(&(&p.id, &legacy)).unwrap();
    assert_eq!(
        serde_json::to_string(&(&p.id, &old)).unwrap(),
        exact_old_wire
    );
    assert_eq!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, old.clone())
            .await
            .unwrap_err()
            .to_string(),
        "project_setup_allocation_required"
    );
    assert!(read(&f, &p).await.setup.is_none());
    let result = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, input.clone())
        .await
        .unwrap();
    let original = result.setup.unwrap();
    // Reconstruct the exact pre-field command receipt. A wire upgrade may not
    // reinterpret this historical command as a newly selected allocation.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_commands SET digest=$3,result=(result::jsonb-'eligibleAllocation')::text WHERE actor_user_id=$1 AND command_id=$2")
        .bind::<Text,_>(&f.outsider.p.user_id).bind::<Text,_>(&old.command_id).bind::<Text,_>(crate::hash(&exact_old_wire)).execute(&mut db).await.unwrap();
    let replay = f
        .domain
        .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, old.clone())
        .await
        .unwrap();
    assert_eq!(replay.setup.unwrap().id, original.id);
    assert_eq!(
        f.domain
            .begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, input)
            .await
            .unwrap_err()
            .to_string(),
        "management_command_conflict"
    );
    let recovered = f
        .domain
        .recover_management_setup_command(
            &f.outsider.p,
            &f.policy,
            &old.command_id,
            RecoverCommandInput {
                operation: "project.setup.begin".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered["recorded"], true);
    assert_eq!(recovered["readiness"]["setup"]["id"], original.id);
    assert!(recovered.get("executionAllowed").is_none());
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_project_setups WHERE plan_id=$1").bind::<Text,_>(&p.id).get_result::<Matched>(&mut db).await.unwrap().matched);
}
