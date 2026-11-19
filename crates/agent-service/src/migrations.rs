//! Embedded, forward-only PostgreSQL migrations. The five entry points may open
//! independently, so each owns a disjoint set of Diesel migration versions.
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Bool};
use diesel_async::AsyncPgConnection;
use diesel_async::async_connection_wrapper::AsyncConnectionWrapper;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

use crate::{Error, Result};

const IDENTITY: EmbeddedMigrations = embed_migrations!("migrations/identity");
const DOMAIN: EmbeddedMigrations = embed_migrations!("migrations/domain");
const BOARD: EmbeddedMigrations = embed_migrations!("migrations/board");
const INBOX: EmbeddedMigrations = embed_migrations!("migrations/inbox");
const TRANSPORT: EmbeddedMigrations = embed_migrations!("migrations/transport");

const SCHEMA_LOCK: &str = "SELECT pg_advisory_xact_lock(5210750088328902)";
const BOARD_LOCK: &str = "SET LOCAL lock_timeout='30s';
    SET LOCAL statement_timeout='120s';
    SELECT pg_advisory_xact_lock(5210750088328902);
    SELECT pg_advisory_xact_lock(5210750088328915)";

type ConnectionWrapper = AsyncConnectionWrapper<AsyncPgConnection>;

fn apply(db: &mut ConnectionWrapper, migrations: EmbeddedMigrations) -> Result<()> {
    db.run_pending_migrations(migrations)
        .map_err(|e| Error::Database(diesel::result::Error::QueryBuilderError(e)))?;
    Ok(())
}

/// Serialize even the creation of Diesel's ledger. Identity checks, DDL,
/// backfills and migration receipts commit or roll back together.
async fn run(
    conn: AsyncPgConnection,
    lock_sql: &'static str,
    upgrade: impl FnOnce(&mut ConnectionWrapper) -> Result<()> + Send + 'static,
) -> Result<AsyncPgConnection> {
    tokio::task::spawn_blocking(move || {
        let mut db = ConnectionWrapper::from(conn);
        db.transaction(|db| {
            db.batch_execute(lock_sql)?;
            upgrade(db)
        })?;
        Ok(db.into_inner())
    })
    .await
    .map_err(|_| Error::Unavailable("database_migration_task_failed"))?
}

#[derive(Queryable, QueryableByName)]
struct Flag {
    #[diesel(sql_type=Bool)]
    matched: bool,
}

fn matches_schema(db: &mut ConnectionWrapper, query: &str) -> Result<bool> {
    Ok(sql_query(query).get_result::<Flag>(db)?.matched)
}

