use super::*;
use crate::recurring_quota::{Calendar, Cadence};

async fn configured() -> Fixture {
    let mut f = pool_agents::isolated_fixture().await;
    f.policy = f.policy.with_agent_admission(true).with_agent_executor(true)
        .with_pool_grant_succession(true).with_parent_settlement(true);
    f
}
async fn draft(f: &Fixture) -> Admission {
    let mut input = offer(f, "publication_first");
    input.publication_first = true;
    f.domain.create_contribution(&f.owner.p, &f.policy, input).await.unwrap()
}
async fn terms(f: &Fixture, a: &Admission) -> PublishGrantInput {
    let mut g = grant_input(f, a, &f.outsider.p).await;
    let calendar = Calendar {cadence: Cadence::Monthly, time_zone: "America/Los_Angeles".into()};
    let period = calendar.period_at(now_ms() as u64).unwrap();
    g.receipt.version = 5;
    g.receipt.calendar = Some(calendar);
    g.receipt.admission_request_id = Some(a.request.id.clone());
    g.receipt.allocated_tokens = 500_000_000;
    g.receipt.per_request_tokens = Some(50_000_000);
    g.receipt.period_key = period.key;
    g.receipt.period_start_ms = period.start_ms as i64;
    g.receipt.period_end_ms = period.end_ms as i64;
    g.receipt.delegation_expires_at_ms = period.end_ms as i64;
    g
}
async fn actionable(f: &Fixture, id: &str) -> bool {
    f.domain.management_inbox(&f.admin.p, &f.policy, ManagementQuery {needs_my_action: true, ..Default::default()})
        .await.unwrap().items.iter().any(|r|r.id==id)
}
async fn visible_pool(f: &Fixture, id: &str) -> bool {
    f.domain.resource_pools(&f.outsider.p, &f.policy, CatalogQuery {after: None, capability: None})
        .await.unwrap().items.iter().any(|p|p.id==id)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_renewal_policy_is_owner_authority_and_requesters_cannot_override_it() {
    for mode in [crate::recurring_quota::RenewalMode::Owner, crate::recurring_quota::RenewalMode::Automatic] {
        let f = configured().await;
        let a = draft(&f).await;
        let mut input = terms(&f, &a).await;
        input.receipt.version = 6;
        input.receipt.renewal_mode = Some(mode);
        f.domain.publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input).await.unwrap();
        let a = f.domain.decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("admit", 1)).await.unwrap();
        let catalog = f.domain.resource_pools(&f.outsider.p, &f.policy, CatalogQuery {after:None,capability:None}).await.unwrap();
        assert_eq!(catalog.items[0].renewal_mode, Some(mode));
        let request = pool_agents::input(&a, "inherit_policy");
        assert!(request.renewal_mode.is_none());
        let r = f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, request.clone()).await.unwrap();
        assert_eq!(r.renewal_mode, Some(mode));
        assert_eq!(f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, request).await.unwrap().id, r.id);
        let mut forged = pool_agents::input(&a, "override_policy");
        forged.renewal_mode = Some(if mode == crate::recurring_quota::RenewalMode::Owner {crate::recurring_quota::RenewalMode::Automatic} else {crate::recurring_quota::RenewalMode::Owner});
        assert!(matches!(f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, forged).await,
            Err(crate::Error::Conflict("pool_renewal_policy_changed"))));
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn publication_first_one_owner_confirmation_then_admin_approval_makes_capacity_requestable() {
    let f = configured().await;
    let a = draft(&f).await;
    assert_eq!(a.contribution.state, "draft");
    assert_eq!(a.request.state, "draft");
    assert!(!actionable(&f, &a.request.id).await);
    assert!(!visible_pool(&f, &a.contribution.id).await);
    assert!(f.domain.decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("premature", 1)).await.is_err());
    let grant = terms(&f, &a).await;
    // Capacity and the admin inbox must commit together. A notification write
    // failure leaves a private draft, and the exact original submission retries.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fail_publication_inbox() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected_publication_failure'; END; $$; CREATE TRIGGER fail_publication_inbox BEFORE INSERT ON hagency_agent_v1.management_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fail_publication_inbox();").await.unwrap();
    assert!(f.domain.publish_management_grant(&f.owner.p,&f.policy,&a.contribution.id,serde_json::from_value(serde_json::to_value(&grant).unwrap()).unwrap()).await.is_err());
    assert_eq!(f.domain.contribution(&f.owner.p,&f.policy,&a.contribution.id).await.unwrap().contribution.state,"draft");
    assert!(!actionable(&f,&a.request.id).await);
    db.batch_execute("DROP TRIGGER fail_publication_inbox ON hagency_agent_v1.management_outbox; DROP FUNCTION hagency_agent_v1.fail_publication_inbox();").await.unwrap();
    let server = service(&f, true);
    let path = format!("contributions/{}/grants", a.contribution.id);
    let published = post(&server, &path, &f.owner.token, serde_json::to_value(&grant).unwrap(), StatusCode::OK).await;
    let pending = f.domain.contribution(&f.owner.p, &f.policy, &a.contribution.id).await.unwrap();
    assert_eq!(pending.contribution.state, "pending_admission");
    assert!(actionable(&f, &a.request.id).await);
    assert!(!visible_pool(&f, &a.contribution.id).await);
    assert!(f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, pool_agents::input(&pending, "before_approval")).await.is_err());
    let approved = post(&server, &format!("requests/{}/decide",a.request.id), &f.admin.token,
        json!({"commandId":"approve_once","expectedRevision":1,"decision":"approve"}), StatusCode::OK).await;
    let a: Admission = serde_json::from_value(approved["admission"].clone()).unwrap();
    assert_eq!(a.contribution.state, "admitted");
    assert!(visible_pool(&f, &a.contribution.id).await);
    let pools = f.domain.resource_pools(&f.outsider.p, &f.policy, CatalogQuery {after:None,capability:None}).await.unwrap();
    let pool = pools.items.iter().find(|p|p.id==a.contribution.id).unwrap();
    assert_eq!(pool.published_tokens, Some(500_000_000));
    assert_eq!(pool.per_request_tokens, Some(50_000_000));
    let mut request=pool_agents::input(&a, "after_approval");
    request.renewal_mode=Some(crate::recurring_quota::RenewalMode::Owner);
    f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, request).await.unwrap();
    // A lost owner reply is recovered using the exact original request even
    // though admin approval has since advanced the contribution revision.
    let replay = post(&server, &path, &f.owner.token, serde_json::to_value(&grant).unwrap(), StatusCode::OK).await;
    assert_eq!(replay["grant"]["id"], published["grant"]["id"]);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn publication_first_rejection_revokes_capacity_and_never_exposes_the_pool() {
    let f = configured().await;
    let a = draft(&f).await;
    let g = f.domain.publish_management_grant(&f.owner.p,&f.policy,&a.contribution.id,terms(&f,&a).await).await.unwrap();
    let result = f.domain.decide_management_request(&f.admin.p,&f.policy,&a.request.id,
        DecisionInput {command_id:"reject_once".into(),expected_revision:1,decision:Decision::Reject}).await.unwrap();
    assert_eq!(result.contribution.state,"rejected");
    assert!(!visible_pool(&f,&a.contribution.id).await);
    assert_eq!(f.domain.management_resource_grant(&f.owner.p,&f.policy,&g.id).await.unwrap().state,"revoked");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn publication_first_drafts_are_private_replayable_and_cannot_be_changed_to_legacy() {
    let f = configured().await;
    let a = draft(&f).await;
    assert!(f.domain.contribution(&f.admin.p,&f.policy,&a.contribution.id).await.is_err());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(db.batch_execute(include_str!("management_publication_first_down.sql")).await.is_err(),
        "rollback must preserve the private draft's authority and history");
    let replay = draft(&f).await;
    assert_eq!(a.contribution.id,replay.contribution.id);
    assert!(f.domain.create_contribution(&f.owner.p,&f.policy,offer(&f,"publication_first")).await.is_err());
    let mut invalid = terms(&f,&a).await;
    invalid.receipt.admission_request_id = Some("different_admission".into());
    assert!(f.domain.publish_management_grant(&f.owner.p,&f.policy,&a.contribution.id,invalid).await.is_err());
    assert!(!actionable(&f,&a.request.id).await);
}
