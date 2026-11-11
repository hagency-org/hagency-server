//! Owner ledger epochs: a receipt number is refused as reused only inside the
//! ledger database its device last declared, and a declaration that replaces
//! a database ends what the earlier one held.
use super::*;
use crate::management_parent_types::*;
use std::collections::{BTreeMap, BTreeSet};

const AGENT_RECEIPTS: &str = "SELECT coalesce(string_agg(receipt_ledger_epoch||'/'||(receipt::jsonb->>'ledgerRevision'),',' ORDER BY receipt_ledger_epoch,(receipt::jsonb->>'ledgerRevision')::bigint),'') AS value FROM hagency_agent_v1.management_agent_executor_commands WHERE receipt IS NOT NULL";
const GRANTS: &str = "SELECT coalesce(string_agg(ledger_epoch||'/'||(receipt::jsonb->>'ledgerRevision'),',' ORDER BY ledger_epoch,(receipt::jsonb->>'ledgerRevision')::bigint),'') AS value FROM hagency_agent_v1.management_resource_grants";

/// `epoch/number` of every stored row the query selects, in order.
async fn stored(f: &Fixture, sql: &str) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(sql)
        .get_result::<Stored>(&mut db)
        .await
        .unwrap()
        .value
}
/// `epoch/first two id characters` of every ledger `who` declared, in order.
async fn ledgers(f: &Fixture, who: &Actor) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT coalesce(string_agg(epoch||'/'||left(ledger_id,2),',' ORDER BY epoch),'') AS value FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2")
        .bind::<Text, _>(&who.p.user_id)
        .bind::<Text, _>(who.p.device_id.as_deref().unwrap())
        .get_result::<Stored>(&mut db)
        .await
        .unwrap()
        .value
}
fn ledger(n: u8) -> ExecutorLedgerInput {
    ExecutorLedgerInput {
        ledger_id: format!("{n:02x}").repeat(32),
        fresh: false,
    }
}
async fn declare(f: &Fixture, who: &Actor, n: u8) -> crate::Result<ExecutorLedger> {
    f.domain
        .declare_executor_ledger(&who.p, &f.policy, ledger(n))
        .await
}
/// The device created database `n` empty and says so.
async fn declare_fresh(f: &Fixture, who: &Actor, n: u8) -> crate::Result<ExecutorLedger> {
    f.domain
        .declare_executor_ledger(
            &who.p,
            &f.policy,
            ExecutorLedgerInput {
                fresh: true,
                ..ledger(n)
            },
        )
        .await
}
/// Request, approve and deliver the reserve command of one pool Agent.
async fn approved(
    f: &Fixture,
    a: &Admission,
    id: &str,
    grant_revision: i64,
) -> AgentExecutorDelivery {
    let r = request_agent(f, a, id, 100, grant_revision).await;
    decide(f, &r, true).await.unwrap();
    let d = reserve_delivery(f, &r.id).await;
    assert!(d.execution_allowed);
    d
}
fn reserved(d: &AgentExecutorDelivery, ledger: i64) -> AgentExecutorResultInput {
    let AgentExecutorCommand::Reserve(c) = &d.command else {
        unreachable!()
    };
    AgentExecutorResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentExecutorReceipt::Reserve(ManagedAgentReservationReceipt {
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
        }),
    }
}
/// The native ledger reports its reservation of `d` as mutation `ledger`.
async fn report(
    f: &Fixture,
    d: &AgentExecutorDelivery,
    ledger: i64,
) -> crate::Result<AgentExecutorAcknowledgement> {
    let AgentExecutorCommand::Reserve(c) = &d.command else {
        unreachable!()
    };
    f.domain
        .record_managed_agent_executor_result(
            &f.owner.p,
            &f.policy,
            &c.command_id,
            reserved(d, ledger),
        )
        .await
}
/// A second resource of the same owner device, admitted and ready to publish
/// its pool grant as mutation `ledger`.
async fn second_pool(f: &Fixture, ledger: i64) -> (Admission, PublishGrantInput) {
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(f, "recreated_capacity"))
        .await
        .unwrap();
    let a = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("admit_recreated", 1),
        )
        .await
        .unwrap();
    let mut g = grant_input(f, &a, &f.outsider.p).await;
    g.command_id = "publish_recreated_grant".into();
    g.receipt.version = 2;
    g.receipt.admission_request_id = Some(a.request.id.clone());
    g.receipt.ledger_revision = ledger;
    (a, g)
}
async fn call(s: &Service, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = TestClient::post(format!(
        "https://example.test/hagency/api/v1/management/{path}"
    ))
    .add_header("host", "example.test", true);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let mut r = Box::pin(request.json(&body).send(s)).await;
    (
        r.status_code.unwrap(),
        r.take_json::<Value>().await.unwrap(),
    )
}

type Rows = BTreeMap<String, BTreeSet<String>>;
/// Every row of every table of the schema, as text.
async fn rows(f: &Fixture) -> Rows {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let tables: Vec<Stored> = sql_query("SELECT table_name::text AS value FROM information_schema.tables WHERE table_schema='hagency_agent_v1' AND table_type='BASE TABLE' ORDER BY 1")
        .load(&mut db)
        .await
        .unwrap();
    let mut all = Rows::new();
    for table in tables {
        let found: Vec<Stored> = sql_query(format!(
            "SELECT t::text AS value FROM hagency_agent_v1.\"{}\" t",
            table.value
        ))
        .load(&mut db)
        .await
        .unwrap();
        all.insert(
            table.value,
            found.into_iter().map(|row| row.value).collect(),
        );
    }
    all
}
/// Rows that exist on one side only, as `table: row`.
fn differences(before: &Rows, after: &Rows) -> Vec<String> {
    before
        .iter()
        .flat_map(|(table, was)| {
            was.symmetric_difference(&after[table])
                .map(move |row| format!("{table}: {row}"))
        })
        .collect()
}
/// The tables `differences` names.
fn tables(changed: &[String]) -> Vec<&str> {
    changed
        .iter()
        .map(|row| row.split(':').next().unwrap())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
/// One text value of the row `id`.
async fn one(f: &Fixture, sql: &str, id: &str) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(sql)
        .bind::<Text, _>(id)
        .get_result::<Stored>(&mut db)
        .await
        .unwrap()
        .value
}
const CONTRIBUTION: &str = "SELECT state||'/'||coalesce(ledger_replaced_epoch::text,'-') AS value FROM hagency_agent_v1.management_contributions WHERE id=$1";
const REQUEST: &str = "SELECT state AS value FROM hagency_agent_v1.management_requests WHERE id=$1";
const GRANT: &str =
    "SELECT state AS value FROM hagency_agent_v1.management_resource_grants WHERE id=$1";
