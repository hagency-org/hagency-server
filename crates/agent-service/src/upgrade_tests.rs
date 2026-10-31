//! Upgrade populated historical schemas in a separate database, never in the fixture DB.
use crate::{domain::DomainStore, identity::Identity, store::Store};
use diesel::{Connection as _, PgConnection, QueryableByName, connection::SimpleConnection};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};

struct IsolatedDatabase {
    admin_url: String,
    url: String,
    name: String,
}
impl IsolatedDatabase {
    fn create() -> Self {
        let admin_url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let name = format!("hagency_server_upgrade_{}", crate::entity_id().unwrap());
        let mut connection = PgConnection::establish(&admin_url).unwrap();
        connection
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .unwrap();
        let mut parsed = url::Url::parse(&admin_url).unwrap();
        parsed.set_path(&name);
        Self {
            admin_url,
            url: parsed.to_string(),
            name,
        }
    }
}
impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        if let Ok(mut connection) = PgConnection::establish(&self.admin_url) {
            let _ = connection.batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", self.name));
        }
    }
}

#[derive(QueryableByName)]
struct Snapshot {
    #[diesel(sql_type = diesel::sql_types::Jsonb)]
    value: serde_json::Value,
}
async fn snapshot(connection: &mut AsyncPgConnection) -> serde_json::Value {
    diesel::sql_query("SELECT jsonb_build_object('users',(SELECT jsonb_agg(to_jsonb(u) ORDER BY u.id) FROM hagency_agent_v1.users u),'sessions',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.id) FROM hagency_agent_v1.sessions s),'projects',(SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM hagency_agent_v1.projects p),'tasks',(SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM hagency_project_v1.tasks t),'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.intent_key) FROM hagency_project_v1.receipts r)) AS value")
        .get_result::<Snapshot>(connection).await.unwrap().value
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn populated_v6_upgrade_and_reopen_preserve_identity_tasks_receipts_and_active_attachment_bytes()
 {
    let isolated = tokio::task::spawn_blocking(IsolatedDatabase::create)
        .await
        .unwrap();
    let auth = Store::open(&isolated.url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let now = crate::api::now_ms();
    let grant = auth
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: "upgrade-owner".into(),
                mxid: "@upgrade-owner:example.test".into(),
                client_id: "upgrade-test".into(),
                valid_until_ms: now + 600_000,
            },
            now,
        )
        .await
        .unwrap();
    drop(
        DomainStore::open(&isolated.url, "example.test", "_hagency_test_")
            .await
            .unwrap(),
    );
    let mut connection = AsyncPgConnection::establish(&isolated.url).await.unwrap();
    // Reconstruct exactly the historical v6 board layout inside this newly created DB.
    connection.batch_execute("DROP SCHEMA hagency_project_v1 CASCADE; ALTER TABLE hagency_agent_v1.projects DROP COLUMN archived").await.unwrap();
    for sql in [
        include_str!("project_board/schema.sql"),
        include_str!("project_board/schema_v2.sql"),
        include_str!("project_board/schema_v3.sql"),
        include_str!("project_board/schema_v4.sql"),
        include_str!("project_board/schema_v5.sql"),
        include_str!("project_board/schema_v6.sql"),
    ] {
        connection.batch_execute(sql).await.unwrap();
    }
    connection.batch_execute("INSERT INTO hagency_agent_v1.projects(id,space_id,creation_policy) VALUES('upgrade-project','!upgrade:example.test','{}');
        INSERT INTO hagency_project_v1.boards(project_id,owner_project_id) VALUES('upgrade-project','upgrade-project');
        INSERT INTO hagency_project_v1.columns(project_id,column_key,title,position,is_done) VALUES('upgrade-project','todo','To do',0,false);
        INSERT INTO hagency_project_v1.tasks(id,project_id,number,title,description,state_key,rank,priority,labels,version,created_by,created_at,updated_at) VALUES('upgrade-task','upgrade-project',1,'Keep this task','Preserve description','todo',1024,'normal','[\"legacy\"]',8,'@upgrade-owner:example.test',10,20);
        INSERT INTO hagency_project_v1.receipts(project_id,actor,intent_key,digest,result) VALUES('upgrade-project','@upgrade-owner:example.test','original-intent','original-digest','{\"task\":{\"id\":\"upgrade-task\",\"version\":8}}');
        INSERT INTO hagency_project_v1.attachments(id,project_id,task_id,filename,content_type,data,created_by,created_at,removed_at) VALUES('active-attachment','upgrade-project','upgrade-task','old.bin','application/octet-stream',decode('0001ff','hex'),'@upgrade-owner:example.test',10,NULL),('removed-attachment','upgrade-project','upgrade-task','removed.bin','application/octet-stream',decode('02ff','hex'),'@upgrade-owner:example.test',10,20)")
        .await.unwrap();
    let before = snapshot(&mut connection).await;
    let domain = DomainStore::open(&isolated.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(before, snapshot(&mut connection).await);
    assert_eq!(
        auth.authenticate(&grant.token, now, false)
            .await
            .unwrap()
            .user_id,
        grant.user_id
    );
    let attachments = diesel::sql_query("SELECT jsonb_agg(jsonb_build_object('id',id,'size',byte_size,'digest',content_sha256,'data',encode(data,'hex')) ORDER BY id) AS value FROM hagency_project_v1.attachments")
        .get_result::<Snapshot>(&mut connection).await.unwrap().value;
    assert_eq!(attachments[0]["data"], "0001ff");
    assert_eq!(attachments[0]["size"], 3);
    assert_eq!(attachments[1]["size"], 2);
    assert!(
        attachments[1]["data"].is_null(),
        "v7 intentionally releases removed blobs while retaining tombstones"
    );
    use sha2::{Digest, Sha256};
    assert_eq!(
        attachments[0]["digest"],
        hex::encode(Sha256::digest([0_u8, 1, 255]))
    );
    assert_eq!(
        attachments[1]["digest"],
        hex::encode(Sha256::digest([2_u8, 255]))
    );
    drop(domain);
    let reopened = DomainStore::open(&isolated.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        before,
        snapshot(&mut connection).await,
        "repeated startup must be idempotent"
    );
    drop(reopened);
}
