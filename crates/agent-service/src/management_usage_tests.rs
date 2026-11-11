use super::*;
use diesel_async::{AsyncConnection, SimpleAsyncConnection};

/// Only the aggregate's relational boundary is under test here. Full native
/// receipts and authenticated APIs are covered by the pooled-daemon fixture.
/// Run in an explicitly supplied empty disposable database; all schema/data
/// below remains in one transaction, including on a panic/disconnect.
#[tokio::test]
#[ignore = "requires an empty disposable PostgreSQL database via HAGENCY_USAGE_TEST_DATABASE_URL"]
async fn pool_agent_usage_without_project_preserves_scopes_and_high_water() {
    let url = std::env::var("HAGENCY_USAGE_TEST_DATABASE_URL").unwrap();
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    db.batch_execute(r#"
        BEGIN;
        CREATE SCHEMA hagency_agent_v1;
        CREATE TABLE hagency_agent_v1.management_allocations(id text PRIMARY KEY,grant_id text);
        CREATE TABLE hagency_agent_v1.management_agent_requests(id text PRIMARY KEY,allocation_id text);
        CREATE TABLE hagency_agent_v1.management_shared_assignments(id text PRIMARY KEY,request_id text);
        CREATE TABLE hagency_agent_v1.management_shared_runtimes(id text PRIMARY KEY,assignment_id text);
        CREATE TABLE hagency_agent_v1.management_shared_work(id text PRIMARY KEY,runtime_id text,known_consumed_tokens bigint);
        CREATE TABLE hagency_agent_v1.management_agent_settlement_commands(id text PRIMARY KEY,request_id text);
        CREATE TABLE hagency_agent_v1.management_agent_settlement_results(command_id text,payload text);
        CREATE TABLE hagency_agent_v1.management_agent_intents(id text PRIMARY KEY,request_id text);
        CREATE TABLE hagency_agent_v1.management_agent_executor_commands(intent_id text,operation text,receipt text);
        INSERT INTO hagency_agent_v1.management_allocations VALUES('allocation','grant');
        INSERT INTO hagency_agent_v1.management_agent_requests VALUES('pool',NULL),('project','allocation'),('unrelated',NULL);
        INSERT INTO hagency_agent_v1.management_shared_assignments VALUES('pool_assignment','pool'),('project_assignment','project'),('other_assignment','unrelated');
        INSERT INTO hagency_agent_v1.management_shared_runtimes VALUES('original','pool_assignment'),('successor','pool_assignment'),('project_runtime','project_assignment'),('other_runtime','other_assignment');
        INSERT INTO hagency_agent_v1.management_shared_work VALUES('first','original',5),('second','successor',5),('project_work','project_runtime',7),('other_work','other_runtime',999);
        INSERT INTO hagency_agent_v1.management_agent_settlement_commands VALUES('settlement','pool');
        INSERT INTO hagency_agent_v1.management_agent_settlement_results VALUES('settlement','{"knownConsumedTokens":8}');
        INSERT INTO hagency_agent_v1.management_agent_intents VALUES('intent','pool');
        INSERT INTO hagency_agent_v1.management_agent_executor_commands VALUES('intent','agent.fence','{"knownConsumedTokens":9}');
    "#).await.unwrap();

    assert_eq!(known(&mut db, Scope::Agent("pool")).await.unwrap(), 10);
    assert_eq!(known(&mut db, Scope::Agent("project")).await.unwrap(), 7);
    assert_eq!(known(&mut db, Scope::Agent("missing")).await.unwrap(), 0);
    assert_eq!(
        known(&mut db, Scope::Allocation("allocation"))
            .await
            .unwrap(),
        7
    );
    assert_eq!(known(&mut db, Scope::Grant("grant")).await.unwrap(), 7);
    assert_eq!(known(&mut db, Scope::Grant("missing")).await.unwrap(), 0);

    db.batch_execute("UPDATE hagency_agent_v1.management_agent_settlement_results SET payload='{\"knownConsumedTokens\":12}'").await.unwrap();
    assert_eq!(
        known(&mut db, Scope::Agent("pool")).await.unwrap(),
        12,
        "settlement is a high-water floor, not usage to add to the same work"
    );
    db.batch_execute("UPDATE hagency_agent_v1.management_agent_executor_commands SET receipt='{\"knownConsumedTokens\":13}'").await.unwrap();
    assert_eq!(known(&mut db, Scope::Agent("pool")).await.unwrap(), 13);
    db.batch_execute(&format!("UPDATE hagency_agent_v1.management_shared_work SET known_consumed_tokens={MAX_EXACT} WHERE id='first'")).await.unwrap();
    assert!(matches!(
        known(&mut db, Scope::Agent("pool")).await,
        Err(Error::Unavailable("shared_usage_overflow"))
    ));
    db.batch_execute("ROLLBACK").await.unwrap();
}