const AGENT: &str = "SELECT r.state||'/'||coalesce((SELECT i.state FROM hagency_agent_v1.management_agent_intents i WHERE i.request_id=r.id),'-')||'/'||coalesce((SELECT a.state FROM hagency_agent_v1.management_shared_assignments a WHERE a.request_id=r.id),'-') AS value FROM hagency_agent_v1.management_agent_requests r WHERE r.id=$1";
/// Whether the ledger `who` declared at `epoch` swept, and what it recorded.
async fn sweep_record(f: &Fixture, who: &Actor, epoch: i64) -> (bool, Option<Value>) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let row = sql_query("SELECT swept::text||'|'||coalesce(sweep,'') AS value FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2 AND epoch=$3")
        .bind::<Text, _>(&who.p.user_id)
        .bind::<Text, _>(who.p.device_id.as_deref().unwrap())
        .bind::<BigInt, _>(epoch)
        .get_result::<Stored>(&mut db)
        .await
        .unwrap()
        .value;
    let (swept, record) = row.split_once('|').unwrap();
    (
        swept == "true",
        (!record.is_empty()).then(|| serde_json::from_str(record).unwrap()),
    )
}
async fn sweeps_audited(f: &Fixture) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT count(*) AS count FROM hagency_agent_v1.domain_audit WHERE operation='executor.ledger.sweep'")
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
/// A pool `name` of `f.owner`'s device, admitted, open to `manager`, with its
/// grant published as mutation `ledger`.
async fn named_pool(
    f: &Fixture,
    name: &str,
    manager: &Principal,
    ledger: i64,
) -> (Admission, ResourceGrantView) {
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(f, name))
        .await
        .unwrap();
    let a = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve(&format!("admit_{name}"), 1),
        )
        .await
        .unwrap();
    let mut g = grant_input(f, &a, manager).await;
    g.command_id = format!("publish_{name}");
    g.receipt.version = 2;
    g.receipt.admission_request_id = Some(a.request.id.clone());
    g.receipt.ledger_revision = ledger;
    let view = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g)
        .await
        .unwrap();
    (a, view)
}
/// `applicant` asks the pool for an Agent, its owner approves, and the
/// reserve command is leased to the owner device.
async fn asked(
    f: &Fixture,
    a: &Admission,
    applicant: &Principal,
    id: &str,
) -> (PoolAgentRequest, AgentExecutorDelivery) {
    let r = f
        .domain
        .request_pool_agent(applicant, &f.policy, &a.contribution.id, input(a, id))
        .await
        .unwrap();
    decide(f, &r, true).await.unwrap();
    let d = reserve_delivery(f, &r.id).await;
    (r, d)
}
/// What the ledger database of `f.owner`'s device held for one pool: its
/// grant, an Agent it reserved, an Agent whose reserve command it leased and
/// never answered, and a request its owner had not decided.
struct Held {
    pool: Admission,
    grant: ResourceGrantView,
    reserved: PoolAgentRequest,
    reserve: AgentExecutorDelivery,
    acknowledged: AgentExecutorAcknowledgement,
    leased: PoolAgentRequest,
    lease: AgentExecutorDelivery,
    undecided: PoolAgentRequest,
}
/// The pool is mutation 10 of the database and its reserved Agent 11.
async fn held(f: &Fixture, name: &str, applicant: &Principal) -> Held {
    let (pool, grant) = named_pool(f, name, applicant, 10).await;
    let (reserved, reserve) = asked(f, &pool, applicant, &format!("{name}_reserved")).await;
    let acknowledged = report(f, &reserve, 11).await.unwrap();
    let (leased, lease) = asked(f, &pool, applicant, &format!("{name}_leased")).await;
    let undecided = f
        .domain
        .request_pool_agent(
            applicant,
            &f.policy,
            &pool.contribution.id,
            input(&pool, &format!("{name}_undecided")),
        )
        .await
        .unwrap();
    Held {
        pool,
        grant,
        reserved,
        reserve,
        acknowledged,
        leased,
        lease,
        undecided,
    }
}
/// The same people, with the owner's second device as the resource executor.
async fn second_device(f: &Fixture) -> Fixture {
    let auth = Store::open(&f.url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    domain.set_pool_grant_succession(true);
    domain
        .observe_shared_daemon_protocol(&f.coordinator.p, true)
        .await
        .unwrap();
    Fixture {
        owner: same_actor(&auth, &f.coordinator).await,
        coordinator: same_actor(&auth, &f.owner).await,
        admin: same_actor(&auth, &f.admin).await,
        outsider: same_actor(&auth, &f.outsider).await,
        policy: f.policy.clone(),
        url: f.url.clone(),
        auth,
        domain,
    }
}
/// Another owner in the same database, with their own device, coordinator,
/// administrator and applicant.
async fn another_owner(f: &Fixture) -> Fixture {
    let mut other = fixture_at(f.url.clone()).await;
    other.policy = other
        .policy
        .with_project_reservations(true)
        .with_agent_admission(true)
        .with_agent_executor(true)
        .with_pool_grant_succession(true);
    other.domain.set_pool_grant_succession(true);
    other
}
/// The fence receipt of a database that still holds the Agent and reports
/// that it used nothing.
fn fenced(d: &AgentExecutorDelivery, reserved: bool, ledger: i64) -> AgentExecutorResultInput {
    let AgentExecutorCommand::Fence(c) = &d.command else {
        unreachable!()
    };
    AgentExecutorResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentExecutorReceipt::Fence(ManagedAgentFenceReceipt {
            version: 1,
            command_id: c.command_id.clone(),
            command_digest: d.command_digest.clone(),
            reserve_command_digest: c.reserve_command_digest.clone(),
            request_id: c.reserve_command.request_id.clone(),
            agent_id: c.reserve_command.proposed_agent_id.clone(),
            agent_reservation_id: reserved
                .then(|| format!("child_{}", c.reserve_command.proposed_agent_id)),
            state: if reserved { "fenced" } else { "absent_fenced" }.into(),
            fence_revision: ledger,
            ledger_revision: ledger,
            known_consumed_tokens: 0,
            retained_tokens: Some(0),
            usage_complete: true,
            process_state: "not_started".into(),
        }),
    }
}
/// The owner withdraws the pool, and the device answers every fence from the
/// database that still holds the Agents: a completed owner withdrawal.
async fn withdraw(f: &Fixture, h: &Held) {
    let current = f
        .domain
        .contribution(&f.owner.p, &f.policy, &h.pool.contribution.id)
        .await
        .unwrap();
    f.domain
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &h.pool.contribution.id,
            RevokeInput {
                command_id: format!("withdraw_{}", &h.pool.contribution.id[4..20]),
                expected_revision: current.contribution.revision,
            },
        )
        .await
        .unwrap();
    let (mut after, mut ledger, mut answered) = (None, 20, 0);
    for _ in 0..16 {
        let Some(d) = f
            .domain
            .poll_managed_agent_executor(
                &f.owner.p,
                &f.policy,
                ExecutorPollInput {
                    after: after.clone(),
                },
            )
            .await
            .unwrap()
        else {
            break;
        };
        match &d.command {
            AgentExecutorCommand::Fence(c) => {
                let reserved = c.reserve_command.request_id == h.reserved.id;
                f.domain
                    .record_managed_agent_executor_result(
                        &f.owner.p,
                        &f.policy,
                        &c.command_id,
                        fenced(&d, reserved, ledger),
                    )
                    .await
                    .unwrap();
                ledger += 1;
                answered += 1;
                after = Some(c.command_id.clone());
            }
            AgentExecutorCommand::Reserve(c) => after = Some(c.command_id.clone()),
            AgentExecutorCommand::RenewPeriod(c) => after = Some(c.command_id.clone()),
        }
    }
    assert_eq!(answered, 2, "the reserved and the leased Agent are fenced");
}
/// Replaces every value that names one pool, Agent, person or moment.
fn anonymous(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (key, v) in o.iter_mut() {
                if key == "id"
                    || ["Id", "Mxid", "AtMs", "Digest", "Label", "Name"]
                        .iter()
                        .any(|end| key.ends_with(end))
                {
                    if !v.is_null() {
                        *v = json!("set");
                    }
                } else {
                    anonymous(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(anonymous),
        _ => {}
    }
}
/// What `applicant` is shown about one pool Agent request: the request, its
/// removal progress and its assignment.
async fn shown(f: &Fixture, applicant: &Principal, request: &str) -> Value {
    let mut v = json!({
        "request": f.domain.pool_agent_request(applicant, request).await.unwrap(),
        "removal": f.domain.managed_agent_removal(applicant, request).await.unwrap(),
        "assignment": f.domain.managed_agent_assignment(applicant, &f.policy, request).await.unwrap(),
    });
    anonymous(&mut v);
    v
}
/// Whether `applicant` is offered the pool.
async fn offered(f: &Fixture, applicant: &Principal, contribution: &str) -> bool {
    f.domain
        .resource_pools(
            applicant,
            &f.policy,
            CatalogQuery {
                after: None,
                capability: None,
            },
        )
        .await
        .unwrap()
        .items
        .iter()
        .any(|pool| pool.id == contribution && pool.can_request)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn receipt_numbers_collide_only_inside_the_declared_ledger() {
    let f = succession_fixture().await;
    let (a, _) = pool_grant(&f, &f.outsider.p).await;
    let first = approved(&f, &a, "first", 1).await;
    let recorded = report(&f, &first, 100).await.unwrap();
    // Another device's declaration, even the same owner's, moves nothing here.
    assert_eq!(declare(&f, &f.coordinator, 9).await.unwrap().epoch, 1);
    assert_eq!(declare(&f, &f.outsider, 9).await.unwrap().epoch, 1);
    // A device that never declared has one number space, exactly as before.
    let second = approved(&f, &a, "second", 1).await;
    assert_eq!(
        code(report(&f, &second, 100).await),
        "agent_executor_ledger_revision_changed"
    );
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100");
    assert_eq!(stored(&f, GRANTS).await, "0/10");
    assert_eq!(ledgers(&f, &f.owner).await, "");
    // The device replaced its ledger database: the new one counts again, so
    // the number the old database used is accepted once more.
    assert_eq!(
        declare(&f, &f.owner, 1).await.unwrap(),
        ExecutorLedger {
            ledger_id: "01".repeat(32),
            epoch: 1
        }
    );
    report(&f, &second, 100).await.unwrap();
    // Inside the new ledger a number is still used exactly once.
    let third = approved(&f, &a, "third", 1).await;
    assert_eq!(
        code(report(&f, &third, 100).await),
        "agent_executor_ledger_revision_changed"
    );
    report(&f, &third, 101).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101");
    // Exact replay of a receipt from either ledger is acknowledged unchanged,
    // also after a later declaration. It never moves the stored epoch.
    assert_eq!(
        report(&f, &first, 100).await.unwrap().receipt_digest,
        recorded.receipt_digest
    );
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap().epoch, 2);
    assert_eq!(
        report(&f, &first, 100).await.unwrap().receipt_digest,
        recorded.receipt_digest
    );
    report(&f, &second, 100).await.unwrap();
    assert_eq!(
        code(report(&f, &second, 102).await),
        "agent_executor_result_conflict"
    );
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101");
    // The third ledger starts empty as well, and the pool the second one
    // held ended with it: its Agents are requested from a new pool.
    assert_eq!(
        code(
            f.domain
                .request_pool_agent(
                    &f.outsider.p,
                    &f.policy,
                    &a.contribution.id,
                    input(&a, "fourth")
                )
                .await
        ),
        "pool_capacity_unavailable"
    );
    let (b, publish) = second_pool(&f, 99).await;
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &b.contribution.id, publish)
        .await
        .unwrap();
    let fourth = approved(&f, &b, "fourth", 1).await;
    report(&f, &fourth, 100).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101,2/100");
    assert_eq!(ledgers(&f, &f.owner).await, "1/01,2/02");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_declaration_is_idempotent_and_a_retired_ledger_never_returns() {
    let f = succession_fixture().await;
    let one = declare(&f, &f.owner, 1).await.unwrap();
    assert_eq!(
        one,
        ExecutorLedger {
            ledger_id: "01".repeat(32),
            epoch: 1
        }
    );
    assert_eq!(declare(&f, &f.owner, 1).await.unwrap(), one);
    let two = declare(&f, &f.owner, 2).await.unwrap();
    assert_eq!(two.epoch, 2);
    // The first database's numbers are spent; it cannot become current again.
    assert_eq!(
        code(declare(&f, &f.owner, 1).await),
        "agent_executor_ledger_retired"
    );
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap(), two);
    for id in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        format!("{} ", "a".repeat(63)),
    ] {
        assert_eq!(
            code(
                f.domain
                    .declare_executor_ledger(
                        &f.owner.p,
                        &f.policy,
                        ExecutorLedgerInput {
                            ledger_id: id,
                            fresh: false
                        }
                    )
                    .await
            ),
            "invalid_agent_executor_ledger"
        );
    }
    // Epochs belong to one device of one owner: the owner's second device and
    // another account each start at 1, and may name an id used elsewhere.
    assert_eq!(declare(&f, &f.coordinator, 1).await.unwrap().epoch, 1);
    assert_eq!(declare(&f, &f.outsider, 2).await.unwrap().epoch, 1);
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap(), two);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let rows: Count =
        sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_executor_ledgers")
            .get_result(&mut db)
            .await
            .unwrap();
    assert_eq!(rows.count, 4);
    // One audit entry per ledger opened, none for a repeat or a refusal.
    let audited: Count = sql_query("SELECT count(*) AS count FROM hagency_agent_v1.domain_audit WHERE operation='executor.ledger.declare'")
        .get_result(&mut db)
        .await
        .unwrap();
    assert_eq!(audited.count, 4);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn concurrent_declarations_take_distinct_epochs() {
    let f = succession_fixture().await;
    let other = crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    for round in 0..6u8 {
        let (x, y) = tokio::join!(
            f.domain
                .declare_executor_ledger(&f.owner.p, &f.policy, ledger(2 * round + 1)),
            other.declare_executor_ledger(&f.owner.p, &f.policy, ledger(2 * round + 2)),
        );
        let mut epochs = [x.unwrap().epoch, y.unwrap().epoch];
        epochs.sort();
        assert_eq!(epochs, [2 * i64::from(round) + 1, 2 * i64::from(round) + 2]);
    }
    // The same new id arriving twice at once opens one epoch.
    let (x, y) = tokio::join!(
        f.domain
            .declare_executor_ledger(&f.owner.p, &f.policy, ledger(200)),
        other.declare_executor_ledger(&f.owner.p, &f.policy, ledger(200)),
    );
    assert_eq!((x.unwrap().epoch, y.unwrap().epoch), (13, 13));
    assert_eq!(
        ledgers(&f, &f.owner)
            .await
            .split(',')
            .map(|row| row.split('/').next().unwrap().to_owned())
            .collect::<Vec<_>>(),
        (1..=13).map(|epoch| epoch.to_string()).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn declare_route_is_exact_and_device_authenticated() {
    // No executor, admission or succession policy is switched on.
    let f = isolated_fixture().await;
    let http = service(&f, true);
    let path = "executor-ledger/declare";
    let id = "ab".repeat(32);
    // No shared-protocol header is needed.
    let (status, reply) = call(&http, path, Some(&f.owner.token), json!({"ledgerId": id})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&reply["ledgerId"], &reply["epoch"]),
        (&json!(id), &json!(1))
    );
    assert!(reply["observedAtMs"].as_i64().unwrap() > 0);
    assert_eq!(reply.as_object().unwrap().len(), 3);
    let replay = post(
        &http,
        path,
        &f.owner.token,
        json!({"ledgerId": id}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (&replay["ledgerId"], &replay["epoch"]),
        (&json!(id), &json!(1))
    );
    // `fresh` is optional, and on the current id it is only a repeat.
    for fresh in [true, false] {
        let replay = post(
            &http,
            path,
            &f.owner.token,
            json!({"ledgerId": id, "fresh": fresh}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            (&replay["ledgerId"], &replay["epoch"]),
            (&json!(id), &json!(1))
        );
        assert_eq!(replay.as_object().unwrap().len(), 3);
    }
    for fresh in [json!("true"), json!(1), Value::Null] {
        let refused = post(
            &http,
            path,
            &f.owner.token,
            json!({"ledgerId": id, "fresh": fresh}),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(refused["code"], "invalid_management_payload");
    }
    let next = post(
        &http,
        path,
        &f.owner.token,
        json!({"ledgerId": "cd".repeat(32), "fresh": true}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(next["epoch"], 2);
    assert_eq!(sweep_record(&f, &f.owner, 1).await, (false, None));
    assert!(sweep_record(&f, &f.owner, 2).await.0);
    let retired = post(
        &http,
        path,
        &f.owner.token,
        json!({"ledgerId": id}),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(retired["code"], "agent_executor_ledger_retired");
    let malformed = post(
        &http,
        path,
        &f.owner.token,
        json!({"ledgerId": "AB".repeat(32)}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(malformed["code"], "invalid_agent_executor_ledger");
    for body in [
        json!({}),
        json!({"ledgerId": id, "epoch": 7}),
        json!({"ledger_id": id}),
        json!({"ledgerId": 1}),
        json!([]),
    ] {
        let refused = post(&http, path, &f.owner.token, body, StatusCode::BAD_REQUEST).await;
        assert_eq!(refused["code"], "invalid_management_payload");
    }
    let (status, refused) = call(
        &http,
        &format!("{path}?ledgerId={id}"),
        Some(&f.owner.token),
        json!({"ledgerId": id}),
    )
    .await;
    assert_eq!(
        (status, &refused["code"]),
        (
            StatusCode::BAD_REQUEST,
            &json!("invalid_management_payload")
        )
    );
    // Only an enrolled device declares: not a signed-in session, not nobody.
    let (status, refused) = call(&http, path, None, json!({"ledgerId": id})).await;
    assert_eq!(
        (status, &refused["code"]),
        (StatusCode::UNAUTHORIZED, &json!("authentication_required"))
    );
    let (status, refused) =
        call(&http, path, Some(&f.owner.session), json!({"ledgerId": id})).await;
    assert_eq!(
        (status, &refused["code"]),
        (StatusCode::UNAUTHORIZED, &json!("authorization_expired"))
    );
    let (status, refused) = call(&http, path, Some("short"), json!({"ledgerId": id})).await;
    assert_eq!(
        (status, &refused["code"]),
        (StatusCode::UNAUTHORIZED, &json!("authentication_required"))
    );
    let (status, refused) = call(
        &service(&f, false),
        path,
        Some(&f.owner.token),
        json!({"ledgerId": id}),
    )
    .await;
    assert_eq!(
        (status, &refused["code"]),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            &json!("management_not_configured")
        )
    );
    assert_eq!(ledgers(&f, &f.owner).await, "1/ab,2/cd");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_recreated_ledger_publishes_and_reserves_from_its_first_numbers() {
    let f = succession_fixture().await;
    let (a, _) = pool_grant(&f, &f.outsider.p).await;
    let old = approved(&f, &a, "old", 1).await;
    report(&f, &old, 11).await.unwrap();
    let older = approved(&f, &a, "older", 1).await;
    report(&f, &older, 12).await.unwrap();
    // A new pool numbered like a receipt the old database reported is a
    // reused number...
    let (b, publish) = second_pool(&f, 11).await;
    assert_eq!(
        code(
            f.domain
                .publish_management_grant(
                    &f.owner.p,
                    &f.policy,
                    &b.contribution.id,
                    publish.clone()
                )
                .await
        ),
        "agent_executor_ledger_revision_changed"
    );
    // ...until the device declares the database that numbered it.
    declare(&f, &f.owner, 1).await.unwrap();
    f.domain
        .publish_management_grant(&f.owner.p, &f.policy, &b.contribution.id, publish)
        .await
        .unwrap();
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/11");
    // Its Agents reserve at numbers the old database also reported.
    let new = approved(&f, &b, "new", 1).await;
    report(&f, &new, 12).await.unwrap();
    let another = approved(&f, &b, "another", 1).await;
    assert_eq!(
        code(report(&f, &another, 12).await),
        "agent_executor_ledger_revision_changed"
    );
    report(&f, &another, 13).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/11,0/12,1/12,1/13");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn grant_successors_share_the_number_space_of_the_declared_ledger() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let held = approved(&f, &a, "held", 1).await;
    report(&f, &held, 100).await.unwrap();
    // In one ledger an amendment cannot take the number of an Agent receipt.
    assert_eq!(
        code(attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 100)).await),
        "agent_executor_ledger_revision_changed"
    );
    declare(&f, &f.owner, 1).await.unwrap();
    let next = attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 100))
        .await
        .unwrap();
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/100");
    // The new ledger still refuses a number twice, in both arrival orders.
    let late = approved(&f, &a, "late", 2).await;
    report(&f, &late, 150).await.unwrap();
    assert_eq!(
        code(attempt(&f, &a, &next, successor(&next, "amendment", 700_000, 150)).await),
        "agent_executor_ledger_revision_changed"
    );
    attempt(&f, &a, &next, successor(&next, "amendment", 700_000, 160))
        .await
        .unwrap();
    let last = approved(&f, &a, "last", 3).await;
    assert_eq!(
        code(report(&f, &last, 160).await),
        "agent_executor_ledger_revision_changed"
    );
    report(&f, &last, 161).await.unwrap();
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/100,1/160");
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/150,1/161");
}

/// What this change does not do. Epochs separate the reuse check only. Every
/// "numbered after" rule still compares bare numbers, so a chain that began in
/// an earlier ledger continues only above the numbers that ledger recorded.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_chain_from_an_earlier_ledger_still_orders_by_bare_number() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    declare(&f, &f.owner, 1).await.unwrap();
    // The pool's grant is mutation 10 of the earlier ledger.
    let agent = approved(&f, &a, "below", 1).await;
    for number in [1, 10] {
        assert_eq!(
            code(report(&f, &agent, number).await),
            "agent_executor_ledger_revision_changed"
        );
    }
    assert_eq!(
        code(attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 10)).await),
        "invalid_grant_succession"
    );
    report(&f, &agent, 11).await.unwrap();
    attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 12))
        .await
        .unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "1/11");
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/12");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_first_declaration_ends_nothing_unless_the_database_is_fresh() {
    let f = succession_fixture().await;
    let h = held(&f, "native_capacity", &f.outsider.p).await;
    let before = rows(&f).await;
    assert_eq!(declare(&f, &f.owner, 1).await.unwrap().epoch, 1);
    let named = rows(&f).await;
    // The declaration is recorded and audited. Nothing else is written.
    let changed = differences(&before, &named);
    assert_eq!(
        tables(&changed),
        ["domain_audit", "management_executor_ledgers"],
        "{changed:#?}"
    );
    assert_eq!(changed.len(), 2, "{changed:#?}");
    assert_eq!(sweep_record(&f, &f.owner, 1).await, (false, None));
    assert_eq!(sweeps_audited(&f).await, 0);
    // Calling the current database fresh afterwards is a repeat: no sweep.
    for _ in 0..2 {
        assert_eq!(
            declare_fresh(&f, &f.owner, 1).await.unwrap(),
            ExecutorLedger {
                ledger_id: "01".repeat(32),
                epoch: 1
            }
        );
    }
    assert_eq!(differences(&named, &rows(&f).await), Vec::<String>::new());
    // Everything the device held goes on: the leased command is answered,
    // the undecided request is approved and reserved, new Agents are asked for.
    assert!(offered(&f, &f.outsider.p, &h.pool.contribution.id).await);
    report(&f, &h.lease, 12).await.unwrap();
    decide(&f, &h.undecided, true).await.unwrap();
    report(&f, &reserve_delivery(&f, &h.undecided.id).await, 13)
        .await
        .unwrap();
    for request in [&h.reserved, &h.leased, &h.undecided] {
        assert_eq!(
            one(&f, AGENT, &request.id).await,
            "approved_waiting_executor/unavailable/pending_native"
        );
    }
    assert_eq!(
        one(&f, CONTRIBUTION, &h.pool.contribution.id).await,
        "admitted/-"
    );
    assert_eq!(one(&f, GRANT, &h.grant.id).await, "active");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_fresh_first_declaration_ends_what_the_earlier_database_held() {
    let f = succession_fixture().await;
    let applicant = &f.outsider.p;
    // The device that will replace its database: a pool with Agents, an
    // offer the administrator has not decided, and a draft.
    let h = held(&f, "native_capacity", applicant).await;
    let pending = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, offer(&f, "pending_capacity"))
        .await
        .unwrap();
    let mut unpublished = offer(&f, "draft_capacity");
    unpublished.publication_first = true;
    let draft = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, unpublished)
        .await
        .unwrap();
    assert_eq!(
        (
            pending.contribution.state.as_str(),
            draft.contribution.state.as_str()
        ),
        ("pending_admission", "draft")
    );
    // The same owner's second device and another owner host pools too. The
    // other owner's Agent belongs to the owner whose device declares.
    let device = second_device(&f).await;
    let theirs = held(&device, "second_device_capacity", applicant).await;
    let other = another_owner(&f).await;
    let others = held(&other, "other_owner_capacity", &f.owner.p).await;
    // For comparison a third owner withdraws an identical pool, and the
    // device completes the withdrawal from the database that holds it.
    let third = another_owner(&f).await;
    let withdrawn = held(&third, "withdrawn_capacity", &third.outsider.p).await;
    withdraw(&third, &withdrawn).await;

    let before = rows(&f).await;
    assert_eq!(
        declare_fresh(&f, &f.owner, 1).await.unwrap(),
        ExecutorLedger {
            ledger_id: "01".repeat(32),
            epoch: 1
        }
    );
    let after = rows(&f).await;

    // Pools: every contribution of the device is withdrawn and marked.
    for contribution in [&h.pool, &pending, &draft] {
        assert_eq!(
            one(&f, CONTRIBUTION, &contribution.contribution.id).await,
            "revoked/1"
        );
        assert_eq!(one(&f, REQUEST, &contribution.request.id).await, "revoked");
        assert!(!offered(&f, applicant, &contribution.contribution.id).await);
    }
    assert_eq!(one(&f, GRANT, &h.grant.id).await, "revoked");
    // Agents: fenced, and the reserved one's assignment with it. A request
    // keeps its state, as it does when its pool is withdrawn.
    assert_eq!(
        one(&f, AGENT, &h.reserved.id).await,
        "approved_waiting_executor/fenced/fenced"
    );
    assert_eq!(
        one(&f, AGENT, &h.leased.id).await,
        "approved_waiting_executor/fenced/-"
    );
    assert_eq!(
        one(&f, AGENT, &h.undecided.id).await,
        "pending_approval/-/-"
    );
    assert_eq!(
        sweep_record(&f, &f.owner, 1).await,
        (
            true,
            Some(json!({
                "reason": "executor_ledger_replaced",
                "contributionsRevoked": 3,
                "contributionsAlreadyClosed": 0,
                "grantsRevoked": 1,
                "allocationsReleased": 0,
                "parentSettlementsClosed": 0,
                "agentIntentsFenced": 2,
                "agentAssignmentsFenced": 1,
                "sharedProcessesStopped": 0,
                "sharedWorkEnded": 0,
                // The leased reserve command, and the fence command each of
                // the two fenced Agents was given. None is ever delivered.
                "unansweredAgentCommands": 3,
                "unansweredProjectCommands": 0,
                "unansweredParentCommands": 0,
                "unansweredBudgetCommands": 0,
                "unansweredSettlementCommands": 0,
            }))
        )
    );
    assert_eq!(sweeps_audited(&f).await, 1);

    // Nothing of the earlier database is delivered, recovered, decided or
    // recorded again. A receipt it had recorded is still acknowledged.
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .poll_management_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        code(report(&f, &h.lease, 12).await),
        "executor_ledger_replaced"
    );
    let AgentExecutorCommand::Reserve(late) = &h.lease.command else {
        unreachable!()
    };
    assert_eq!(
        code(
            f.domain
                .recover_managed_agent_executor(
                    &f.owner.p,
                    &f.policy,
                    &late.command_id,
                    ExecutorRecoverInput {
                        command_digest: h.lease.command_digest.clone()
                    }
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    assert_eq!(
        report(&f, &h.reserve, 11).await.unwrap().receipt_digest,
        h.acknowledged.receipt_digest
    );
    assert_eq!(
        code(report(&f, &h.reserve, 12).await),
        "agent_executor_result_conflict"
    );
    assert_eq!(
        code(decide(&f, &h.undecided, true).await),
        "pool_capacity_unavailable"
    );
    assert_eq!(
        code(
            f.domain
                .request_pool_agent(
                    applicant,
                    &f.policy,
                    &h.pool.contribution.id,
                    input(&h.pool, "too_late")
                )
                .await
        ),
        "pool_capacity_unavailable"
    );
    assert_eq!(after, rows(&f).await, "a refusal writes nothing");

    // The second device, the other owners and their Agents are not touched:
    // no row that changed names any of them.
    let untouched = [
        device.owner.p.device_id.clone().unwrap(),
        theirs.pool.contribution.id.clone(),
        theirs.grant.id.clone(),
        theirs.reserved.id.clone(),
        theirs.leased.id.clone(),
        theirs.undecided.id.clone(),
        other.owner.p.user_id.clone(),
        others.pool.contribution.id.clone(),
        others.grant.id.clone(),
        others.reserved.id.clone(),
        others.leased.id.clone(),
        others.undecided.id.clone(),
        third.owner.p.user_id.clone(),
    ];
    let changed = differences(&before, &after);
    assert_eq!(
        tables(&changed),
        [
            "domain_audit",
            "management_agent_executor_commands",
            "management_agent_intents",
            "management_agent_matrix",
            "management_contributions",
            "management_executor_ledgers",
            "management_outbox",
            "management_requests",
            "management_resource_grants",
            "management_shared_assignments",
        ]
    );
    for row in &changed {
        assert!(
            !untouched.iter().any(|id| row.contains(id.as_str())),
            "{row}"
        );
    }
    for (world, held, who) in [(&device, &theirs, applicant), (&other, &others, &f.owner.p)] {
        assert_eq!(
            one(&f, CONTRIBUTION, &held.pool.contribution.id).await,
            "admitted/-"
        );
        assert_eq!(one(&f, GRANT, &held.grant.id).await, "active");
        assert_eq!(
            one(&f, AGENT, &held.reserved.id).await,
            "approved_waiting_executor/unavailable/pending_native"
        );
        assert!(offered(world, who, &held.pool.contribution.id).await);
        // Their databases answer and reserve as before.
        report(world, &held.lease, 12).await.unwrap();
        decide(world, &held.undecided, true).await.unwrap();
        report(
            world,
            &reserve_delivery(world, &held.undecided.id).await,
            13,
        )
        .await
        .unwrap();
    }

    // The applicant sees each Agent as after the completed owner withdrawal.
    for (swept, reference) in [
        (&h.reserved, &withdrawn.reserved),
        (&h.leased, &withdrawn.leased),
        (&h.undecided, &withdrawn.undecided),
    ] {
        let swept = shown(&f, applicant, &swept.id).await;
        let mut reference = shown(&third, &third.outsider.p, &reference.id).await;
        // One part differs and is not papered over: no fence receipt exists
        // for the replaced database, so none is shown. Its fence command
        // stays unanswered where the withdrawal shows the device's receipt.
        if !reference["removal"]["nativeFence"].is_null() {
            assert_eq!(
                reference["removal"]["nativeFence"]["deliveryState"],
                "recorded"
            );
            assert_eq!(reference["removal"]["usage"]["fenceUsageComplete"], true);
            reference["removal"]["nativeFence"] = json!({"deliveryState": "pending", "ledgerRevision": null, "nativeState": null, "processState": null});
            reference["removal"]["usage"]["fenceUsageComplete"] = Value::Null;
        }
        assert_eq!(swept, reference);
    }
    // The reserved Agent is shown fenced with nothing held and nothing used.
    let reserved = shown(&f, applicant, &h.reserved.id).await;
    assert_eq!(
        (
            &reserved["request"]["membershipState"],
            &reserved["assignment"]["state"],
            &reserved["request"]["blockingReason"]
        ),
        (
            &json!("fenced"),
            &json!("fenced"),
            &json!("pool_capacity_unavailable")
        )
    );
    assert_eq!(
        reserved["removal"]["usage"],
        json!({"fenceUsageComplete": null, "knownConsumedTokens": 0, "periodKey": h.reserved.period_key, "requiresReconciliation": false, "retainedTokensFloor": 0})
    );
    // The applicant can remove each Agent, and that asks nothing of the device.
    for request in [&h.reserved, &h.leased, &h.undecided] {
        let current = f
            .domain
            .pool_agent_request(applicant, &request.id)
            .await
            .unwrap();
        let removed = f
            .domain
            .stop_pool_agent(
                applicant,
                &request.id,
                RevokeInput {
                    command_id: format!("remove_{}", &request.id[4..20]),
                    expected_revision: current.revision,
                },
            )
            .await
            .unwrap();
        assert_eq!(removed.state, "cancelled");
    }
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_later_declaration_sweeps_without_being_called_fresh_and_a_repeat_never_does() {
    let f = succession_fixture().await;
    // The first declaration names the database the device always had.
    assert_eq!(declare(&f, &f.owner, 1).await.unwrap().epoch, 1);
    let old = held(&f, "native_capacity", &f.outsider.p).await;
    // The pool's capacity was amended once: its first grant is superseded.
    let amended = amend(&f, &old.pool, &old.grant, 800_000, 12).await;
    assert_eq!(stored(&f, GRANTS).await, "1/10,1/12");
    // A different id afterwards can only be a new database.
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap().epoch, 2);
    assert_eq!(
        one(&f, CONTRIBUTION, &old.pool.contribution.id).await,
        "revoked/2"
    );
    assert_eq!(one(&f, GRANT, &old.grant.id).await, "superseded");
    assert_eq!(one(&f, GRANT, &amended.id).await, "revoked");
    assert_eq!(
        one(&f, AGENT, &old.reserved.id).await,
        "approved_waiting_executor/fenced/fenced"
    );
    assert_eq!(
        one(&f, AGENT, &old.leased.id).await,
        "approved_waiting_executor/fenced/-"
    );
    assert_eq!(
        code(report(&f, &old.lease, 13).await),
        "executor_ledger_replaced"
    );
    assert_eq!(sweep_record(&f, &f.owner, 1).await, (false, None));
    let (swept, record) = sweep_record(&f, &f.owner, 2).await;
    assert!(swept);
    assert_eq!(record.unwrap()["contributionsRevoked"], 1);
    assert_eq!(sweeps_audited(&f).await, 1);
    // What the second database holds is not swept by repeating its id,
    // whatever the repeat claims.
    let new = held(&f, "recreated_capacity", &f.outsider.p).await;
    let before = rows(&f).await;
    for _ in 0..2 {
        assert_eq!(declare_fresh(&f, &f.owner, 2).await.unwrap().epoch, 2);
        assert_eq!(declare(&f, &f.owner, 2).await.unwrap().epoch, 2);
    }
    assert_eq!(differences(&before, &rows(&f).await), Vec::<String>::new());
    assert_eq!(
        one(&f, CONTRIBUTION, &new.pool.contribution.id).await,
        "admitted/-"
    );
    report(&f, &new.lease, 12).await.unwrap();
    assert_eq!(stored(&f, GRANTS).await, "1/10,1/12,2/10");
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "1/11,2/11,2/12");
    // A third database ends the second one's pool and leaves the mark of
    // the first sweep where it was.
    assert_eq!(declare(&f, &f.owner, 3).await.unwrap().epoch, 3);
    assert_eq!(
        one(&f, CONTRIBUTION, &old.pool.contribution.id).await,
        "revoked/2"
    );
    assert_eq!(
        one(&f, CONTRIBUTION, &new.pool.contribution.id).await,
        "revoked/3"
    );
    assert_eq!(sweeps_audited(&f).await, 2);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_swept_device_offers_the_same_resource_again_and_counts_from_one() {
    let f = succession_fixture().await;
    let applicant = &f.outsider.p;
    let old = held(&f, "native_capacity", applicant).await;
    declare_fresh(&f, &f.owner, 1).await.unwrap();
    // The new database offers the same resource. It sends its own command
    // ids, and the old offer does not stand in its way.
    let mut again = offer(&f, "native_capacity");
    again.command_id = "offer_native_capacity_again".into();
    let a = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, again)
        .await
        .unwrap();
    assert_ne!(a.contribution.id, old.pool.contribution.id);
    let a = f
        .domain
        .decide_management_request(
            &f.admin.p,
            &f.policy,
            &a.request.id,
            approve("admit_native_capacity_again", 1),
        )
        .await
        .unwrap();
    // Its grant is mutation 1 and its first Agent mutation 2: below every
    // number the earlier database reported.
    let mut publish = grant_input(&f, &a, applicant).await;
    publish.command_id = "publish_native_capacity_again".into();
    publish.receipt.version = 2;
    publish.receipt.admission_request_id = Some(a.request.id.clone());
    publish.receipt.ledger_revision = 1;
    let grant = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, publish)
        .await
        .unwrap();
    assert!(offered(&f, applicant, &a.contribution.id).await);
    let (agent, reserve) = asked(&f, &a, applicant, "again_reserved").await;
    assert!(reserve.execution_allowed);
    assert_eq!(
        code(report(&f, &reserve, 1).await),
        "agent_executor_ledger_revision_changed"
    );
    assert!(report(&f, &reserve, 2).await.unwrap().assignment.is_some());
    assert_eq!(
        one(&f, AGENT, &agent.id).await,
        "approved_waiting_executor/unavailable/pending_native"
    );
    // What this change does not give back: a room that had a pool Agent
    // takes no other pool Agent, whatever became of the first one. That rule
    // is older than this change and holds after an owner withdrawal too, so
    // the room of a swept Agent stays closed to a new one, also after its
    // applicant removed the swept Agent.
    let mut same_room = input(&a, "again_same_room");
    same_room.room_id = old.reserved.room_id.clone();
    for removed in [false, true] {
        if removed {
            let swept = f
                .domain
                .pool_agent_request(applicant, &old.reserved.id)
                .await
                .unwrap();
            f.domain
                .stop_pool_agent(
                    applicant,
                    &old.reserved.id,
                    RevokeInput {
                        command_id: "remove_swept_agent".into(),
                        expected_revision: swept.revision,
                    },
                )
                .await
                .unwrap();
        }
        let refused = f
            .domain
            .request_pool_agent(applicant, &f.policy, &a.contribution.id, same_room.clone())
            .await
            .unwrap_err();
        assert!(
            format!("{refused:?}").contains("pool_agent_room_used"),
            "{refused:?}"
        );
    }
    // The owner withdraws the new pool and the device fences its Agent: a
    // whole cycle inside the new database.
    let current = f
        .domain
        .contribution(&f.owner.p, &f.policy, &a.contribution.id)
        .await
        .unwrap();
    f.domain
        .revoke_contribution(
            &f.owner.p,
            &f.policy,
            &a.contribution.id,
            RevokeInput {
                command_id: "withdraw_native_capacity_again".into(),
                expected_revision: current.contribution.revision,
            },
        )
        .await
        .unwrap();
    let fence = f
        .domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .expect("the fence of the new Agent");
    let AgentExecutorCommand::Fence(c) = &fence.command else {
        panic!("the earlier database's commands are not delivered")
    };
    assert_eq!(c.reserve_command.request_id, agent.id);
    f.domain
        .record_managed_agent_executor_result(
            &f.owner.p,
            &f.policy,
            &c.command_id,
            fenced(&fence, true, 3),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(one(&f, GRANT, &grant.id).await, "revoked");
    assert_eq!(one(&f, CONTRIBUTION, &a.contribution.id).await, "revoked/-");
    assert_eq!(
        one(&f, AGENT, &agent.id).await,
        "approved_waiting_executor/fenced/fenced"
    );
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/1");
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/11,1/2,1/3");
    // The earlier database's objects stayed ended throughout.
    assert_eq!(
        one(&f, CONTRIBUTION, &old.pool.contribution.id).await,
        "revoked/1"
    );
    assert_eq!(
        code(report(&f, &old.lease, 4).await),
        "executor_ledger_replaced"
    );
    // Only a replaced database frees its resource id. The pool this
    // database withdrew itself still holds the id, as before this change.
    let mut third = offer(&f, "native_capacity");
    third.command_id = "offer_native_capacity_third".into();
    assert_eq!(
        code(
            f.domain
                .create_contribution(&f.owner.p, &f.policy, third)
                .await
        ),
        "contribution_already_exists"
    );
    // When that database is replaced in turn, the pool it had already
    // closed is marked as well, and the id is free for the next one.
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap().epoch, 2);
    assert_eq!(one(&f, CONTRIBUTION, &a.contribution.id).await, "revoked/2");
    assert_eq!(
        one(&f, CONTRIBUTION, &old.pool.contribution.id).await,
        "revoked/1"
    );
    let swept = sweep_record(&f, &f.owner, 2).await.1.unwrap();
    assert_eq!(
        (
            &swept["contributionsRevoked"],
            &swept["contributionsAlreadyClosed"],
            &swept["grantsRevoked"],
            &swept["agentIntentsFenced"]
        ),
        (&json!(0), &json!(1), &json!(0), &json!(0))
    );
    let mut fourth = offer(&f, "native_capacity");
    fourth.command_id = "offer_native_capacity_fourth".into();
    let fourth = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, fourth)
        .await
        .unwrap();
    assert_eq!(
        one(&f, CONTRIBUTION, &fourth.contribution.id).await,
        "pending_admission/-"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_sweep_leaves_the_owners_own_local_agents_alone() {
    let f = succession_fixture().await;
    let now = crate::api::now_ms();
    // A local Agent of the declaring owner, executed by the declaring device.
    let local = f
        .domain
        .create_agent(
            &f.owner.p,
            crate::domain::CreateAgent {
                visibility: Default::default(),
                display_name: "Local helper".into(),
                idempotency_key: "local_helper".into(),
            },
            now,
        )
        .await
        .unwrap();
    let local = f
        .domain
        .set_execution_device(
            &f.owner.p,
            &local.id,
            crate::domain::SetExecutionDevice {
                expected_generation: local.generation,
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(local.execution_device_id, f.owner.p.device_id);
    let h = held(&f, "native_capacity", &f.outsider.p).await;
    let before = rows(&f).await;
    declare_fresh(&f, &f.owner, 1).await.unwrap();
    let after = rows(&f).await;
    // The sweep ran: the pooled Agent of the same device is fenced.
    assert_eq!(
        one(&f, AGENT, &h.reserved.id).await,
        "approved_waiting_executor/fenced/fenced"
    );
    // No row that names the local Agent changed, and the tables that hold
    // local Agents and their rooms, settings and devices did not change at all.
    for row in differences(&before, &after) {
        assert!(
            !row.contains(local.id.as_str()) && !row.contains(local.puppet_mxid.as_str()),
            "{row}"
        );
    }
    assert!(
        before["agents"]
            .iter()
            .any(|row| row.contains(local.id.as_str()))
    );
    for table in [
        "agents",
        "bindings",
        "configuration_requests",
        "personal_e2ee_candidates",
        "executor_matrix_devices",
        "devices",
    ] {
        assert_eq!(before[table], after[table], "{table}");
    }
    let same = f
        .domain
        .agent(&f.owner.p, &local.id, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(
        (
            same.state,
            same.generation,
            same.execution_device_id,
            same.puppet_mxid
        ),
        (
            local.state,
            local.generation,
            local.execution_device_id,
            local.puppet_mxid
        )
    );
}

/// A project that reserved capacity from the owner's grant.
async fn project(f: &Fixture) -> (ResourceGrantView, ProjectPlan, ProjectAllocation) {
    let a = admitted(f).await;
    let plan = approved_plan(f, &a, &f.outsider.p).await;
    let grant = f
        .domain
        .publish_management_grant(
            &f.owner.p,
            &f.policy,
            &a.contribution.id,
            grant_input(f, &a, &f.outsider.p).await,
        )
        .await
        .unwrap();
    let wanted = allocation(f, &plan).await;
    (grant, plan, accepted(f, &wanted).await)
}

const PLAN: &str = "SELECT state AS value FROM hagency_agent_v1.management_projects WHERE id=$1";
/// The parent settlement of a grant whose database was replaced: closed,
/// nothing held, and no receipt where the device reported none.
async fn closed_parent(f: &Fixture, grant: &ResourceGrantView) {
    for who in [&f.owner.p, &f.admin.p] {
        let parent = f
            .domain
            .management_parent_settlement(who, &f.policy, &grant.id)
            .await
            .unwrap();
        assert_eq!(
            (
                parent.state.as_str(),
                parent.held_tokens,
                parent.known_unused_released_tokens,
                parent.unresolved_tokens,
                parent.can_close,
                parent.can_settle
            ),
            ("closed", 0, 1_000_000, None, false, false)
        );
        assert!(
            parent.receipt.is_none()
                && parent.receipt_digest.is_none()
                && parent.known_consumed_tokens.is_none()
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_fresh_ledger_closes_project_capacity_and_parent_settlements() {
    // A project reserve command leased and never answered.
    let mut f = succession_fixture().await;
    f.policy = f.policy.with_parent_settlement(true);
    let (grant, plan, wanted) = project(&f).await;
    let reserve = delivered(&f).await;
    assert_eq!(
        (reserve.command.operation.as_str(), wanted.state.as_str()),
        ("project.reserve", "approved_waiting_executor")
    );
    declare_fresh(&f, &f.owner, 1).await.unwrap();
    let ended = allocation(&f, &plan).await;
    assert_eq!(
        (ended.state.as_str(), ended.can_release, ended.can_decide),
        ("released", false, false)
    );
    assert!(ended.result.is_none());
    // The project plan ends as it does when its pool is withdrawn.
    assert_eq!(one(&f, PLAN, &plan.id).await, "revoked");
    assert_eq!(one(&f, GRANT, &grant.id).await, "revoked");
    assert!(
        f.domain
            .poll_management_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        code(
            f.domain
                .record_management_executor_result(
                    &f.owner.p,
                    &f.policy,
                    &reserve.command.command_id,
                    result(&reserve, "reserved", 11)
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    assert_eq!(
        code(
            f.domain
                .recover_management_executor(
                    &f.owner.p,
                    &f.policy,
                    &reserve.command.command_id,
                    ExecutorRecoverInput {
                        command_digest: reserve.command_digest.clone()
                    }
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    closed_parent(&f, &grant).await;
    let swept = sweep_record(&f, &f.owner, 1).await.1.unwrap();
    assert_eq!(
        (
            &swept["contributionsRevoked"],
            &swept["grantsRevoked"],
            &swept["allocationsReleased"],
            &swept["parentSettlementsClosed"],
            &swept["unansweredProjectCommands"],
            &swept["unansweredParentCommands"]
        ),
        (
            &json!(1),
            &json!(1),
            &json!(1),
            &json!(0),
            &json!(1),
            &json!(0)
        )
    );

    // Reserved capacity whose owner then withdrew the grant: the release and
    // the parent close are both leased, and neither is answered.
    let mut f = succession_fixture().await;
    f.policy = f.policy.with_parent_settlement(true);
    let (grant, plan, _) = project(&f).await;
    let reserve = delivered(&f).await;
    record(&f, &reserve, "reserved", 11).await;
    let closing = f
        .domain
        .close_management_parent(
            &f.owner.p,
            &f.policy,
            &grant.id,
            ParentCloseInput {
                command_id: "close_parent".into(),
                expected_grant_revision: grant.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (closing.state.as_str(), closing.held_tokens),
        ("closing", 1_000_000)
    );
    let reserved = allocation(&f, &plan).await;
    assert_eq!(reserved.state, "releasing");
    let release = delivered(&f).await;
    let close = f
        .domain
        .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            release.command.operation.as_str(),
            close.command.operation.as_str()
        ),
        ("project.release", "grant.close")
    );
    declare_fresh(&f, &f.owner, 1).await.unwrap();
    // The allocation is released. Its recorded reserve receipt is unchanged.
    let ended = allocation(&f, &plan).await;
    assert_eq!(
        (ended.state.as_str(), ended.can_release, ended.can_decide),
        ("released", false, false)
    );
    assert_eq!(
        serde_json::to_value(&ended.result).unwrap(),
        serde_json::to_value(&reserved.result).unwrap()
    );
    assert_eq!(ended.result.as_ref().unwrap().ledger_revision, 11);
    assert_eq!(one(&f, PLAN, &plan.id).await, "revoked");
    closed_parent(&f, &grant).await;
    // Neither leased command is delivered, recovered or answered again.
    assert!(
        f.domain
            .poll_management_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .poll_management_parent_executor(&f.owner.p, &f.policy, Default::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        code(
            f.domain
                .record_management_executor_result(
                    &f.owner.p,
                    &f.policy,
                    &release.command.command_id,
                    result(&release, "released", 12)
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    let settled = ParentResultInput {
        command_digest: close.command_digest.clone(),
        token: close.token.clone(),
        receipt: NativeGrantSettlementReceipt {
            version: 1,
            command_id: close.command.command_id.clone(),
            command_digest: close.command_digest.clone(),
            grant_id: grant.id.clone(),
            grant_receipt_digest: grant.receipt_digest.clone(),
            enrollment_id: grant.receipt.enrollment_id.clone(),
            period_key: grant.receipt.period_key.clone(),
            fence_revision: 12,
            ledger_revision: 13,
            state: "closed".into(),
            retained_tokens: 0,
            known_consumed_tokens: 0,
            unresolved_tokens: 0,
            liability_count: 0,
            liability_digest: "b".repeat(64),
        },
    };
    assert_eq!(
        code(
            f.domain
                .record_management_parent_result(
                    &f.owner.p,
                    &f.policy,
                    &close.command.command_id,
                    settled
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    assert_eq!(
        code(
            f.domain
                .recover_management_parent_executor(
                    &f.owner.p,
                    &f.policy,
                    &close.command.command_id,
                    ParentRecoverInput {
                        command_digest: close.command_digest.clone()
                    }
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    assert_eq!(
        code(
            f.domain
                .settle_management_parent(
                    &f.owner.p,
                    &f.policy,
                    &grant.id,
                    RevokeInput {
                        command_id: "settle_parent".into(),
                        expected_revision: 2
                    }
                )
                .await
        ),
        "executor_ledger_replaced"
    );
    let swept = sweep_record(&f, &f.owner, 1).await.1.unwrap();
    // The owner had revoked the grant already.
    assert_eq!(
        (
            &swept["contributionsRevoked"],
            &swept["grantsRevoked"],
            &swept["allocationsReleased"],
            &swept["parentSettlementsClosed"],
            &swept["unansweredProjectCommands"],
            &swept["unansweredParentCommands"]
        ),
        (
            &json!(1),
            &json!(0),
            &json!(1),
            &json!(1),
            &json!(1),
            &json!(1)
        )
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn the_schema_keeps_a_sweep_and_its_marks_as_they_were_written() {
    let f = succession_fixture().await;
    let h = held(&f, "native_capacity", &f.outsider.p).await;
    declare_fresh(&f, &f.owner, 1).await.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    for (statement, refusal) in [
        // A swept pool does not reopen and its mark does not move.
        (
            "UPDATE hagency_agent_v1.management_contributions SET state='admitted' WHERE ledger_replaced_epoch IS NOT NULL",
            "executor_ledger_replaced",
        ),
        (
            "UPDATE hagency_agent_v1.management_contributions SET ledger_replaced_epoch=NULL WHERE ledger_replaced_epoch IS NOT NULL",
            "executor_ledger_replaced",
        ),
        // The declaration and its sweep record are history.
        (
            "DELETE FROM hagency_agent_v1.management_executor_ledgers",
            "permanent_executor_ledger_history",
        ),
        (
            "UPDATE hagency_agent_v1.management_executor_ledgers SET sweep='{}'",
            "immutable_executor_ledger",
        ),
        (
            "UPDATE hagency_agent_v1.management_executor_ledgers SET swept=false,sweep=NULL",
            "immutable_executor_ledger",
        ),
        (
            "UPDATE hagency_agent_v1.management_executor_ledgers SET declared_at_ms=declared_at_ms+1",
            "immutable_executor_ledger",
        ),
        // A command of the replaced database never takes a receipt, even
        // written past the service.
        (
            "UPDATE hagency_agent_v1.management_agent_executor_commands SET receipt='{}' WHERE receipt IS NULL",
            "executor_ledger_replaced",
        ),
    ] {
        let refused = sql_query(statement)
            .execute(&mut db)
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains(refusal), "{statement}: {refused}");
    }
    // An open pool cannot be given the mark.
    let mut again = offer(&f, "native_capacity");
    again.command_id = "offer_native_capacity_again".into();
    let open = f
        .domain
        .create_contribution(&f.owner.p, &f.policy, again)
        .await
        .unwrap();
    let refused = sql_query(
        "UPDATE hagency_agent_v1.management_contributions SET ledger_replaced_epoch=1 WHERE id=$1",
    )
    .bind::<Text, _>(&open.contribution.id)
    .execute(&mut db)
    .await
    .unwrap_err()
    .to_string();
    assert!(
        refused.contains("management_contribution_replaced_closed"),
        "{refused}"
    );
    assert_eq!(
        one(&f, CONTRIBUTION, &open.contribution.id).await,
        "pending_admission/-"
    );
    assert_eq!(
        one(&f, CONTRIBUTION, &h.pool.contribution.id).await,
        "revoked/1"
    );
}

#[derive(QueryableByName)]
struct LedgerSchema {
    #[diesel(sql_type = BigInt)]
    version: i64,
    #[diesel(sql_type = BigInt)]
    columns: i64,
    #[diesel(sql_type = Bool)]
    declared: bool,
    #[diesel(sql_type = Bool)]
    exempt: bool,
    #[diesel(sql_type = BigInt)]
    sweep: i64,
    #[diesel(sql_type = Bool)]
    one_offer: bool,
}
/// Deployment version, how many epoch columns exist, whether the ledger table
/// exists, whether the command guard knows the epoch column, how many of the
/// nine sweep objects exist (two ledger columns, the contribution mark, its
/// two constraints and its guard, the partial index and the two lookups), and
/// whether schema 34's rule of one offer per resource id is in force.
async fn ledger_schema(db: &mut AsyncPgConnection) -> (i64, i64, bool, bool, i64, bool) {
    let s: LedgerSchema = sql_query("SELECT version::bigint AS version,(SELECT count(*) FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND column_name IN ('ledger_epoch','receipt_ledger_epoch'))::bigint AS columns,to_regclass('hagency_agent_v1.management_executor_ledgers') IS NOT NULL AS declared,position('receipt_ledger_epoch' in pg_get_functiondef('hagency_agent_v1.retain_agent_executor()'::regprocedure))>0 AS exempt,((SELECT count(*) FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND (table_name,column_name) IN (('management_executor_ledgers','swept'),('management_executor_ledgers','sweep'),('management_contributions','ledger_replaced_epoch')))+(SELECT count(*) FROM pg_constraint WHERE conrelid='hagency_agent_v1.management_contributions'::regclass AND conname IN ('management_contribution_replaced_ledger','management_contribution_replaced_closed'))+(SELECT count(*) FROM pg_trigger WHERE tgrelid='hagency_agent_v1.management_contributions'::regclass AND tgname='retain_replaced_contribution')+(SELECT count(*) FROM pg_indexes WHERE schemaname='hagency_agent_v1' AND indexname='management_contribution_resource')+(SELECT count(*) FROM pg_proc WHERE pronamespace='hagency_agent_v1'::regnamespace AND proname IN ('executor_ledger_replaced_grant','executor_ledger_replaced_request')))::bigint AS sweep,EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='hagency_agent_v1.management_contributions'::regclass AND contype='u' AND pg_get_constraintdef(oid)='UNIQUE (owner_user_id, executor_device_id, resource_id)') AS one_offer FROM hagency_agent_v1.management_deployment")
        .get_result(db)
        .await
        .unwrap();
    (
        s.version,
        s.columns,
        s.declared,
        s.exempt,
        s.sweep,
        s.one_offer,
    )
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn schema_35_upgrades_a_populated_schema_34_database_into_epoch_zero() {
    let f = succession_fixture().await;
    let (a, root) = pool_grant(&f, &f.outsider.p).await;
    let kept = approved(&f, &a, "kept", 1).await;
    let recorded = report(&f, &kept, 100).await.unwrap();
    let waiting = approved(&f, &a, "waiting", 1).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true, 9, false));
    // Back to schema 34 while nothing is declared: no epoch other than 0 is lost.
    db.batch_execute(include_str!("management_executor_ledgers_down.sql"))
        .await
        .unwrap();
    assert_eq!(ledger_schema(&mut db).await, (34, 0, false, false, 0, true));
    // Opening the store migrates the populated database, and is repeatable.
    for _ in 0..2 {
        crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap();
        assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true, 9, false));
    }
    let f = Fixture {
        domain: crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .unwrap(),
        ..f
    };
    f.domain.set_pool_grant_succession(true);
    // Every receipt recorded before the upgrade is in epoch 0 and behaves as
    // it did: exact replay is acknowledged, its number is still taken.
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100");
    assert_eq!(stored(&f, GRANTS).await, "0/10");
    assert_eq!(
        report(&f, &kept, 100).await.unwrap().receipt_digest,
        recorded.receipt_digest
    );
    assert_eq!(
        code(report(&f, &waiting, 100).await),
        "agent_executor_ledger_revision_changed"
    );
    assert_eq!(
        code(attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 100)).await),
        "agent_executor_ledger_revision_changed"
    );
    // The guard on recorded commands covers the new column: an epoch is
    // written only together with its receipt and never changes afterwards.
    for filter in ["receipt IS NOT NULL", "receipt IS NULL"] {
        let refused = sql_query(format!("UPDATE hagency_agent_v1.management_agent_executor_commands SET receipt_ledger_epoch=receipt_ledger_epoch+1 WHERE {filter}"))
            .execute(&mut db)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("immutable_agent_executor_command"),
            "{filter}: {refused}"
        );
    }
    declare(&f, &f.owner, 1).await.unwrap();
    report(&f, &waiting, 100).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100");
    // Schema 34 cannot describe a declared ledger, so the fixture refuses.
    let refused = db
        .batch_execute(include_str!("management_executor_ledgers_down.sql"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("executor_ledger_downgrade_has_epoch"),
        "{refused}"
    );
    assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true, 9, false));
}
