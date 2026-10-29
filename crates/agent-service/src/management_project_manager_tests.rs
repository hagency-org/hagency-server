use super::*;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn split_role_upgrade_is_atomic_fences_work_and_retains_history() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!("hagency_manager_merge_{}", &crate::hash(&secret_token())[..16]);
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control.batch_execute(&format!("CREATE DATABASE {name}")).await.unwrap();
    let mut url = Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move {
        let mut f = fixture_at(url.to_string()).await;
        f.policy = f.policy.with_project_reservations(true).with_project_setup(true).with_agent_admission(true).with_agent_executor(true);
        let c = admitted(&f).await;
        let p = approved_plan(&f, &c, &f.outsider.p).await;
        f.domain.publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, grant_input(&f, &c, &f.outsider.p).await).await.unwrap();
        let a = allocation(&f, &p).await;
        accepted(&f, &a).await;
        let d = delivered(&f).await;
        let a = record(&f, &d, "reserved", 11).await.allocation;
        let former = actor(&f.auth, "former_project_administrator").await;
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_project_manager_remove_test.sql")).await.unwrap();
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=21;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=21)").await.unwrap();
        // Reconstruct an actual v21 project before creating its immutable setup
        // and decisions, so historical actor identities are internally valid.
        sql_query("UPDATE hagency_agent_v1.management_project_authorities SET administrator_user_id=$2,assigned_by_user_id=$2 WHERE plan_id=$1").bind::<Text,_>(&p.id).bind::<Text,_>(&former.p.user_id).execute(&mut db).await.unwrap();
        // The current typed reader uses a private view. Supply it only while
        // constructing the historical fixture, then remove it before migration.
        db.batch_execute("CREATE VIEW hagency_agent_v1.management_agent_request_read AS SELECT r.*,NULL::text AS pool_contribution_id,NULL::text AS pool_grant_id FROM hagency_agent_v1.management_agent_requests r").await.unwrap();
        let s = f.domain.begin_management_project_setup(&f.outsider.p, &f.policy, &p.id, begin(&p, &a)).await.unwrap().setup.unwrap();
        let (ready, gw) = confirm_ok(&f, &s).await;
        let s = ready.setup.unwrap();
        let pending = request(&f, &p, &s, &gw).await;
        let approved = f.domain.request_managed_agent(&f.outsider.p, &f.policy, &p.id, input(&f, &p, &s, "second").await, &gw).await.unwrap();
        let approved = f.domain.decide_managed_agent(&former.p, &f.policy, &approved.id, decide(&approved, "old_approval"), &gw).await.unwrap();
        let terminal = f.domain.request_managed_agent(&f.outsider.p, &f.policy, &p.id, input(&f, &p, &s, "already_removed").await, &gw).await.unwrap();
        let terminal = f.domain.decide_managed_agent(&former.p, &f.policy, &terminal.id, decide(&terminal, "old_terminal_approval"), &gw).await.unwrap();
        f.domain.revoke_managed_agent_request(&former.p, &f.policy, &terminal.id, RevokeInput { command_id: "old_removal".into(), expected_revision: terminal.revision }).await.unwrap();
        // The existing terminal request already has a manager notification at
        // its latest revision. Merging must not collide with that durable row.
        let history_sql = "SELECT json_build_object('decision',d,'intent',i.payload,'binding',b)::text AS payload FROM hagency_agent_v1.management_agent_decisions d JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=d.request_id JOIN hagency_agent_v1.management_agent_requests r ON r.id=d.request_id JOIN hagency_agent_v1.management_setup_bindings b ON b.setup_id=r.setup_id AND b.revision=r.setup_binding_revision WHERE d.request_id=$1";
        let history = sql_query(history_sql).bind::<Text,_>(&approved.id).get_result::<Payload>(&mut db).await.unwrap().payload;
        db.batch_execute("DROP VIEW hagency_agent_v1.management_agent_request_read").await.unwrap();
        // Fail near the end, after all proposed authority/work updates. The
        // transaction must roll back every update and leave schema 21 usable.
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_projects ADD CONSTRAINT management_project_manager_key UNIQUE(id,manager_user_id)").await.unwrap();
        assert!(DomainStore::open(&f.url, "example.test", "_hagency_test_").await.is_err());
        assert!(sql_query("SELECT version=21 AND to_regclass('hagency_agent_v1.management_project_role_merges') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
        assert_eq!(f.domain.management_project_setup(&f.outsider.p, &f.policy, &p.id).await.unwrap().administrator.unwrap().mxid, former.p.mxid);
        assert!(sql_query("SELECT state='unavailable' AS matched FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1").bind::<Text,_>(&approved.id).get_result::<Check>(&mut db).await.unwrap().matched);
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_projects DROP CONSTRAINT management_project_manager_key").await.unwrap();
        for _ in 0..2 {
            let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
            let current = reopened.management_project_setup(&f.outsider.p, &f.policy, &p.id).await.unwrap();
            assert!(!current.can_assign_administrator && !current.matrix_ready && current.can_rebase);
            assert_eq!(current.administrator.unwrap().mxid, f.outsider.p.mxid);
            assert_eq!(current.setup.unwrap().administrator_mxid, former.p.mxid);
            let queued = reopened.management_agent_request(&f.outsider.p, &f.policy, &pending.id).await.unwrap();
            assert_eq!(queued.administrator_mxid, f.outsider.p.mxid);
            assert_eq!(queued.revision, pending.revision + 1);
            assert!(queued.can_reject && !queued.can_approve);
            assert!(reopened.decide_managed_agent(&former.p, &f.policy, &pending.id, decide(&queued, "former_denied"), &gw).await.is_err());
            assert!(reopened.management_agent_request(&former.p, &f.policy, &pending.id).await.is_err());
            let old = reopened.recover_managed_agent_command(&former.p, &f.policy, "old_approval", RecoverCommandInput { operation: "agent.decide".into() }).await.unwrap();
            assert_eq!(old["agentRequest"]["state"], "revoked");
            assert!(sql_query("SELECT state='fenced' AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands w WHERE w.intent_id=i.id AND w.operation='agent.fence') AS matched FROM hagency_agent_v1.management_agent_intents i WHERE request_id=$1").bind::<Text,_>(&approved.id).get_result::<Check>(&mut db).await.unwrap().matched);
            assert_eq!(sql_query(history_sql).bind::<Text,_>(&approved.id).get_result::<Payload>(&mut db).await.unwrap().payload, history);
            assert_eq!(allocation(&f, &p).await.revision, a.revision);
        }
        assert!(sql_query("UPDATE hagency_agent_v1.management_project_authorities SET administrator_user_id=$2 WHERE plan_id=$1").bind::<Text,_>(&p.id).bind::<Text,_>(&former.p.user_id).execute(&mut db).await.is_err());
        assert!(sql_query("DELETE FROM hagency_agent_v1.management_project_role_merges WHERE plan_id=$1").bind::<Text,_>(&p.id).execute(&mut db).await.is_err());
    }).await;
    control.batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)")).await.unwrap();
    result.unwrap();
}
