//! Pool capacity succession: a v3 amendment supersedes the effective grant in
//! one transaction, reserved Agents follow the chain and nothing else does.
use super::*;
use crate::management_agent_executor_types::*;
use crate::management_agent_types::ManagedAgentReservationReceipt;
use diesel::sql_types::{Bool, Nullable};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    count: i64,
}
#[derive(QueryableByName)]
struct Stored {
    #[diesel(sql_type = Text)]
    value: String,
}
#[derive(QueryableByName)]
struct Chain {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Text)]
    chain_root_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    predecessor_grant_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    succession: Option<String>,
    #[diesel(sql_type = BigInt)]
    period_start_ms: i64,
}

#[derive(QueryableByName)]
struct SchemaState {
    #[diesel(sql_type = BigInt)]
    version: i64,
    #[diesel(sql_type = Bool)]
    columns: bool,
}
async fn schema_state(db: &mut AsyncPgConnection) -> SchemaState {
    sql_query("SELECT version::bigint AS version,EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='management_resource_grants' AND column_name IN ('chain_root_id','predecessor_grant_id','succession','period_start_ms')) AS columns FROM hagency_agent_v1.management_deployment")
        .get_result::<SchemaState>(db)
        .await
        .unwrap()
}
async fn session(s: &salvo::Service, token: &str) -> Value {
    let mut r = Box::pin(
        salvo::test::TestClient::get("https://example.test/hagency/api/v1/management/session")
            .add_header("host", "example.test", true)
            .bearer_auth(token)
            .send(s),
    )
    .await;
    r.take_json::<Value>().await.unwrap()
}
async fn pool_entry(f: &Fixture, contribution: &str) -> ResourcePool {
    f.domain
        .resource_pools(&f.outsider.p, &f.policy, CatalogQuery { after: None, capability: None })
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|p| p.id == contribution)
        .unwrap()
}
async fn attempt(f: &Fixture, a: &Admission, root: &ResourceGrantView, receipt: NativeGrantReceipt) -> crate::Result<ResourceGrantView> {
    f.domain
        .publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, succeed(a, root, receipt))
        .await
}
async fn succession_fixture() -> Fixture {
    let mut f = isolated_fixture().await;
    f.policy = f
        .policy
        .with_agent_admission(true)
        .with_agent_executor(true)
        .with_pool_grant_succession(true);
    f
}
/// A v2 pool grant, as `pool` publishes it, with its server view.
async fn pool_grant(f: &Fixture, manager: &Principal) -> (Admission, ResourceGrantView) {
    let a = admitted(f).await;
    let mut g = grant_input(f, &a, manager).await;
    g.receipt.version = 2;
    g.receipt.admission_request_id = Some(a.request.id.clone());
    let view = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g)
        .await
        .unwrap();
    (a, view)
}
/// The owner's native successor receipt of `pred` (no carried Agents unless set).
fn successor(pred: &ResourceGrantView, kind: &str, tokens: i64, ledger: i64) -> NativeGrantReceipt {
    let mut r = pred.receipt.clone();
    r.version = 3;
    r.grant_id = format!("mgs_{}", &secret_token()[..32]);
    r.grant_revision = pred.receipt.grant_revision + 1;
    r.allocated_tokens = tokens;
    r.ledger_revision = ledger;
    r.succession = Some(GrantSuccession {
        kind: kind.into(),
        predecessor_grant_id: pred.id.clone(),
        predecessor_receipt_digest: pred.receipt_digest.clone(),
        carried_children: 0,
        carried_retained_tokens: 0,
    });
    r
}
fn succeed(a: &Admission, pred: &ResourceGrantView, receipt: NativeGrantReceipt) -> SucceedGrantInput {
    SucceedGrantInput {
        command_id: format!("succeed_{}", &receipt.grant_id[4..20]),
        expected_contribution_revision: a.contribution.revision,
        expected_predecessor_revision: pred.revision,
        receipt,
    }
}
async fn amend(f: &Fixture, a: &Admission, pred: &ResourceGrantView, tokens: i64, ledger: i64) -> ResourceGrantView {
    f.domain
        .publish_management_grant_successor(&f.owner.p, &f.policy, &pred.id, succeed(a, pred, successor(pred, "amendment", tokens, ledger)))
        .await
        .unwrap()
}
async fn grant_view(f: &Fixture, id: &str) -> ResourceGrantView {
    f.domain.management_resource_grant(&f.owner.p, &f.policy, id).await.unwrap()
}
fn code<T: std::fmt::Debug>(r: crate::Result<T>) -> String {
    r.unwrap_err().to_string()
}
async fn request_agent(f: &Fixture, a: &Admission, id: &str, tokens: i64, grant_revision: i64) -> PoolAgentRequest {
    let mut i = input(a, id);
    i.requested_tokens = tokens;
    i.expected_grant_revision = grant_revision;
    f.domain
        .request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, i)
        .await
        .unwrap()
}
async fn decide(f: &Fixture, r: &PoolAgentRequest, approve: bool) -> crate::Result<PoolAgentRequest> {
    f.domain
        .decide_pool_agent(
            &f.owner.p,
            &f.policy,
            &r.id,
            DecidePoolAgent {
                command_id: format!("decide_{}", &r.id[4..20]),
                expected_revision: r.revision,
                decision: if approve { Decision::Approve } else { Decision::Reject },
            },
        )
        .await
}
/// Poll the owner executor until the reserve command of `request` is delivered.
async fn reserve_delivery(f: &Fixture, request: &str) -> AgentExecutorDelivery {
    let mut after = None;
    for _ in 0..16 {
        let d = f
            .domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: after.clone() })
            .await
            .unwrap()
            .expect("an executor command");
        if let AgentExecutorCommand::Reserve(c) = &d.command {
            if c.request_id == request {
                return d;
            }
            after = Some(c.command_id.clone());
        }
    }
    panic!("no reserve delivery for {request}");
}
/// Record the native ledger's reservation of an approved request.
async fn reserve(f: &Fixture, request: &str, ledger: i64) -> PendingManagedAssignment {
    let d = reserve_delivery(f, request).await;
    assert!(d.execution_allowed);
    let AgentExecutorCommand::Reserve(c) = &d.command else { unreachable!() };
    let receipt = AgentExecutorReceipt::Reserve(ManagedAgentReservationReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.command_digest.clone(),
        request_id: c.request_id.clone(),
        agent_id: c.proposed_agent_id.clone(),
        allocation_id: c.allocation_id.clone(),
        agent_reservation_id: Some(format!("child_{}", c.proposed_agent_id)),
        definition_digest: c.definition_digest.clone(),
        state: "reserved".into(),
        budget_revision: 1,
        ledger_revision: ledger,
        allocated_tokens: c.requested_tokens,
        retained_tokens: c.requested_tokens,
        period_key: c.period_key.clone(),
        reason: None,
    });
    f.domain
        .record_managed_agent_executor_result(
            &f.owner.p,
            &f.policy,
            &c.command_id,
            AgentExecutorResultInput { command_digest: d.command_digest.clone(), token: d.token.clone(), receipt },
        )
        .await
        .unwrap()
        .assignment
        .expect("a live reservation creates its assignment")
}
async fn intent_state(f: &Fixture, request: &str) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT state AS value FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1")
        .bind::<Text, _>(request)
        .get_result::<Stored>(&mut db)
        .await
        .unwrap()
        .value
}
async fn chain(f: &Fixture, contribution: &str) -> Vec<Chain> {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT id,state,chain_root_id,predecessor_grant_id,succession,period_start_ms FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 ORDER BY period_start_ms,id")
        .bind::<Text, _>(contribution)
        .load::<Chain>(&mut db)
        .await
        .unwrap()
}
/// Grants of the contribution whose window covers `at`, by the server rule.
async fn live_count(f: &Fixture, contribution: &str, at: i64) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND state='active' AND period_start_ms<=$2 AND expires_at_ms>$2")
        .bind::<Text, _>(contribution)
        .bind::<BigInt, _>(at)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
