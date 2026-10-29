use super::*;
fn rebase(s: &ProjectSetup, allocation: &ProjectAllocation, authority: i64) -> RebaseProjectSetup {
    RebaseProjectSetup {
        command_id: "rebase_setup".into(),
        expected_revision: s.revision,
        allocation_id: allocation.id.clone(),
        expected_project_revision: allocation.project_revision,
        expected_allocation_revision: allocation.revision,
        expected_authority_revision: authority,
    }
}
fn cancel(s: &ProjectSetup) -> CancelProjectSetup {
    CancelProjectSetup {
        command_id: "cancel_setup".into(),
        expected_revision: s.revision,
    }
}
fn same_identity(a: &ProjectSetup, b: &ProjectSetup) {
    assert_eq!(a.id, b.id);
    assert_eq!(a.project_id, b.project_id);
    assert_eq!(a.allocation_id, b.allocation_id);
    assert_eq!(a.allocation_receipt_digest, b.allocation_receipt_digest);
    assert_eq!(a.authority_revision, b.authority_revision);
    assert_eq!(a.definition_digest, b.definition_digest);
    assert_eq!(a.administrator_mxid, b.administrator_mxid);
    assert_eq!(a.space_creation_command_id, b.space_creation_command_id);
    assert_eq!(a.room_creation_command_id, b.room_creation_command_id);
    assert_eq!(a.space_id, b.space_id);
    assert_eq!(a.room_id, b.room_id);
    assert_eq!(a.domain_project_id, b.domain_project_id);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cancel_rebase_http_preserves_identity_holds_and_managed_binding_fence() {
    let (f, _, p, s, _admin) = begun().await;
    let (done, gw) = confirm_ok(&f, &s).await;
    let original = done.setup.unwrap();
    let a = allocation(&f, &p).await;
    let service = svc(&f, gw);
    let path = format!("project-setups/{}/cancel", s.id);
    for actor in [&f.coordinator, &f.owner] {
        post(
            &service,
            &path,
            &actor.token,
            serde_json::to_value(cancel(&original)).unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let mut injected = serde_json::to_value(cancel(&original)).unwrap();
    injected["releaseTokens"] = json!(true);
    post(
        &service,
        &path,
        &f.outsider.token,
        injected,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let response = post(
        &service,
        &path,
        &f.outsider.token,
        serde_json::to_value(cancel(&original)).unwrap(),
        StatusCode::OK,
    )
    .await;
    let cancelled: ProjectReadiness =
        serde_json::from_value(response["readiness"].clone()).unwrap();
    assert!(!cancelled.matrix_ready && !cancelled.can_confirm && cancelled.can_rebase);
    assert!(cancelled.capacity_ready);
    assert_eq!(
        cancelled.blocking_reason.as_deref(),
        Some("project_setup_cancelled")
    );
    let cancelled = cancelled.setup.unwrap();
    same_identity(&original, &cancelled);
    assert!(cancelled.verified_at_ms.is_none());
    let held = allocation(&f, &p).await;
    assert_eq!(held.revision, a.revision);
    assert_eq!(held.state, "reserved");
    assert_eq!(
        serde_json::to_value(&held.balance).unwrap(),
        serde_json::to_value(&a.balance).unwrap()
    );
    let agent = personal_agent(&f).await;
    assert_eq!(
        bind(
            &f,
            &agent,
            original.domain_project_id.as_deref().unwrap(),
            original.space_id.as_deref().unwrap(),
            original.room_id.as_deref().unwrap(),
            "cancelled_bypass"
        )
        .await
        .unwrap_err()
        .to_string(),
        "managed_agent_admission_required"
    );
    let recovered = post(
        &service,
        "project-setup-commands/confirm_project_setup/recover",
        &f.outsider.token,
        json!({"operation":"project.setup.confirm"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["readiness"]["setup"]["state"], "cancelled");
    assert_eq!(recovered["readiness"]["canConfirm"], false);
    assert!(recovered.get("executionAllowed").is_none());
    let rebase_path = format!("project-setups/{}/rebase", s.id);
    let input = rebase(&cancelled, &a, 1);
    for actor in [&f.admin, &f.coordinator, &f.owner] {
        post(
            &service,
            &rebase_path,
            &actor.token,
            serde_json::to_value(&input).unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let result = post(
        &service,
        &rebase_path,
        &f.outsider.token,
        serde_json::to_value(&input).unwrap(),
        StatusCode::OK,
    )
    .await;
    let latest: ProjectReadiness = serde_json::from_value(result["readiness"].clone()).unwrap();
    assert!(!latest.matrix_ready && latest.can_confirm);
    let rebased = latest.setup.unwrap();
    same_identity(&original, &rebased);
    assert_eq!(rebased.current_binding.as_ref().unwrap().revision, 2);
    // Original cancellation replay must show resumed state and not cancel it again.
    let replay = post(
        &service,
        &path,
        &f.outsider.token,
        serde_json::to_value(cancel(&original)).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay["readiness"]["setup"]["revision"], rebased.revision);
    let mut fresh = confirm(&rebased);
    fresh.command_id = "confirm_after_rebase".into();
    let result = post(
        &service,
        &format!("project-setups/{}/confirm", s.id),
        &f.outsider.token,
        serde_json::to_value(fresh).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result["readiness"]["matrixReady"], true);
    post(
        &service,
        &path,
        &f.admin.token,
        serde_json::to_value(CancelProjectSetup {
            command_id: "server_admin_cancel".into(),
            expected_revision: rebased.revision + 1,
        })
        .unwrap(),
        StatusCode::OK,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn manager_authority_rebase_reuses_original_marker_and_requires_current_membership() {
    let (f, _, p, s, old_admin) = begun().await;
    let (done, _) = confirm_ok(&f, &s).await;
    let original = done.setup.unwrap();
    let mut inactive_db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.users SET active=false WHERE id=$1")
        .bind::<Text, _>(&old_admin.p.user_id)
        .execute(&mut inactive_db)
        .await
        .unwrap();
    assert!(f.domain.management_project_setup(&f.outsider.p, &f.policy, &p.id).await.is_err());
    sql_query("UPDATE hagency_agent_v1.users SET active=true WHERE id=$1")
        .bind::<Text, _>(&old_admin.p.user_id)
        .execute(&mut inactive_db)
        .await
        .unwrap();
    let replacement = same_actor(&f.auth, &f.outsider).await;
    let changed = invalidate_authority(&f, &p.id).await;
    assert!(!changed.matrix_ready && !changed.can_confirm);
    assert_eq!(
        changed.blocking_reason.as_deref(),
        Some("project_administrator_changed")
    );
    let changed = changed.setup.unwrap();
    same_identity(&original, &changed);
    for denied in [&f.owner.p, &f.coordinator.p] {
        assert_eq!(
            f.domain
                .cancel_management_project_setup(denied, &f.policy, &s.id, cancel(&changed))
                .await
                .unwrap_err()
                .to_string(),
            "project_setup_cancel_denied"
        );
    }
    let a = allocation(&f, &p).await;
    let rebased = f
        .domain
        .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&changed, &a, 2))
        .await
        .unwrap()
        .setup
        .unwrap();
    same_identity(&original, &rebased);
    assert_eq!(
        rebased.current_binding.as_ref().unwrap().administrator_mxid,
        replacement.p.mxid
    );
    let mut input = confirm(&rebased);
    input.command_id = "confirm_current_admin".into();
    let mut missing_manager = states(&s, &input);
    for events in missing_manager.values_mut() {
        events.retain(|e| !(e["type"] == "m.room.member" && e["state_key"] == old_admin.p.mxid));
    }
    let (gw, _) = gateway(missing_manager);
    assert!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, input.clone(), &gw)
            .await
            .is_err()
    );
    let mut map = states(&s, &input);
    for events in map.values_mut() {
        events.retain(|e| !(e["type"] == "m.room.member" && e["state_key"] == old_admin.p.mxid));
        events.push(json!({"type":"m.room.member","state_key":replacement.p.mxid,"content":{"membership":"join"}}));
    }
    let (gw, _) = gateway(map);
    let result = f
        .domain
        .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, input, &gw)
        .await
        .unwrap();
    assert!(result.matrix_ready);
    same_identity(&original, result.setup.as_ref().unwrap());
    // Updating/repointing history cannot restore old administrator authority.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("DELETE FROM hagency_agent_v1.management_setup_bindings WHERE setup_id=$1 AND revision=2").bind::<Text,_>(&s.id).execute(&mut db).await.is_err());
    assert!(sql_query("UPDATE hagency_agent_v1.management_project_setups SET current_binding_revision=1 WHERE id=$1").bind::<Text,_>(&s.id).execute(&mut db).await.is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn in_flight_confirmation_is_fenced_by_cancel_rebase_or_admin_replacement() {
    for transition in 0..3 {
        let (f, _, p, s, _) = begun().await;
        let input = confirm(&s);
        let map = Arc::new(states(&s, &input));
        let a = allocation(&f, &p).await;
        let started = Arc::new(tokio::sync::Barrier::new(3));
        let release = Arc::new(tokio::sync::Barrier::new(3));
        let gateway = Gateway::new(
            Arc::new({
                let started = started.clone();
                let release = release.clone();
                move |room| {
                    let map = map.clone();
                    let started = started.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        started.wait().await;
                        release.wait().await;
                        Ok(map.get(&room).unwrap().clone())
                    })
                }
            }),
            SERVICE.into(),
        );
        let domain = DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap();
        let manager = f
            .auth
            .authenticate(&f.outsider.token, now_ms(), true)
            .await
            .unwrap();
        let policy = f.policy.clone();
        let id = s.id.clone();
        let checking = tokio::spawn(async move {
            domain
                .confirm_management_project_setup(&manager, &policy, &id, input, &gateway)
                .await
        });
        started.wait().await;
        let changed = match transition {
            0 => f
                .domain
                .cancel_management_project_setup(&f.outsider.p, &f.policy, &s.id, cancel(&s))
                .await
                .unwrap(),
            1 => f
                .domain
                .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &a, 1))
                .await
                .unwrap(),
            _ => invalidate_authority(&f, &p.id).await,
        };
        release.wait().await;
        assert_eq!(
            checking.await.unwrap().unwrap_err().to_string(),
            "management_revision_changed"
        );
        let now = read(&f, &p).await;
        assert!(!now.matrix_ready);
        let latest = now.setup.unwrap();
        same_identity(&s, &latest);
        assert_eq!(latest.revision, changed.setup.unwrap().revision);
        assert!(latest.domain_project_id.is_none());
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn rebase_uses_distinct_confirmed_replacement_without_rewriting_original_allocation() {
    let (f, c, p, s, _) = begun().await;
    let original = confirm_ok(&f, &s).await.0.setup.unwrap();
    let old = allocation(&f, &p).await;
    let grant = f
        .domain
        .management_resource_grant(&f.owner.p, &f.policy, &old.grant_id)
        .await
        .unwrap();
    f.domain
        .revoke_management_grant(
            &f.owner.p,
            &f.policy,
            &grant.id,
            RevokeInput {
                command_id: "retire_original_grant".into(),
                expected_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    let stale = read(&f, &p).await;
    assert!(!stale.capacity_ready && !stale.can_confirm);
    assert!(stale.eligible_allocation.is_none());
    let s = stale.setup.unwrap();
    assert_eq!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &old, 1))
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
    let mut input = grant_input(&f, &c, &f.outsider.p).await;
    input.command_id = "publish_replacement_grant".into();
    input.receipt.ledger_revision = 20;
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, input)
        .await
        .unwrap();
    let new = allocation(&f, &p).await;
    assert_ne!(old.id, new.id);
    assert!(read(&f, &p).await.eligible_allocation.is_none());
    assert_eq!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &new, 1))
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
    f.domain
        .decide_management_allocation(
            &f.coordinator.p,
            &f.policy,
            &new.request_id,
            approve("accept_replacement_capacity", 1),
        )
        .await
        .unwrap();
    for revision in 21..=22 {
        let d = delivered(&f).await;
        let state = if d.command.operation == "project.release" {
            "released"
        } else {
            "reserved"
        };
        record(&f, &d, state, revision).await;
    }
    let new = allocation(&f, &p).await;
    assert_eq!(new.state, "reserved");
    let ready_to_rebase = read(&f, &p).await;
    assert!(!ready_to_rebase.capacity_ready && ready_to_rebase.can_rebase);
    let eligible = ready_to_rebase.eligible_allocation.unwrap();
    assert_eq!(eligible.id, new.id);
    assert_eq!(eligible.revision, new.revision);
    assert_eq!(eligible.project_revision, p.revision);
    assert_eq!(
        eligible.receipt_digest,
        allocation_digest(new.balance.as_ref().unwrap()).unwrap()
    );
    let rebased = f
        .domain
        .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &new, 1))
        .await
        .unwrap();
    let current = rebased.setup.unwrap();
    assert_eq!(rebased.eligible_allocation, Some(eligible));
    same_identity(&original, &current);
    assert_eq!(
        current.current_binding.as_ref().unwrap().allocation_id,
        new.id
    );
    assert!(!rebased.matrix_ready && rebased.capacity_ready);
    let mut confirm = confirm(&current);
    confirm.command_id = "confirm_replacement_capacity".into();
    let (gw, _) = gateway(states(&s, &confirm));
    assert!(
        f.domain
            .confirm_management_project_setup(&f.outsider.p, &f.policy, &s.id, confirm, &gw)
            .await
            .unwrap()
            .matrix_ready
    );
    let unchanged = allocation(&f, &p).await;
    assert_eq!(unchanged.revision, new.revision);
    assert_eq!(
        serde_json::to_value(unchanged.balance).unwrap(),
        serde_json::to_value(new.balance).unwrap()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn rebase_rejects_foreign_capacity_stale_revisions_inactive_admin_and_revoked_plan() {
    let (f, _, p, s, admin) = begun().await;
    let a = allocation(&f, &p).await;
    let base = rebase(&s, &a, 1);
    for field in 0..4 {
        let mut stale = base.clone();
        match field {
            0 => stale.expected_revision += 1,
            1 => stale.expected_project_revision += 1,
            2 => stale.expected_allocation_revision += 1,
            _ => stale.expected_authority_revision += 1,
        };
        assert_eq!(
            f.domain
                .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, stale)
                .await
                .unwrap_err()
                .to_string(),
            "management_revision_changed"
        );
    }
    let (mut foreign, _, _, _, fa) = prepared().await;
    foreign.policy = foreign.policy.with_project_setup(true);
    accepted(&foreign, &fa).await;
    let d = delivered(&foreign).await;
    let fa = record(&foreign, &d, "reserved", 11).await.allocation;
    let mut wrong = base.clone();
    wrong.allocation_id = fa.id;
    wrong.expected_allocation_revision = fa.revision;
    assert_eq!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, wrong)
            .await
            .unwrap_err()
            .to_string(),
        "live_reservation_required"
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.users SET active=false WHERE id=$1")
        .bind::<Text, _>(&admin.p.user_id)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, base.clone())
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
    f.domain
        .revoke_management_project(
            &f.outsider.p,
            &f.policy,
            &p.id,
            RevokeInput {
                command_id: "revoke_before_rebase".into(),
                expected_revision: p.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, base)
            .await
            .unwrap_err()
            .to_string(),
        "project_unavailable"
    );
    let cancelled = f
        .domain
        .cancel_management_project_setup(&f.outsider.p, &f.policy, &s.id, cancel(&s))
        .await
        .unwrap();
    assert!(!cancelled.can_rebase);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn recovery_transitions_rollback_history_and_receipts_when_outbox_fails() {
    let (f, _, p, s, _) = begun().await;
    let a = allocation(&f, &p).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let function = format!(
        "fail_setup_recovery_{}",
        &crate::hash(&secret_token())[..16]
    );
    db.batch_execute(&format!("CREATE FUNCTION hagency_agent_v1.{function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_mxid='{}' THEN RAISE EXCEPTION 'injected_setup_recovery_failure'; END IF; RETURN NEW; END; $$; CREATE TRIGGER {function} BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.{function}();",f.outsider.p.mxid)).await.unwrap();
    assert!(
        f.domain
            .cancel_management_project_setup(&f.outsider.p, &f.policy, &s.id, cancel(&s))
            .await
            .is_err()
    );
    assert!(
        f.domain
            .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &a, 1))
            .await
            .is_err()
    );
    let unchanged = read(&f, &p).await.setup.unwrap();
    assert_eq!(unchanged.revision, s.revision);
    assert_eq!(unchanged.current_binding.unwrap().revision, 1);
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_setup_bindings WHERE setup_id=$1").bind::<Text,_>(&s.id).get_result::<Matched>(&mut db).await.unwrap().matched);
    for (id, operation) in [
        ("cancel_setup", "project.setup.cancel"),
        ("rebase_setup", "project.setup.rebase"),
    ] {
        let r = f
            .domain
            .recover_management_setup_command(
                &f.outsider.p,
                &f.policy,
                id,
                RecoverCommandInput {
                    operation: operation.into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(r["recorded"], false);
    }
    db.batch_execute(&format!("DROP TRIGGER {function} ON hagency_agent_v1.management_outbox;DROP FUNCTION hagency_agent_v1.{function}()")).await.unwrap();
    let rebased = f
        .domain
        .rebase_management_project_setup(&f.outsider.p, &f.policy, &s.id, rebase(&s, &a, 1))
        .await
        .unwrap();
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
    let current = f.auth.authenticate(&token, now_ms(), true).await.unwrap();
    let r = f
        .domain
        .recover_management_setup_command(
            &current,
            &f.policy,
            "rebase_setup",
            RecoverCommandInput {
                operation: "project.setup.rebase".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(r["recorded"], true);
    assert_eq!(
        r["readiness"]["setup"]["revision"],
        rebased.setup.unwrap().revision
    );
    assert!(r.get("executionAllowed").is_none());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn v6_recovery_migration_is_atomic_and_preserves_ready_setup_and_old_receipts() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!(
        "hagency_setup_recovery_upgrade_{}",
        &crate::hash(&secret_token())[..16]
    );
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
        let auth=Store::open(url.as_str(),"example.test","https://example.test/auth/").await.unwrap();let domain=DomainStore::open(url.as_str(),"example.test","_hagency_test_").await.unwrap();
        let owner=actor(&auth,"old_owner").await;let admin=actor(&auth,"old_admin").await;let coordinator=owner_coordinator_device(&auth,&owner).await;let outsider=actor(&auth,"old_manager").await;let assigned=same_actor(&auth,&outsider).await;
        let policy=ManagementPolicy::new(admin.p.mxid.clone(),"example.test").unwrap().with_project_reservations(true).with_project_setup(true);
        let f=Fixture{auth,domain,url:url.to_string(),policy,owner,admin,coordinator,outsider};
        let c=admitted(&f).await;let p=approved_plan(&f,&c,&f.outsider.p).await;let input=grant_input(&f,&c,&f.outsider.p).await;f.domain.publish_management_grant(&f.owner.p,&f.policy,&c.contribution.id,input).await.unwrap();let a=allocation(&f,&p).await;accepted(&f,&a).await;let d=delivered(&f).await;let a=record(&f,&d,"reserved",11).await.allocation;
        f.domain.assign_management_project_admin(&f.admin.p,&f.policy,&p.id,assign(&assigned)).await.unwrap();let setup=f.domain.begin_management_project_setup(&f.outsider.p,&f.policy,&p.id,begin(&p,&a)).await.unwrap().setup.unwrap();let original=confirm_ok(&f,&setup).await.0.setup.unwrap();
        let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
        super::agents::remove_migration(&mut db).await;
        // Reconstruct the exact v6 storage shape from a fully validated setup,
        // including old command receipts without the newly additive fields.
        db.batch_execute("DROP TRIGGER advance_setup_binding ON hagency_agent_v1.management_project_setups;DROP FUNCTION hagency_agent_v1.advance_setup_binding();ALTER TABLE hagency_agent_v1.management_project_setups DROP CONSTRAINT management_current_setup_binding;ALTER TABLE hagency_agent_v1.management_project_setups DROP COLUMN current_binding_revision;DROP TABLE hagency_agent_v1.management_setup_bindings;DROP FUNCTION hagency_agent_v1.retain_setup_binding();ALTER TABLE hagency_agent_v1.management_project_setups DROP CONSTRAINT management_project_setups_state_check;ALTER TABLE hagency_agent_v1.management_project_setups ADD CONSTRAINT management_project_setups_state_check CHECK(state IN ('awaiting_matrix_setup','ready'));ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=6;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=6)").await.unwrap();
        sql_query("UPDATE hagency_agent_v1.management_commands SET result=jsonb_set((result::jsonb-'canCancel'-'canRebase'),'{setup}',(result::jsonb->'setup')-'currentBinding')::text WHERE actor_user_id=$1 AND operation IN ('project.setup.begin','project.setup.confirm')").bind::<Text,_>(&f.outsider.p.user_id).execute(&mut db).await.unwrap();
        #[derive(QueryableByName)]struct Stored {#[diesel(sql_type=Text)] value:String}
        let old=sql_query("SELECT result AS value FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND operation='project.setup.confirm'").bind::<Text,_>(&f.outsider.p.user_id).get_result::<Stored>(&mut db).await.unwrap().value;
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_project_setups ADD COLUMN current_binding_revision bigint").await.unwrap();
        assert!(DomainStore::open(&f.url,"example.test","_hagency_test_").await.is_err());
        assert!(sql_query("SELECT version=6 AND to_regclass('hagency_agent_v1.management_setup_bindings') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Matched>(&mut db).await.unwrap().matched);
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_project_setups DROP COLUMN current_binding_revision").await.unwrap();
        for _ in 0..2 {
            let reopened=DomainStore::open(&f.url,"example.test","_hagency_test_").await.unwrap();let current=reopened.management_project_setup(&f.outsider.p,&f.policy,&p.id).await.unwrap();same_identity(&original,current.setup.as_ref().unwrap());let binding=current.setup.unwrap().current_binding.unwrap();assert_eq!(binding.revision,1);assert_eq!(binding.allocation_id,original.allocation_id);assert_eq!(binding.administrator_mxid,original.administrator_mxid);
            let bytes=sql_query("SELECT result AS value FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND operation='project.setup.confirm'").bind::<Text,_>(&f.outsider.p.user_id).get_result::<Stored>(&mut db).await.unwrap().value;assert_eq!(bytes,old);
            let recovered=reopened.recover_management_setup_command(&f.outsider.p,&f.policy,"confirm_project_setup",RecoverCommandInput{operation:"project.setup.confirm".into()}).await.unwrap();assert_eq!(recovered["recorded"],true);assert_eq!(recovered["readiness"]["setup"]["currentBinding"]["revision"],1);
            assert!(sql_query("SELECT version=28 AS matched FROM hagency_agent_v1.management_deployment").get_result::<Matched>(&mut db).await.unwrap().matched);
        }
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[test]
fn setup_recovery_fixture_keeps_creation_definition_separate_from_current_binding() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-setup-recovery-v1.json"
    ))
    .unwrap();
    let cancel: CancelProjectSetup = serde_json::from_value(fixture["cancel"].clone()).unwrap();
    let rebase: RebaseProjectSetup = serde_json::from_value(fixture["rebase"].clone()).unwrap();
    let binding: SetupBinding = serde_json::from_value(fixture["currentBinding"].clone()).unwrap();
    assert_eq!(allocation_digest(&cancel).unwrap(), fixture["cancelDigest"]);
    assert_eq!(allocation_digest(&rebase).unwrap(), fixture["rebaseDigest"]);
    assert_eq!(
        allocation_digest(&fixture["creationDefinition"]).unwrap(),
        fixture["creationDefinitionDigest"]
    );
    assert_eq!(
        fixture["originalRoomMarker"]["definitionDigest"],
        fixture["creationDefinitionDigest"]
    );
    assert_ne!(
        binding.administrator_mxid,
        fixture["creationDefinition"]["administratorMxid"]
    );
    assert_ne!(
        binding.allocation_id,
        fixture["creationDefinition"]["allocationId"]
    );
    let mut forbidden = fixture["rebase"].clone();
    forbidden["allowCreateRoom"] = json!(true);
    assert!(serde_json::from_value::<RebaseProjectSetup>(forbidden).is_err());
}
