use super::*;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn resource_owner_is_the_only_new_capacity_coordinator() {
    let f = enabled_fixture().await;
    let other = actor(&f.auth, "former_coordinator").await;
    let mut input = offer(&f, "owner_resource");
    input.coordinator_mxid = other.p.mxid.clone();
    assert_eq!(f.domain.create_contribution(&f.owner.p, &f.policy, input).await.unwrap_err().to_string(), "resource_owner_is_coordinator");
    let c = admitted(&f).await;
    assert_eq!(c.contribution.owner_mxid, c.contribution.coordinator_mxid);
    let p = approved_plan(&f, &c, &f.outsider.p).await;
    let mut input = grant_input(&f, &c, &f.outsider.p).await;
    input.receipt.coordinator_mxid = other.p.mxid.clone();
    assert_eq!(f.domain.publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, input).await.unwrap_err().to_string(), "resource_grant_binding_mismatch");
    f.domain.publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, grant_input(&f, &c, &f.outsider.p).await).await.unwrap();
    let a = allocation(&f, &p).await;
    assert!(a.can_decide);
    assert_eq!(a.coordinator_mxid, f.owner.p.mxid);
    assert_eq!(f.domain.decide_management_allocation(&other.p, &f.policy, &a.request_id, approve("not_owner", 1)).await.unwrap_err().to_string(), "allocation_coordinator_required");
    assert_eq!(accepted(&f, &a).await.state, "approved_waiting_executor");
}

#[derive(QueryableByName)]
struct StoredRoleValue { #[diesel(sql_type=Text)] value: String }

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn owner_coordination_upgrade_preserves_grant_bytes_and_moves_pending_decisions() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name = format!("owner_coordination_{}", &crate::hash(&secret_token())[..16]);
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    control.batch_execute(&format!("CREATE DATABASE {name}")).await.unwrap();
    let mut url = url::Url::parse(&source).unwrap(); url.set_path(&format!("/{name}"));
    let result = tokio::spawn(async move {
        let mut f = fixture_at(url.to_string()).await;
        f.policy = f.policy.with_project_reservations(true);
        let former = actor(&f.auth, "former_coordinator").await;
        let c = admitted(&f).await;
        let p = approved_plan(&f, &c, &f.outsider.p).await;
        let grant = f.domain.publish_management_grant(&f.owner.p, &f.policy, &c.contribution.id, grant_input(&f, &c, &f.outsider.p).await).await.unwrap();
        let a = allocation(&f, &p).await;
        let mut legacy = grant.receipt;
        legacy.coordinator_mxid = former.p.mxid.clone();
        let raw = serde_json::to_string(&legacy).unwrap();
        let digest = allocation_digest(&legacy).unwrap();
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_owner_coordinator_remove_test.sql")).await.unwrap();
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=22;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=22)").await.unwrap();
        sql_query("UPDATE hagency_agent_v1.management_contributions SET coordinator_mxid=$2 WHERE id=$1").bind::<Text,_>(&c.contribution.id).bind::<Text,_>(&former.p.mxid).execute(&mut db).await.unwrap();
        sql_query("UPDATE hagency_agent_v1.management_resource_grants SET receipt=$2,receipt_digest=$3 WHERE id=$1").bind::<Text,_>(&legacy.grant_id).bind::<Text,_>(&raw).bind::<Text,_>(&digest).execute(&mut db).await.unwrap();
        sql_query("UPDATE hagency_agent_v1.management_requests SET approver_mxid=$2 WHERE id=$1").bind::<Text,_>(&a.request_id).bind::<Text,_>(&former.p.mxid).execute(&mut db).await.unwrap();
        // Force a late error and verify that authority/history changes roll back.
        db.batch_execute("ALTER TABLE hagency_agent_v1.users ADD CONSTRAINT management_owner_mxid_key UNIQUE(id,mxid)").await.unwrap();
        assert!(DomainStore::open(&f.url, "example.test", "_hagency_test_").await.is_err());
        assert_eq!(sql_query("SELECT coordinator_mxid AS value FROM hagency_agent_v1.management_contributions WHERE id=$1").bind::<Text,_>(&c.contribution.id).get_result::<StoredRoleValue>(&mut db).await.unwrap().value, former.p.mxid);
        db.batch_execute("ALTER TABLE hagency_agent_v1.users DROP CONSTRAINT management_owner_mxid_key").await.unwrap();
        for _ in 0..2 {
            let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
            let current = reopened.management_allocation_detail(&f.owner.p, &f.policy, &a.id).await.unwrap();
            assert!(current.allocation.can_decide);
            assert_eq!(current.allocation.coordinator_mxid, f.owner.p.mxid);
            assert_eq!(current.request.approver_mxid, f.owner.p.mxid);
            assert_eq!(current.request.revision, 2);
            assert!(reopened.management_allocation_detail(&former.p, &f.policy, &a.id).await.is_err());
            assert_eq!(sql_query("SELECT receipt AS value FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&legacy.grant_id).get_result::<StoredRoleValue>(&mut db).await.unwrap().value, raw);
        }
        let current = f.domain.management_request(&f.owner.p, &f.policy, &a.request_id).await.unwrap();
        f.domain.decide_management_allocation(&f.owner.p, &f.policy, &a.request_id, approve("owner_decision", current.revision)).await.unwrap();
        let delivery = delivered(&f).await;
        assert_eq!(delivery.command.decision_actor, f.owner.p.mxid);
        assert_eq!(delivery.command.grant_receipt.coordinator_mxid, former.p.mxid);
        assert_eq!(allocation_digest(&delivery.command.grant_receipt).unwrap(), digest);
        assert!(sql_query("UPDATE hagency_agent_v1.management_contributions SET coordinator_mxid=$2 WHERE id=$1").bind::<Text,_>(&c.contribution.id).bind::<Text,_>(&former.p.mxid).execute(&mut db).await.is_err());
        assert!(sql_query("DELETE FROM hagency_agent_v1.management_owner_coordination_merges WHERE contribution_id=$1").bind::<Text,_>(&c.contribution.id).execute(&mut db).await.is_err());
    }).await;
    control.batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)")).await.unwrap();
    result.unwrap();
}
