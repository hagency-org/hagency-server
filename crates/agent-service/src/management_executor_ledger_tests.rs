//! Owner ledger epochs: a receipt number is refused as reused only inside the
//! ledger database its device last declared.
use super::*;

const AGENT_RECEIPTS: &str = "SELECT coalesce(string_agg(receipt_ledger_epoch||'/'||(receipt::jsonb->>'ledgerRevision'),',' ORDER BY receipt_ledger_epoch,(receipt::jsonb->>'ledgerRevision')::bigint),'') AS value FROM hagency_agent_v1.management_agent_executor_commands WHERE receipt IS NOT NULL";
const GRANTS: &str = "SELECT coalesce(string_agg(ledger_epoch||'/'||(receipt::jsonb->>'ledgerRevision'),',' ORDER BY ledger_epoch,(receipt::jsonb->>'ledgerRevision')::bigint),'') AS value FROM hagency_agent_v1.management_resource_grants";

/// `epoch/number` of every stored row the query selects, in order.
async fn stored(f: &Fixture, sql: &str) -> String {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(sql).get_result::<Stored>(&mut db).await.unwrap().value
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
    ExecutorLedgerInput { ledger_id: format!("{n:02x}").repeat(32) }
}
async fn declare(f: &Fixture, who: &Actor, n: u8) -> crate::Result<ExecutorLedger> {
    f.domain.declare_executor_ledger(&who.p, ledger(n)).await
}
/// Request, approve and deliver the reserve command of one pool Agent.
async fn approved(f: &Fixture, a: &Admission, id: &str, grant_revision: i64) -> AgentExecutorDelivery {
    let r = request_agent(f, a, id, 100, grant_revision).await;
    decide(f, &r, true).await.unwrap();
    let d = reserve_delivery(f, &r.id).await;
    assert!(d.execution_allowed);
    d
}
fn reserved(d: &AgentExecutorDelivery, ledger: i64) -> AgentExecutorResultInput {
    let AgentExecutorCommand::Reserve(c) = &d.command else { unreachable!() };
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
async fn report(f: &Fixture, d: &AgentExecutorDelivery, ledger: i64) -> crate::Result<AgentExecutorAcknowledgement> {
    let AgentExecutorCommand::Reserve(c) = &d.command else { unreachable!() };
    f.domain
        .record_managed_agent_executor_result(&f.owner.p, &f.policy, &c.command_id, reserved(d, ledger))
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
        .decide_management_request(&f.admin.p, &f.policy, &a.request.id, approve("admit_recreated", 1))
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
    let mut request = TestClient::post(format!("https://example.test/hagency/api/v1/management/{path}"))
        .add_header("host", "example.test", true);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let mut r = Box::pin(request.json(&body).send(s)).await;
    (r.status_code.unwrap(), r.take_json::<Value>().await.unwrap())
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
    assert_eq!(code(report(&f, &second, 100).await), "agent_executor_ledger_revision_changed");
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100");
    assert_eq!(stored(&f, GRANTS).await, "0/10");
    assert_eq!(ledgers(&f, &f.owner).await, "");
    // The device replaced its ledger database: the new one counts again, so
    // the number the old database used is accepted once more.
    assert_eq!(declare(&f, &f.owner, 1).await.unwrap(), ExecutorLedger { ledger_id: "01".repeat(32), epoch: 1 });
    report(&f, &second, 100).await.unwrap();
    // Inside the new ledger a number is still used exactly once.
    let third = approved(&f, &a, "third", 1).await;
    assert_eq!(code(report(&f, &third, 100).await), "agent_executor_ledger_revision_changed");
    report(&f, &third, 101).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101");
    // Exact replay of a receipt from either ledger is acknowledged unchanged,
    // also after a later declaration. It never moves the stored epoch.
    assert_eq!(report(&f, &first, 100).await.unwrap().receipt_digest, recorded.receipt_digest);
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap().epoch, 2);
    assert_eq!(report(&f, &first, 100).await.unwrap().receipt_digest, recorded.receipt_digest);
    report(&f, &second, 100).await.unwrap();
    assert_eq!(code(report(&f, &second, 102).await), "agent_executor_result_conflict");
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101");
    // The third ledger starts empty as well.
    let fourth = approved(&f, &a, "fourth", 1).await;
    report(&f, &fourth, 100).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100,1/100,1/101,2/100");
    assert_eq!(ledgers(&f, &f.owner).await, "1/01,2/02");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_declaration_is_idempotent_and_a_retired_ledger_never_returns() {
    let f = succession_fixture().await;
    let one = declare(&f, &f.owner, 1).await.unwrap();
    assert_eq!(one, ExecutorLedger { ledger_id: "01".repeat(32), epoch: 1 });
    assert_eq!(declare(&f, &f.owner, 1).await.unwrap(), one);
    let two = declare(&f, &f.owner, 2).await.unwrap();
    assert_eq!(two.epoch, 2);
    // The first database's numbers are spent; it cannot become current again.
    assert_eq!(code(declare(&f, &f.owner, 1).await), "agent_executor_ledger_retired");
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
            code(f.domain.declare_executor_ledger(&f.owner.p, ExecutorLedgerInput { ledger_id: id }).await),
            "invalid_agent_executor_ledger"
        );
    }
    // Epochs belong to one device of one owner: the owner's second device and
    // another account each start at 1, and may name an id used elsewhere.
    assert_eq!(declare(&f, &f.coordinator, 1).await.unwrap().epoch, 1);
    assert_eq!(declare(&f, &f.outsider, 2).await.unwrap().epoch, 1);
    assert_eq!(declare(&f, &f.owner, 2).await.unwrap(), two);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let rows: Count = sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_executor_ledgers")
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
    let other = crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
    for round in 0..6u8 {
        let (x, y) = tokio::join!(
            f.domain.declare_executor_ledger(&f.owner.p, ledger(2 * round + 1)),
            other.declare_executor_ledger(&f.owner.p, ledger(2 * round + 2)),
        );
        let mut epochs = [x.unwrap().epoch, y.unwrap().epoch];
        epochs.sort();
        assert_eq!(epochs, [2 * i64::from(round) + 1, 2 * i64::from(round) + 2]);
    }
    // The same new id arriving twice at once opens one epoch.
    let (x, y) = tokio::join!(
        f.domain.declare_executor_ledger(&f.owner.p, ledger(200)),
        other.declare_executor_ledger(&f.owner.p, ledger(200)),
    );
    assert_eq!((x.unwrap().epoch, y.unwrap().epoch), (13, 13));
    assert_eq!(
        ledgers(&f, &f.owner).await.split(',').map(|row| row.split('/').next().unwrap().to_owned()).collect::<Vec<_>>(),
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
    assert_eq!((&reply["ledgerId"], &reply["epoch"]), (&json!(id), &json!(1)));
    assert!(reply["observedAtMs"].as_i64().unwrap() > 0);
    assert_eq!(reply.as_object().unwrap().len(), 3);
    let replay = post(&http, path, &f.owner.token, json!({"ledgerId": id}), StatusCode::OK).await;
    assert_eq!((&replay["ledgerId"], &replay["epoch"]), (&json!(id), &json!(1)));
    let next = post(&http, path, &f.owner.token, json!({"ledgerId": "cd".repeat(32)}), StatusCode::OK).await;
    assert_eq!(next["epoch"], 2);
    let retired = post(&http, path, &f.owner.token, json!({"ledgerId": id}), StatusCode::CONFLICT).await;
    assert_eq!(retired["code"], "agent_executor_ledger_retired");
    let malformed = post(&http, path, &f.owner.token, json!({"ledgerId": "AB".repeat(32)}), StatusCode::BAD_REQUEST).await;
    assert_eq!(malformed["code"], "invalid_agent_executor_ledger");
    for body in [json!({}), json!({"ledgerId": id, "epoch": 7}), json!({"ledger_id": id}), json!({"ledgerId": 1}), json!([])] {
        let refused = post(&http, path, &f.owner.token, body, StatusCode::BAD_REQUEST).await;
        assert_eq!(refused["code"], "invalid_management_payload");
    }
    let (status, refused) = call(&http, &format!("{path}?ledgerId={id}"), Some(&f.owner.token), json!({"ledgerId": id})).await;
    assert_eq!((status, &refused["code"]), (StatusCode::BAD_REQUEST, &json!("invalid_management_payload")));
    // Only an enrolled device declares: not a signed-in session, not nobody.
    let (status, refused) = call(&http, path, None, json!({"ledgerId": id})).await;
    assert_eq!((status, &refused["code"]), (StatusCode::UNAUTHORIZED, &json!("authentication_required")));
    let (status, refused) = call(&http, path, Some(&f.owner.session), json!({"ledgerId": id})).await;
    assert_eq!((status, &refused["code"]), (StatusCode::UNAUTHORIZED, &json!("authorization_expired")));
    let (status, refused) = call(&http, path, Some("short"), json!({"ledgerId": id})).await;
    assert_eq!((status, &refused["code"]), (StatusCode::UNAUTHORIZED, &json!("authentication_required")));
    let (status, refused) = call(&service(&f, false), path, Some(&f.owner.token), json!({"ledgerId": id})).await;
    assert_eq!((status, &refused["code"]), (StatusCode::SERVICE_UNAVAILABLE, &json!("management_not_configured")));
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
        code(f.domain.publish_management_grant(&f.owner.p, &f.policy, &b.contribution.id, publish.clone()).await),
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
    assert_eq!(code(report(&f, &another, 12).await), "agent_executor_ledger_revision_changed");
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
    let next = attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 100)).await.unwrap();
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/100");
    // The new ledger still refuses a number twice, in both arrival orders.
    let late = approved(&f, &a, "late", 2).await;
    report(&f, &late, 150).await.unwrap();
    assert_eq!(
        code(attempt(&f, &a, &next, successor(&next, "amendment", 700_000, 150)).await),
        "agent_executor_ledger_revision_changed"
    );
    attempt(&f, &a, &next, successor(&next, "amendment", 700_000, 160)).await.unwrap();
    let last = approved(&f, &a, "last", 3).await;
    assert_eq!(code(report(&f, &last, 160).await), "agent_executor_ledger_revision_changed");
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
        assert_eq!(code(report(&f, &agent, number).await), "agent_executor_ledger_revision_changed");
    }
    assert_eq!(
        code(attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 10)).await),
        "invalid_grant_succession"
    );
    report(&f, &agent, 11).await.unwrap();
    attempt(&f, &a, &root, successor(&root, "amendment", 800_000, 12)).await.unwrap();
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "1/11");
    assert_eq!(stored(&f, GRANTS).await, "0/10,1/12");
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
}
/// Deployment version, how many epoch columns exist, whether the ledger table
/// exists, and whether the command guard knows the epoch column.
async fn ledger_schema(db: &mut AsyncPgConnection) -> (i64, i64, bool, bool) {
    let s: LedgerSchema = sql_query("SELECT version::bigint AS version,(SELECT count(*) FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND column_name IN ('ledger_epoch','receipt_ledger_epoch'))::bigint AS columns,to_regclass('hagency_agent_v1.management_executor_ledgers') IS NOT NULL AS declared,position('receipt_ledger_epoch' in pg_get_functiondef('hagency_agent_v1.retain_agent_executor()'::regprocedure))>0 AS exempt FROM hagency_agent_v1.management_deployment")
        .get_result(db)
        .await
        .unwrap();
    (s.version, s.columns, s.declared, s.exempt)
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
    assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true));
    // Back to schema 34 while nothing is declared: no epoch other than 0 is lost.
    db.batch_execute(include_str!("management_executor_ledgers_down.sql")).await.unwrap();
    assert_eq!(ledger_schema(&mut db).await, (34, 0, false, false));
    // Opening the store migrates the populated database, and is repeatable.
    for _ in 0..2 {
        crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
        assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true));
    }
    let f = Fixture {
        domain: crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap(),
        ..f
    };
    f.domain.set_pool_grant_succession(true);
    // Every receipt recorded before the upgrade is in epoch 0 and behaves as
    // it did: exact replay is acknowledged, its number is still taken.
    assert_eq!(stored(&f, AGENT_RECEIPTS).await, "0/100");
    assert_eq!(stored(&f, GRANTS).await, "0/10");
    assert_eq!(report(&f, &kept, 100).await.unwrap().receipt_digest, recorded.receipt_digest);
    assert_eq!(code(report(&f, &waiting, 100).await), "agent_executor_ledger_revision_changed");
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
        assert!(refused.contains("immutable_agent_executor_command"), "{filter}: {refused}");
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
    assert!(refused.contains("executor_ledger_downgrade_has_epoch"), "{refused}");
    assert_eq!(ledger_schema(&mut db).await, (35, 9, true, true));
}