pub(crate) async fn identity(
    db: AsyncPgConnection,
    server: &str,
    issuer: &str,
) -> Result<AsyncPgConnection> {
    use crate::schema::agent_tables::deployment as q;
    let server = server.to_owned();
    let issuer = issuer.to_owned();
    run(db, SCHEMA_LOCK, move |db| {
        apply(db, IDENTITY)?;
        diesel::insert_into(q::table)
            .values((
                q::singleton.eq(true),
                q::version.eq(1),
                q::server_name.eq(&server),
                q::issuer.eq(&issuer),
            ))
            .on_conflict_do_nothing()
            .execute(db)?;
        let compatible = q::table
            .filter(q::singleton)
            .select(
                q::version
                    .eq(1)
                    .and(q::server_name.eq(&server))
                    .and(q::issuer.eq(&issuer)),
            )
            .get_result::<bool>(db)?;
        if !compatible {
            return Err(Error::Conflict("deployment_identity_mismatch"));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn domain(
    db: AsyncPgConnection,
    server: &str,
    namespace: &str,
) -> Result<AsyncPgConnection> {
    let expected_server = server.to_owned();
    let expected_namespace = namespace.to_owned();
    run(db, SCHEMA_LOCK, move |db| {
        use crate::schema::agent_tables::{deployment, domain_deployment};
        let deployment_matches = deployment::table.filter(deployment::singleton)
            .select(deployment::version.eq(1).and(deployment::server_name.eq(&expected_server)))
            .get_result::<bool>(db)?;
        if !deployment_matches {
            return Err(Error::Conflict("deployment_identity_mismatch"));
        }
        apply(db, DOMAIN)?;
        if !matches_schema(db, "SELECT EXISTS (
            SELECT 1 FROM pg_proc
            WHERE oid=to_regprocedure('hagency_agent_v1.validate_shared_demand()')
              AND position('retry-aware demand validator' in prosrc)>0
        ) AS matched")? {
            return Err(Error::Conflict("validate_shared_demand_changed_by_migration"));
        }
        if !matches_schema(db, "SELECT version=36 AS matched FROM hagency_agent_v1.management_deployment WHERE singleton")? {
            return Err(Error::Conflict("management_schema_incompatible"));
        }
        diesel::insert_into(domain_deployment::table)
            .values((domain_deployment::singleton.eq(true), domain_deployment::version.eq(4), domain_deployment::namespace.eq(&expected_namespace)))
            .on_conflict_do_nothing().execute(db)?;
        let domain_matches = domain_deployment::table.filter(domain_deployment::singleton)
            .select(domain_deployment::version.eq(4).and(domain_deployment::namespace.eq(&expected_namespace)))
            .get_result::<bool>(db)?;
        if !domain_matches || !matches_schema(db, "SELECT to_regclass('hagency_agent_v1.scope_pauses') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='agents' AND column_name='execution_device_id') AS matched")? {
            return Err(Error::Conflict("domain_schema_incompatible"));
        }
        Ok(())
    }).await
}

pub(crate) async fn transport(db: AsyncPgConnection) -> Result<AsyncPgConnection> {
    run(db, SCHEMA_LOCK, |db| {
        if !matches_schema(db, "SELECT to_regclass('hagency_agent_v1.domain_deployment') IS NOT NULL AS matched")? {
            return Err(Error::Conflict("agent_domain_not_initialized"));
        }
        if !matches_schema(db, "SELECT version=4 AS matched FROM hagency_agent_v1.domain_deployment WHERE singleton")? {
            return Err(Error::Conflict("domain_schema_incompatible"));
        }
        apply(db, TRANSPORT)?;
        for (query, error) in [
            ("SELECT id,source_digest,event,agent_generation,binding_generation,state FROM hagency_agent_v1.ciphertext_inbox LIMIT 0", "ciphertext_schema_incompatible"),
            ("SELECT dispatch_id,inbox_id,source_digest,projection,seal_digest FROM hagency_agent_v1.encrypted_dispatches LIMIT 0", "encrypted_admission_schema_incompatible"),
            ("SELECT id FROM hagency_agent_v1.file_sendable LIMIT 0", "file_delivery_schema_incompatible"),
            ("SELECT dispatch_id,descriptor,descriptor_digest FROM hagency_agent_v1.event_attachments LIMIT 0", "attachment_schema_incompatible"),
            ("SELECT id FROM hagency_agent_v1.processing_sendable LIMIT 0", "processing_schema_incompatible"),
            ("SELECT delivery_epoch FROM hagency_agent_v1.reply_outbox LIMIT 0", "transport_schema_incompatible"),
        ] {
            db.batch_execute(query).map_err(|_| Error::Conflict(error))?;
        }
        Ok(())
    }).await
}

pub(crate) async fn board(db: AsyncPgConnection) -> Result<AsyncPgConnection> {
    #[derive(QueryableByName)]
    struct Version {
        #[diesel(sql_type=BigInt)]
        value: i64,
    }
    const VERSION: &str =
        "SELECT coalesce(max(version),0)::bigint AS value FROM hagency_project_v1.migrations";
    const COMPATIBILITY: [&str; 7] = [
        "SELECT to_regclass('hagency_project_v1.boards') IS NOT NULL AND to_regclass('hagency_project_v1.tasks') IS NOT NULL AND to_regclass('hagency_project_v1.comments') IS NOT NULL AND to_regclass('hagency_project_v1.changes') IS NOT NULL AND to_regclass('hagency_project_v1.receipts') IS NOT NULL AS matched",
        "SELECT to_regclass('hagency_project_v1.columns') IS NOT NULL AS matched",
        "SELECT to_regclass('hagency_project_v1.notifications') IS NOT NULL AND to_regclass('hagency_project_v1.attachments') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='changes' AND column_name='details') AS matched",
        "SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='columns' AND column_name='archived') AS matched",
        "SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='boards' AND column_name='owner_project_id') AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='projects' AND column_name='archived') AS matched",
        "SELECT to_regclass('hagency_project_v1.navigation_defaults') IS NOT NULL AS matched",
        "SELECT (SELECT count(*) FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='attachments' AND column_name IN ('byte_size','content_sha256'))=2 AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='attachments' AND column_name='data' AND is_nullable='YES') AS matched",
    ];
    run(db, BOARD_LOCK, |db| {
        if matches_schema(
            db,
            "SELECT to_regclass('hagency_project_v1.migrations') IS NOT NULL AS matched",
        )? {
            let version = sql_query(VERSION).get_result::<Version>(db)?.value;
            if !(1..=7).contains(&version) {
                return Err(Error::Conflict("board_schema_incompatible"));
            }
            for query in &COMPATIBILITY[..version as usize] {
                if !matches_schema(db, query)? {
                    return Err(Error::Conflict("board_schema_incompatible"));
                }
            }
        }
        apply(db, BOARD)?;
        if sql_query(VERSION).get_result::<Version>(db)?.value != 7 {
            return Err(Error::Conflict("board_schema_incompatible"));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn inbox(db: AsyncPgConnection) -> Result<AsyncPgConnection> {
    run(db, SCHEMA_LOCK, |db| {
        // Older replicas serialize ingress on this key. Hold it while adopting the ledger.
        db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")?;
        apply(db, INBOX)
    })
    .await
}