/// Insert a scheduled renewal of `pred` for the next UTC month, as the route
/// would after native confirmation inside the renewal window. Tests use it to
/// exercise chains without waiting for the last week of the real month.
async fn seed_renewal(f: &Fixture, pred: &ResourceGrantView, tokens: i64, ledger: i64) -> NativeGrantReceipt {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    #[derive(QueryableByName)]
    struct Next {
        #[diesel(sql_type = Text)]
        key: String,
        #[diesel(sql_type = BigInt)]
        end_ms: i64,
    }
    let next = sql_query("SELECT to_char(to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC','YYYY-MM') AS key,(extract(epoch FROM ((date_trunc('month',to_timestamp($1::double precision / 1000) AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC'))*1000)::bigint AS end_ms")
        .bind::<BigInt, _>(pred.receipt.period_end_ms)
        .get_result::<Next>(&mut db)
        .await
        .unwrap();
    let mut r = successor(pred, "renewal", tokens, ledger);
    r.period_key = next.key;
    r.period_start_ms = pred.receipt.period_end_ms;
    r.period_end_ms = next.end_ms;
    r.delegation_expires_at_ms = next.end_ms;
    r.validate("example.test").unwrap();
    let chain_root: Stored = sql_query("SELECT chain_root_id AS value FROM hagency_agent_v1.management_resource_grants WHERE id=$1")
        .bind::<Text, _>(&pred.id)
        .get_result(&mut db)
        .await
        .unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_resource_grants(id,contribution_id,owner_user_id,executor_device_id,executor_generation,receipt,receipt_digest,state,period_key,expires_at_ms,observed_at_ms,chain_root_id,predecessor_grant_id,succession,period_start_ms) SELECT $1,contribution_id,owner_user_id,executor_device_id,executor_generation,$2,$3,'active',$4,$5,observed_at_ms,$6,id,'renewal',$7 FROM hagency_agent_v1.management_resource_grants WHERE id=$8")
        .bind::<Text, _>(&r.grant_id)
        .bind::<Text, _>(serde_json::to_string(&r).unwrap())
        .bind::<Text, _>(allocation_digest(&r).unwrap())
        .bind::<Text, _>(&r.period_key)
        .bind::<BigInt, _>(r.delegation_expires_at_ms.min(r.period_end_ms))
        .bind::<Text, _>(&chain_root.value)
        .bind::<BigInt, _>(r.period_start_ms)
        .bind::<Text, _>(&pred.id)
        .execute(&mut db)
        .await
        .unwrap();
    r
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn amendment_supersedes_atomically_and_only_the_successor_is_live() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let entry = pool_entry(&f, &a.contribution.id).await;
    assert_eq!((entry.grant_revision, entry.published_tokens), (Some(1), Some(1_000_000)));
    let input = succeed(&a, &root, successor(&root, "amendment", 800_000, 20));
    let first = f
        .domain
        .publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, input.clone())
        .await
        .unwrap();
    // An exact retry replays the original result; nothing else changes.
    let replay = f
        .domain
        .publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, input.clone())
        .await
        .unwrap();
    assert_eq!((first.id.clone(), first.revision), (replay.id.clone(), replay.revision));
    assert_eq!(first.state, "active");
    assert_eq!(first.receipt.version, 3);
    assert_eq!(grant_view(&f, &root.id).await.state, "superseded");
    let rows = chain(&f, &a.contribution.id).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.chain_root_id == root.id && r.period_start_ms == root.receipt.period_start_ms));
    let succ = rows.iter().find(|r| r.id == first.id).unwrap();
    assert_eq!((succ.predecessor_grant_id.as_deref(), succ.succession.as_deref()), (Some(root.id.as_str()), Some("amendment")));
    assert_eq!(live_count(&f, &a.contribution.id, crate::api::now_ms()).await, 1);
    let entry = pool_entry(&f, &a.contribution.id).await;
    assert_eq!((entry.grant_revision, entry.published_tokens), (Some(2), Some(800_000)));
    // The superseded predecessor can be neither amended nor revoked again.
    let again = succeed(&a, &grant_view(&f, &root.id).await, successor(&root, "amendment", 700_000, 21));
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, again).await), "grant_predecessor_not_effective");
    // The successor itself is the chain head and can be amended in turn.
    let second = amend(&f, &a, &first, 900_000, 22).await;
    assert_eq!(second.receipt.grant_revision, 3);
    assert_eq!(grant_view(&f, &first.id).await.state, "superseded");
    assert_eq!(live_count(&f, &a.contribution.id, crate::api::now_ms()).await, 1);
    // Another command for the same receipt sees the superseded predecessor.
    let mut other = input.clone();
    other.command_id = "succeed_other_command".into();
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, other).await), "management_revision_changed");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn successor_rules_refuse_every_identity_period_and_manager_change() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let base = successor(&root, "amendment", 800_000, 20);
    // Removing an eligible manager would silently stop that manager's Agents.
    let mut r = base.clone();
    r.eligible_managers = vec![f.admin.p.mxid.clone()];
    assert_eq!(code(attempt(&f, &a, &root, r).await), "invalid_grant_succession");
    // Adding one is allowed (checked below with a successful amendment).
    let cases: [(fn(&mut NativeGrantReceipt), &str); 10] = [
        (|r| r.grant_revision = 3, "invalid_grant_succession"),
        (|r| r.ledger_revision = 10, "invalid_grant_succession"),
        (|r| r.succession.as_mut().unwrap().predecessor_receipt_digest = "0".repeat(64), "invalid_grant_succession"),
        (|r| r.enrollment_id = "men_other".into(), "resource_grant_binding_mismatch"),
        (|r| r.allow_self_approval = true, "resource_grant_binding_mismatch"),
        (|r| r.admission_request_id = Some("mrq_other".into()), "resource_grant_binding_mismatch"),
        (|r| r.resource_revision = "b".repeat(64), "resource_grant_binding_mismatch"),
        (|r| { r.period_start_ms += 1; }, "invalid_grant_period"),
        (|r| r.period_key = "2099-01".into(), "invalid_grant_period"),
        (|r| r.succession.as_mut().unwrap().predecessor_grant_id = "mgr_other".into(), "invalid_grant_succession"),
    ];
    for (change, expected) in cases {
        let mut r = base.clone();
        change(&mut r);
        assert_eq!(code(attempt(&f, &a, &root, r).await), expected);
    }
    // A v2 receipt cannot be published as a successor, nor a v3 one as a root.
    let mut v2 = base.clone();
    v2.version = 2;
    v2.succession = None;
    assert_eq!(code(attempt(&f, &a, &root, v2).await), "invalid_grant_succession");
    let mut root_input = grant_input(&f, &a, &f.outsider.p).await;
    root_input.receipt = base.clone();
    assert_eq!(code(f.domain.publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, root_input).await), "invalid_grant_succession");
    // Only the executor device of the owner may publish.
    let input = succeed(&a, &root, base.clone());
    for wrong in [&f.coordinator.p, &f.outsider.p, &f.admin.p] {
        assert!(f.domain.publish_management_grant_successor(wrong, &f.policy, &root.id, input.clone()).await.is_err());
    }
    // Stale revisions are refused before any effect.
    let mut stale = input.clone();
    stale.expected_predecessor_revision = 2;
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, stale).await), "management_revision_changed");
    // Off by default: the flag gates the whole route.
    let off = f.policy.clone().with_pool_grant_succession(false);
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &off, &root.id, input.clone()).await), "pool_grant_succession_unavailable");
    let mut added = base.clone();
    added.eligible_managers.push(f.admin.p.mxid.clone());
    added.eligible_managers.sort();
    let view = f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, succeed(&a, &root, added)).await.unwrap();
    assert_eq!(view.receipt.eligible_managers.len(), 2);
    assert!(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, input).await.is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn carried_agents_stay_live_and_unreserved_intents_are_fenced() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let carried = request_agent(&f, &a, "carried", 100, 1).await;
    decide(&f, &carried, true).await.unwrap();
    let assignment = reserve(&f, &carried.id, 100).await;
    let unreserved = request_agent(&f, &a, "unreserved", 100, 1).await;
    decide(&f, &unreserved, true).await.unwrap();
    let pending = request_agent(&f, &a, "pending_small", 100, 1).await;
    let large = request_agent(&f, &a, "pending_large", 900_000, 1).await;
    // The server knows 200 tokens are held (one reserved, one approved).
    let below = succeed(&a, &root, successor(&root, "amendment", 150, 20));
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, below).await), "grant_successor_below_reserved");
    let next = amend(&f, &a, &root, 800_000, 21).await;
    // The trigger fenced only the intent that never reserved.
    assert_eq!(intent_state(&f, &unreserved.id).await, "fenced");
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
    // The executor sweep does not fence the carried Agent, and its exact
    // original command is still live (the projection call fences if not).
    let _ = f.domain.poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default()).await.unwrap();
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
    let current = f.domain.managed_agent_assignment(&f.owner.p, &f.policy, &carried.id).await.unwrap().unwrap();
    assert_eq!(current.id, assignment.id);
    assert_eq!(current.state, "pending_native");
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
    let view = f.domain.pool_agent_request(&f.outsider.p, &carried.id).await.unwrap();
    assert_eq!(view.blocking_reason, None);
    let view = f.domain.pool_agent_request(&f.outsider.p, &unreserved.id).await.unwrap();
    assert_eq!(view.blocking_reason.as_deref(), Some("pool_agent_authority_changed"));
    // A pending request is decided against the successor within its limit...
    assert_eq!(f.domain.pool_agent_request(&f.outsider.p, &pending.id).await.unwrap().blocking_reason, None);
    let approved = decide(&f, &pending, true).await.unwrap();
    assert_eq!(approved.state, "approved_waiting_executor");
    let d = reserve_delivery(&f, &pending.id).await;
    assert!(d.execution_allowed);
    let AgentExecutorCommand::Reserve(c) = &d.command else { unreachable!() };
    assert_eq!(c.pool_grant.as_ref().unwrap().grant_id, next.id);
    assert_eq!(c.authority_revision, 2);
    assert_eq!(c.grant_receipt_digest, next.receipt_digest);
    // ...and refused above it.
    assert_eq!(
        f.domain.pool_agent_request(&f.outsider.p, &large.id).await.unwrap().blocking_reason.as_deref(),
        Some("pool_agent_resource_mismatch")
    );
    assert_eq!(code(decide(&f, &large, true).await), "pool_agent_resource_mismatch");
    // A new request needs the successor's grant revision.
    let mut stale = input(&a, "stale_revision");
    stale.expected_grant_revision = 1;
    assert_eq!(code(f.domain.request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, stale).await), "management_revision_changed");
    request_agent(&f, &a, "fresh", 100, 2).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn amendment_waits_for_a_scheduled_renewal_and_revoke_fences_the_chain() {
    let mut f = succession_fixture().await;
    f.policy = f.policy.with_parent_settlement(true);
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let carried = request_agent(&f, &a, "chain_agent", 100, 1).await;
    decide(&f, &carried, true).await.unwrap();
    reserve(&f, &carried.id, 100).await;
    let amended = amend(&f, &a, &root, 800_000, 20).await;
    let renewal = seed_renewal(&f, &amended, 800_000, 30).await;
    // Not live before its month starts; the amendment is still effective.
    assert_eq!(live_count(&f, &a.contribution.id, crate::api::now_ms()).await, 1);
    assert_eq!(live_count(&f, &a.contribution.id, renewal.period_start_ms).await, 1);
    let current = grant_view(&f, &amended.id).await;
    let amend_again = succeed(&a, &current, successor(&current, "amendment", 700_000, 21));
    assert_eq!(code(f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &current.id, amend_again.clone()).await), "grant_successor_exists");
    // Revoking the effective grant withdraws the scheduled renewal and fences
    // every Agent authorized through the chain.
    f.domain
        .revoke_management_grant(&f.owner.p, &f.policy, &current.id, RevokeInput { command_id: "revoke_effective".into(), expected_revision: current.revision })
        .await
        .unwrap();
    assert_eq!(intent_state(&f, &carried.id).await, "fenced");
    let rows = chain(&f, &a.contribution.id).await;
    let state = |id: &str| rows.iter().find(|r| r.id == id).unwrap().state.clone();
    assert_eq!((state(&root.id), state(&amended.id), state(&renewal.grant_id)), ("superseded".into(), "revoked".into(), "revoked".into()));
    assert_eq!(live_count(&f, &a.contribution.id, crate::api::now_ms()).await, 0);
    assert_eq!(live_count(&f, &a.contribution.id, renewal.period_start_ms).await, 0);
    // Both revoked grants have a queued native close; the superseded one none.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let closes: Count = sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_parent_settlements WHERE grant_id IN ($1,$2)")
        .bind::<Text, _>(&amended.id).bind::<Text, _>(&renewal.grant_id).get_result(&mut db).await.unwrap();
    assert_eq!(closes.count, 2);
    let none: Count = sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_parent_settlements WHERE grant_id=$1")
        .bind::<Text, _>(&root.id).get_result(&mut db).await.unwrap();
    assert_eq!(none.count, 0);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn cancelling_a_scheduled_renewal_frees_the_slot_without_fencing_the_month() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let carried = request_agent(&f, &a, "month_agent", 100, 1).await;
    decide(&f, &carried, true).await.unwrap();
    reserve(&f, &carried.id, 100).await;
    let renewal = seed_renewal(&f, &root, 900_000, 30).await;
    let scheduled = grant_view(&f, &renewal.grant_id).await;
    f.domain
        .revoke_management_grant(&f.owner.p, &f.policy, &scheduled.id, RevokeInput { command_id: "cancel_renewal".into(), expected_revision: scheduled.revision })
        .await
        .unwrap();
    // This month's Agents keep running; the root may be amended again (Q7).
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
    assert_eq!(grant_view(&f, &root.id).await.state, "active");
    let amended = amend(&f, &a, &root, 800_000, 31).await;
    assert_eq!(amended.state, "active");
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn contribution_revoke_skips_superseded_grants_with_parent_settlement() {
    let mut f = succession_fixture().await;
    f.policy = f.policy.with_parent_settlement(true);
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let amended = amend(&f, &a, &root, 800_000, 20).await;
    let admission = f.domain.contribution(&f.owner.p, &f.policy, &a.contribution.id).await.unwrap();
    f.domain
        .revoke_contribution(&f.owner.p, &f.policy, &a.contribution.id, RevokeInput { command_id: "revoke_pool".into(), expected_revision: admission.contribution.revision })
        .await
        .unwrap();
    assert_eq!(grant_view(&f, &root.id).await.state, "superseded");
    assert_eq!(grant_view(&f, &amended.id).await.state, "revoked");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let closes: Count = sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_parent_settlements WHERE grant_id=$1")
        .bind::<Text, _>(&amended.id).get_result(&mut db).await.unwrap();
    assert_eq!(closes.count, 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_chain_of_one_grant_behaves_exactly_as_before() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    assert_eq!(chain(&f, &a.contribution.id).await[0].chain_root_id, root.id);
    let r = request_agent(&f, &a, "classic", 100, 1).await;
    decide(&f, &r, true).await.unwrap();
    let d = reserve_delivery(&f, &r.id).await;
    let AgentExecutorCommand::Reserve(c) = &d.command else { unreachable!() };
    // The command is byte-for-byte the pre-succession command for this grant.
    assert_eq!(serde_json::to_value(c.pool_grant.as_ref().unwrap()).unwrap(), serde_json::to_value(&root.receipt).unwrap());
    assert!(serde_json::to_value(c).unwrap()["poolGrant"].get("succession").is_none());
    assert_eq!((c.authority_revision, c.expires_at_ms, c.period_key.as_str()), (1, r.expires_at_ms, root.receipt.period_key.as_str()));
    let stored = f.domain.pool_agent_request(&f.outsider.p, &r.id).await.unwrap();
    assert_eq!(stored.revision, 2);
    // Revoking the only grant fences its Agent, as before.
    f.domain
        .revoke_management_grant(&f.owner.p, &f.policy, &root.id, RevokeInput { command_id: "revoke_root".into(), expected_revision: root.revision })
        .await
        .unwrap();
    assert_eq!(intent_state(&f, &r.id).await, "fenced");
    assert_eq!(live_count(&f, &a.contribution.id, crate::api::now_ms()).await, 0);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn never_two_live_grants_over_random_publish_succeed_and_revoke() {
    let f = succession_fixture().await;
    let (a, _) = pool_grant(&f, &f.outsider.p).await;
    let now = crate::api::now_ms();
    // Deterministic LCG: the sequence is reproducible from this seed.
    let mut seed: u64 = 0x5eed_2026_1029;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let mut ledger = 100;
    let mut renewal_month_end = None;
    for step in 0..40 {
        let rows = chain(&f, &a.contribution.id).await;
        let active: Vec<_> = rows.iter().filter(|r| r.state == "active").collect();
        ledger += 2;
        match next() % 5 {
            0 | 1 => {
                // Amend the effective grant with a random amount.
                if let Some(head) = active.iter().find(|r| r.period_start_ms <= now) {
                    let view = grant_view(&f, &head.id).await;
                    let input = succeed(&a, &view, successor(&view, "amendment", 1 + (next() % 2_000_000) as i64, ledger));
                    let result = f.domain.publish_management_grant_successor(&f.owner.p, &f.policy, &view.id, input).await;
                    if let Err(e) = &result {
                        assert!(matches!(e.to_string().as_str(), "grant_successor_exists"), "{step}: {e}");
                    }
                }
            }
            2 => {
                if let Some(head) = active.iter().find(|r| r.period_start_ms <= now) {
                    let view = grant_view(&f, &head.id).await;
                    if !rows.iter().any(|r| r.predecessor_grant_id.as_deref() == Some(view.id.as_str()) && r.state != "revoked") {
                        let r = seed_renewal(&f, &view, 1 + (next() % 2_000_000) as i64, ledger).await;
                        renewal_month_end = Some(r.period_end_ms);
                    }
                }
            }
            3 => {
                if !active.is_empty() {
                    let pick = active[next() % active.len()];
                    let view = grant_view(&f, &pick.id).await;
                    f.domain
                        .revoke_management_grant(&f.owner.p, &f.policy, &view.id, RevokeInput { command_id: format!("random_revoke_{step}"), expected_revision: view.revision })
                        .await
                        .unwrap();
                }
            }
            _ => {
                // A new root this month is refused while any grant of the
                // admission exists for it (one chain per admission per month).
                let mut g = grant_input(&f, &a, &f.outsider.p).await;
                g.command_id = format!("random_root_{step}");
                g.receipt.version = 2;
                g.receipt.admission_request_id = Some(a.request.id.clone());
                g.receipt.ledger_revision = ledger;
                assert_eq!(code(f.domain.publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g).await), "resource_grant_already_exists");
            }
        }
        for at in [now, now + 1, renewal_month_end.map_or(now, |end| end - 1)] {
            assert!(live_count(&f, &a.contribution.id, at).await <= 1, "step {step} at {at}");
        }
        // Exactly the server's own effective-grant query: never a conflict.
        f.domain
            .resource_pools(&f.outsider.p, &f.policy, CatalogQuery { after: None, capability: None })
            .await
            .unwrap();
        // At most one non-revoked successor per grant (the unique index).
        let rows = chain(&f, &a.contribution.id).await;
        for r in &rows {
            assert!(rows.iter().filter(|s| s.predecessor_grant_id.as_deref() == Some(r.id.as_str()) && s.state != "revoked").count() <= 1);
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn successor_http_route_is_closed_flagged_and_recoverable() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let http = service(&f, true);
    let path = format!("grants/{}/successors", root.id);
    let body = serde_json::to_value(succeed(&a, &root, successor(&root, "amendment", 800_000, 20))).unwrap();
    let mut injected = body.clone();
    injected["chainRootId"] = json!(root.id);
    post(&http, &path, &f.owner.token, injected, StatusCode::BAD_REQUEST).await;
    let mut injected = body.clone();
    injected["receipt"]["succession"]["carriedAgents"] = json!(1);
    post(&http, &path, &f.owner.token, injected, StatusCode::BAD_REQUEST).await;
    let disabled = {
        let g = Fixture {
            auth: f.auth.clone(),
            domain: f.domain.clone(),
            url: f.url.clone(),
            policy: f.policy.clone().with_pool_grant_succession(false),
            owner: same_actor(&f.auth, &f.owner).await,
            admin: same_actor(&f.auth, &f.admin).await,
            coordinator: same_actor(&f.auth, &f.coordinator).await,
            outsider: same_actor(&f.auth, &f.outsider).await,
        };
        service(&g, true)
    };
    let refused = post(&disabled, &path, &f.owner.token, body.clone(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(refused["code"], "pool_grant_succession_unavailable");
    let reply = post(&http, &path, &f.owner.token, body.clone(), StatusCode::OK).await;
    assert_eq!(reply["grant"]["state"], "active");
    assert_eq!(reply["grant"]["receipt"]["succession"]["kind"], "amendment");
    assert!(reply["observedAtMs"].as_i64().unwrap() > 0);
    let replay = post(&http, &path, &f.owner.token, body.clone(), StatusCode::OK).await;
    assert_eq!(reply["grant"]["id"], replay["grant"]["id"]);
    let command = body["commandId"].as_str().unwrap();
    let recovered = post(&http, &format!("allocation-commands/{command}/recover"), &f.owner.token, json!({"operation":"grant.succeed"}), StatusCode::OK).await;
    assert_eq!(recovered["recorded"], true);
    assert_eq!(recovered["grant"]["id"], reply["grant"]["id"]);
    let unknown = post(&http, "allocation-commands/never_sent/recover", &f.owner.token, json!({"operation":"grant.succeed"}), StatusCode::OK).await;
    assert_eq!(unknown["recorded"], false);
    // The session advertises the protocol only while the flag is on.
    let on = session(&http, &f.owner.token).await;
    assert!(on["features"].as_array().unwrap().contains(&json!("pool-grant-succession-v1")));
    let off = session(&disabled, &f.owner.token).await;
    assert!(!off["features"].as_array().unwrap().contains(&json!("pool-grant-succession-v1")));
    // The grant view of a superseded predecessor says so.
    let old = post(&http, &format!("grants/{}/successors", root.id), &f.owner.token, body, StatusCode::OK).await;
    assert_eq!(old["grant"]["id"], reply["grant"]["id"]);
    assert_eq!(grant_view(&f, &root.id).await.state, "superseded");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn schema_29_upgrades_a_populated_schema_28_database_and_downgrades_back() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let carried = request_agent(&f, &a, "migrated", 100, 1).await;
    decide(&f, &carried, true).await.unwrap();
    reserve(&f, &carried.id, 100).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Back to schema 28 with the committed lossy downgrade (no v3 rows yet).
    db.batch_execute(include_str!("../../../deploy/pool-succession-downgrade-to-28.sql")).await.unwrap();
    let downgraded = schema_state(&mut db).await;
    assert_eq!((downgraded.version, downgraded.columns), (28, false));
    assert_ne!(intent_state(&f, &carried.id).await, "fenced");
    // Opening the store migrates the populated database to 29 again.
    let reopened = crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
    let upgraded = schema_state(&mut db).await;
    assert_eq!((upgraded.version, upgraded.columns), (29, true));
    let rows = chain(&f, &a.contribution.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].chain_root_id.as_str(), rows[0].predecessor_grant_id.as_deref(), rows[0].period_start_ms), (root.id.as_str(), None, root.receipt.period_start_ms));
    // The reserved Agent kept its authority through both migrations.
    let current = reopened.managed_agent_assignment(&f.owner.p, &f.policy, &carried.id).await.unwrap().unwrap();
    assert_eq!(current.state, "pending_native");
    // An amendment works on the migrated chain; the downgrade then revokes v3
    // grants and maps superseded ones to revoked (carried Agents stop).
    let amended = f.domain
        .publish_management_grant_successor(&f.owner.p, &f.policy, &root.id, succeed(&a, &root, successor(&root, "amendment", 800_000, 20)))
        .await
        .unwrap();
    db.batch_execute(include_str!("../../../deploy/pool-succession-downgrade-to-28.sql")).await.unwrap();
    #[derive(QueryableByName)]
    struct States {
        #[diesel(sql_type = Text)]
        states: String,
    }
    let states: States = sql_query("SELECT string_agg(id||'='||state,',' ORDER BY id) AS states FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1")
        .bind::<Text, _>(&a.contribution.id).get_result(&mut db).await.unwrap();
    let mut expected = [format!("{}=revoked", root.id), format!("{}=revoked", amended.id)];
    expected.sort();
    assert_eq!(states.states, expected.join(","));
    assert_eq!(intent_state(&f, &carried.id).await, "fenced");
    crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
    assert_eq!(schema_state(&mut db).await.version, 29);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn revoking_a_superseded_grant_withdraws_its_chain_with_and_without_parent_settlement() {
    for settlement in [false, true] {
        let mut f = succession_fixture().await;
        f.policy = f.policy.with_parent_settlement(settlement);
        let (a, root) = pool_grant(&f, &f.outsider.p).await;
        let carried = request_agent(&f, &a, &format!("superseded_{settlement}"), 100, 1).await;
        decide(&f, &carried, true).await.unwrap();
        reserve(&f, &carried.id, 100).await;
        let amended = amend(&f, &a, &root, 800_000, 20).await;
        let old = grant_view(&f, &root.id).await;
        f.domain
            .revoke_management_grant(&f.owner.p, &f.policy, &old.id, RevokeInput { command_id: format!("revoke_superseded_{settlement}"), expected_revision: old.revision })
            .await
            .unwrap();
        assert_eq!(grant_view(&f, &root.id).await.state, "superseded");
        assert_eq!(grant_view(&f, &amended.id).await.state, "revoked");
        assert_eq!(intent_state(&f, &carried.id).await, "fenced");
        let closes = |id: String| {
            let url = f.url.clone();
            async move {
                let mut db = AsyncPgConnection::establish(&url).await.unwrap();
                sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_parent_settlements WHERE grant_id=$1")
                    .bind::<Text, _>(id).get_result::<Count>(&mut db).await.unwrap().count
            }
        };
        assert_eq!(closes(amended.id.clone()).await, i64::from(settlement));
        assert_eq!(closes(root.id.clone()).await, 0);
    }
}
