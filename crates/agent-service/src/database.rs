//! Bounded business transactions and the membership controller leadership session.
//! Ordinary business work uses the bounded Horme pool runtime instead.
use crate::{Error, Result};
use diesel_async::{
    AsyncConnection, AsyncPgConnection, SimpleAsyncConnection,
    pooled_connection::PoolableConnection,
};
/// Business transactions never queue on this lock inside PostgreSQL.
pub(crate) async fn try_domain_writer(db: &mut AsyncPgConnection) -> diesel::QueryResult<bool> {
    use diesel_async::RunQueryDsl;
    #[derive(diesel::QueryableByName)]
    struct Lock {
        #[diesel(sql_type=diesel::sql_types::Bool)]
        acquired: bool,
    }
    Ok(
        diesel::sql_query("SELECT pg_try_advisory_xact_lock(5210750088328904) AS acquired")
            .get_result::<Lock>(db)
            .await?
            .acquired,
    )
}

/// Keep authorization helpers safe even if a caller omits writer admission.
pub(crate) async fn require_domain_writer(db: &mut AsyncPgConnection) -> Result<()> {
    if try_domain_writer(db).await? {
        Ok(())
    } else {
        Err(Error::Unavailable("domain_writer_busy"))
    }
}
/// Authorization writes may race a remote key operation. The trigger has already
/// aborted the entire DB transaction before we retry. Sleep outside the pool.
pub(crate) async fn retry_crypto_write<T, F: Future<Output = Result<T>> + Send>(
    mut operation: impl FnMut() -> F + Send,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(16);
    loop {
        match operation().await {
            Err(Error::Unavailable("agent_crypto_in_progress"))
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
            }
            outcome => return outcome,
        }
    }
}
/// Pool handles never retain a checked-out connection.
#[derive(Clone)]
pub(crate) struct Database {
    runtime: hagency_db_runtime::Database,
    pool_chain_lifetime: Arc<AtomicBool>,
    shared_demand_reoffers: Arc<AtomicBool>,
    writer: Arc<Mutex<()>>,
    #[cfg(test)]
    test_clock: Arc<std::sync::atomic::AtomicI64>,
    #[cfg(test)]
    url: String,
}
impl Database {
    pub(crate) fn business(url: &str) -> diesel::QueryResult<Self> {
        let size = match std::env::var("HAGENCY_AGENT_DB_POOL_SIZE") {
            Ok(value) => value.parse::<usize>().map_err(|_| {
                diesel::result::Error::QueryBuilderError(Box::new(std::io::Error::other(
                    "HAGENCY_AGENT_DB_POOL_SIZE must be a positive integer",
                )))
            })?,
            Err(std::env::VarError::NotPresent) => 8,
            Err(error) => return Err(diesel::result::Error::QueryBuilderError(Box::new(error))),
        };
        Self::connect(url, size)
    }

    pub(crate) fn connect(url: &str, size: usize) -> diesel::QueryResult<Self> {
        Ok(Self {
            runtime: hagency_db_runtime::Database::connect(url, size)?,
            writer: Default::default(),
            pool_chain_lifetime: Arc::new(AtomicBool::new(false)),
            shared_demand_reoffers: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            test_clock: Default::default(),
            #[cfg(test)]
            url: url.into(),
        })
    }
    /// A single bounded Horme checkout covers BEGIN, the policy settings and commit/rollback.
    pub(crate) fn transaction<'a, 'conn, T, F>(
        &'conn self,
        callback: F,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<T>> + Send + 'conn>>
    where
        for<'r> F: horme::TransactionCallback<&'r mut AsyncPgConnection, Result<T>> + Send + 'a,
        T: Send + 'a,
        'a: 'conn,
    {
        // Lines about what a transaction did are written once it has
        // committed, and never for one that rolled back.
        Box::pin(AFTER_COMMIT.scope(RefCell::new(Vec::new()), async move {
            let outcome = self
                .runtime
                .with_connection(async move |db| {
                    let result =
                        horme::transaction(db, async move |conn: &mut AsyncPgConnection| {
                            let on = |flag: &AtomicBool| {
                                if flag.load(Ordering::Acquire) {
                                    "on"
                                } else {
                                    "off"
                                }
                            };
                            conn.batch_execute(&format!(
                                "SET LOCAL hagency.pool_chain_lifetime = '{}';\
                                 SET LOCAL hagency.shared_demand_reoffers = '{}'",
                                on(&self.pool_chain_lifetime),
                                on(&self.shared_demand_reoffers)
                            ))
                            .await?;
                            #[cfg(test)]
                            if let now @ 1.. = self.test_clock.load(Ordering::Acquire) {
                                conn.batch_execute(&format!(
                                    "SET LOCAL hagency.test_clock_ms = '{now}'"
                                ))
                                .await?;
                            }
                            let outcome = callback(conn).await?;
                            // What the database gave back to the queue, or
                            // could not, it notes only while the re-offer is
                            // on (migration 2026-11-28-000161).
                            if self.shared_demand_reoffers.load(Ordering::Acquire) {
                                for note in database_notes(conn).await? {
                                    after_commit(note);
                                }
                            }
                            Ok(outcome)
                        })
                        .await;
                    // A completed rollback drained the transaction. Business rejections can reuse
                    // the session after runtime reset. Database failures retain discard semantics.
                    match result {
                        Err(Error::Database(error)) => Err(error),
                        outcome => Ok(outcome),
                    }
                })
                .await?;
            if outcome.is_ok() {
                let _ = AFTER_COMMIT
                    .try_with(|notes| notes.borrow_mut().drain(..).for_each(|note| note.emit()));
            }
            outcome
        }))
    }
    /// Wait for the domain writer without occupying a PostgreSQL connection.
    /// Only the successful attempt consumes the callback, so writes are never replayed.
    pub(crate) fn write_transaction<'a, 'conn, T, F>(
        &'conn self,
        callback: F,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<T>> + Send + 'conn>>
    where
        for<'r> F: horme::TransactionCallback<&'r mut AsyncPgConnection, Result<T>> + Send + 'a,
        T: Send + 'a,
        'a: 'conn,
    {
        self.write_transaction_with_wait(std::time::Duration::from_secs(5), callback)
    }

    pub(crate) fn write_transaction_with_wait<'a, 'conn, T, F>(
        &'conn self,
        wait: std::time::Duration,
        callback: F,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<T>> + Send + 'conn>>
    where
        for<'r> F: horme::TransactionCallback<&'r mut AsyncPgConnection, Result<T>> + Send + 'a,
        T: Send + 'a,
        'a: 'conn,
    {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + wait;
            let _writer = tokio::time::timeout_at(deadline, self.writer.lock())
                .await
                .map_err(|_| Error::Unavailable("domain_writer_busy"))?;
            let mut callback = Some(callback);
            let mut delay = std::time::Duration::from_millis(5);
            loop {
                let result = self
                    .transaction(async |conn| {
                        if try_domain_writer(conn).await? {
                            let run = callback.take().expect("writer callback runs once");
                            return run(conn).await.map(Some);
                        }
                        Ok(None)
                    })
                    .await?;
                if let Some(outcome) = result {
                    return Ok(outcome);
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Unavailable("domain_writer_busy"));
                }
                tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + delay)).await;
                delay = (delay * 2).min(std::time::Duration::from_millis(100));
            }
        })
    }
    #[cfg(test)]
    pub(crate) fn set_test_clock(&self, now: i64) {
        self.test_clock.store(now, Ordering::Release);
    }
    pub(crate) fn set_pool_chain_lifetime(&self, enabled: bool) {
        self.pool_chain_lifetime.store(enabled, Ordering::Release);
    }
    pub(crate) fn set_shared_demand_reoffers(&self, enabled: bool) {
        self.shared_demand_reoffers
            .store(enabled, Ordering::Release);
    }
    /// Independent fixture/observer session; never used by production operations.
    #[cfg(test)]
    pub(crate) async fn test_connection(&self) -> Result<TestConnection> {
        Ok(TestConnection(
            AsyncPgConnection::establish(&self.url)
                .await
                .map_err(|_| Error::Unavailable("test_database_unavailable"))?,
        ))
    }
}
impl Deref for Database {
    type Target = hagency_db_runtime::Database;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}
#[cfg(test)]
pub(crate) struct TestConnection(AsyncPgConnection);
#[cfg(test)]
impl Deref for TestConnection {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
#[cfg(test)]
impl DerefMut for TestConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, MutexGuard};

/// What a write did to a message that was given back to the queue at its
/// process's fence (time-limit rule 3), for the service log: ids and reasons,
/// never content. The database notes `released`, `release_failed` and
/// `release_search_failed` (migration 2026-11-28-000161); the service notes
/// `ended`, with the block's reason.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ReofferNote {
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) demand: Option<String>,
    #[serde(default)]
    pub(crate) work: Option<String>,
    #[serde(default)]
    pub(crate) runtime: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}
/// What the tests read of the service log.
#[cfg(test)]
pub(crate) static EMITTED: std::sync::Mutex<Vec<ReofferNote>> = std::sync::Mutex::new(Vec::new());
/// The notes written for `demand`, oldest first.
#[cfg(test)]
pub(crate) fn emitted(demand: &str) -> Vec<ReofferNote> {
    EMITTED
        .lock()
        .unwrap()
        .iter()
        .filter(|n| n.demand.as_deref() == Some(demand))
        .cloned()
        .collect()
}
impl ReofferNote {
    fn emit(&self) {
        #[cfg(test)]
        EMITTED.lock().unwrap().push(self.clone());
        let field = |v: &Option<String>| v.clone().unwrap_or_default();
        let (demand, work, runtime, reason) = (
            field(&self.demand),
            field(&self.work),
            field(&self.runtime),
            field(&self.reason),
        );
        match self.kind.as_str() {
            "released" => tracing::info!(
                demand = %demand, work = %work, runtime = %runtime,
                "message given back to the queue at its process's fence"
            ),
            "release_failed" | "release_search_failed" => tracing::warn!(
                kind = %self.kind, demand = %demand, work = %work, runtime = %runtime, sqlstate = %reason,
                "message could not be given back to the queue; it ends as before"
            ),
            _ => tracing::info!(
                kind = %self.kind, demand = %demand, reason = %reason,
                "message that was given back to the queue ended"
            ),
        }
    }
}
tokio::task_local! {
    static AFTER_COMMIT: RefCell<Vec<ReofferNote>>;
}
/// Write `note` to the service log once the transaction it belongs to has
/// committed, and never if it rolls back. Outside a transaction (a statement
/// that commits by itself) it is written now.
pub(crate) fn after_commit(note: ReofferNote) {
    let mut note = Some(note);
    let _ = AFTER_COMMIT.try_with(|notes| notes.borrow_mut().extend(note.take()));
    if let Some(note) = note {
        note.emit();
    }
}
/// The database's notes of this transaction (`hagency.shared_reoffer_notes`).
async fn database_notes(conn: &mut AsyncPgConnection) -> Result<Vec<ReofferNote>> {
    use diesel_async::RunQueryDsl;
    #[derive(diesel::QueryableByName)]
    struct Notes {
        #[diesel(sql_type = diesel::sql_types::Text)]
        value: String,
    }
    let notes = diesel::sql_query(
        "SELECT coalesce(current_setting('hagency.shared_reoffer_notes', true), '') AS value",
    )
    .get_result::<Notes>(conn)
    .await?;
    Ok(serde_json::from_str(&notes.value).unwrap_or_default())
}

/// Session setting read by SQL and by pool Agent lifetime checks: a reserved
/// pool Agent lives while its pool's capacity chain continues.
pub(crate) const POOL_CHAIN_LIFETIME: &str = "hagency.pool_chain_lifetime";
pub(crate) struct DedicatedDatabase {
    connection: Mutex<Option<AsyncPgConnection>>,
    url: String,
}
impl DedicatedDatabase {
    pub(crate) fn new(connection: AsyncPgConnection, url: &str) -> Self {
        Self {
            connection: Mutex::new(Some(connection)),
            url: url.into(),
        }
    }
    pub(crate) async fn lock(&self) -> Result<Guard<'_>> {
        let mut connection =
            tokio::time::timeout(std::time::Duration::from_secs(8), self.connection.lock())
                .await
                .map_err(|_| Error::Unavailable("dedicated_database_busy"))?;
        discard_broken_connection(&mut connection);
        if connection.is_none() {
            let started = std::time::Instant::now();
            let db = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                AsyncPgConnection::establish(&self.url),
            )
            .await
            .map_err(|_| Error::Unavailable("database_reconnect_timeout"))?
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
            *connection = Some(db);
            tracing::info!(
                stage = "database_reconnected",
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Replaced unusable database connection"
            );
        }
        Ok(Guard { connection })
    }
}
pub(crate) struct Guard<'a> {
    connection: MutexGuard<'a, Option<AsyncPgConnection>>,
}
impl Deref for Guard<'_> {
    type Target = AsyncPgConnection;
    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect("connected guard")
    }
}
impl DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect("connected guard")
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        discard_broken_connection(&mut self.connection);
    }
}

fn discard_broken_connection(connection: &mut Option<AsyncPgConnection>) {
    if connection
        .as_mut()
        .is_some_and(PoolableConnection::is_broken)
    {
        // Dropping the PostgreSQL connection rolls back its outer transaction,
        // including locks and uncertain writes. Also replace connections closed
        // while idle, e.g. after a database restart.
        connection.take();
        tracing::warn!(
            stage = "database_connection_discarded",
            "Discarded closed connection or unfinished transaction"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn blocked_writers_leave_single_pool_slot_available_and_execute_once() {
        use diesel_async::RunQueryDsl;
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let db = Database::connect(&url, 1).unwrap();
        let mut held = AsyncPgConnection::establish(&url).await.unwrap();
        held.batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
            .await
            .unwrap();
        let mut writers = tokio::task::JoinSet::new();
        let executed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..8 {
            let pool = db.clone();
            let executed = executed.clone();
            writers.spawn(async move {
                pool.write_transaction(async |_conn| {
                    executed.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .await
            });
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while db.checkout_count() < 2 {
                tokio::task::yield_now().await;
            }
            assert!(matches!(
                db.transaction(async |conn| require_domain_writer(conn).await)
                    .await,
                Err(Error::Unavailable("domain_writer_busy"))
            ));
            db.transaction(async |conn| Ok(diesel::sql_query("SELECT 1").execute(conn).await?))
                .await
                .unwrap();
        })
        .await
        .expect("a reader can use the only pool slot while the domain writer is blocked");
        assert_eq!(executed.as_ref().load(Ordering::Relaxed), 0);
        held.batch_execute("COMMIT").await.unwrap();
        while let Some(result) = writers.join_next().await {
            result.unwrap().unwrap();
        }
        assert_eq!(executed.as_ref().load(Ordering::Relaxed), 8);
        #[derive(diesel::QueryableByName)]
        struct Pid {
            #[diesel(sql_type=diesel::sql_types::Integer)]
            pid: i32,
        }
        let pid = async || {
            db.transaction(async |conn| {
                Ok(diesel::sql_query("SELECT pg_backend_pid() AS pid")
                    .get_result::<Pid>(conn)
                    .await?
                    .pid)
            })
            .await
            .unwrap()
        };
        let before = pid().await;
        assert!(matches!(
            db.transaction(async |_| Err::<(), _>(Error::Conflict("expected")))
                .await,
            Err(Error::Conflict("expected"))
        ));
        assert_eq!(
            before,
            pid().await,
            "expected business rollbacks reuse the session"
        );
    }

    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn managed_session_applies_policy_changes_across_database_clones() {
        use diesel_async::RunQueryDsl;

        #[derive(diesel::QueryableByName)]
        struct Setting {
            #[diesel(sql_type = diesel::sql_types::Text)]
            value: String,
        }

        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let db = Database::connect(&url, 1).unwrap();
        let clone = db.clone();
        for (enabled, expected) in [(true, "on"), (false, "off"), (true, "on")] {
            db.set_pool_chain_lifetime(enabled);
            // The re-offer has its own switch, set the other way here.
            db.set_shared_demand_reoffers(!enabled);
            let current = clone
                .transaction(async |conn| {
                    Ok(diesel::sql_query(
                        "SELECT current_setting('hagency.pool_chain_lifetime', true)||'/'||\
                         current_setting('hagency.shared_demand_reoffers', true) AS value",
                    )
                    .get_result::<Setting>(conn)
                    .await?)
                })
                .await
                .unwrap();
            let other = if enabled { "off" } else { "on" };
            assert_eq!(current.value, format!("{expected}/{other}"));
        }
    }

    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn reoffer_notes_are_written_once_their_transaction_commits() {
        use diesel_async::RunQueryDsl;
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let db = Database::connect(&url, 1).unwrap();
        for (on, commits) in [(true, true), (true, false), (false, true)] {
            let demand = format!("sdm_note_{on}_{commits}");
            db.set_shared_demand_reoffers(on);
            // A note as the database writes it, and one of the service's own.
            let note = format!(
                "SELECT set_config('hagency.shared_reoffer_notes',\
                 '[{{\"kind\":\"released\",\"demand\":\"{demand}\",\"work\":\"w\",\"runtime\":\"r\"}}]',true)"
            );
            let outcome = db
                .transaction(async |conn| {
                    diesel::sql_query(&note).execute(conn).await?;
                    after_commit(ReofferNote {
                        kind: "ended".into(),
                        demand: Some(demand.clone()),
                        reason: Some("retry_lapsed".into()),
                        ..Default::default()
                    });
                    assert!(emitted(&demand).is_empty(), "nothing before the commit");
                    if commits {
                        Ok(())
                    } else {
                        Err(Error::Conflict("rolled_back"))
                    }
                })
                .await;
            assert_eq!(outcome.is_ok(), commits);
            let kinds: Vec<String> = emitted(&demand).into_iter().map(|n| n.kind).collect();
            // The database's notes are read only while the re-offer is on.
            let expected: &[&str] = match (on, commits) {
                (_, false) => &[],
                (true, true) => &["ended", "released"],
                (false, true) => &["ended"],
            };
            assert_eq!(kinds, expected, "on={on} commits={commits}");
        }
        // The setting lives as long as its transaction.
        db.set_shared_demand_reoffers(true);
        let next = db
            .transaction(async |conn| Ok(database_notes(conn).await?))
            .await
            .unwrap();
        assert!(next.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn terminated_idle_connection_is_replaced() {
        use diesel_async::RunQueryDsl;
        #[derive(diesel::QueryableByName)]
        struct Backend {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            pid: i32,
        }
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let db = DedicatedDatabase::new(AsyncPgConnection::establish(&url).await.unwrap(), &url);
        let pid = diesel::sql_query("SELECT pg_backend_pid() AS pid")
            .get_result::<Backend>(&mut *db.lock().await.unwrap())
            .await
            .unwrap()
            .pid;
        let mut observer = AsyncPgConnection::establish(&url).await.unwrap();
        observer
            .batch_execute(&format!("SELECT pg_terminate_backend({pid})"))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let result = diesel::sql_query("SELECT pg_backend_pid() AS pid")
                    .get_result::<Backend>(&mut *db.lock().await.unwrap())
                    .await;
                if let Ok(replacement) = result {
                    assert_ne!(replacement.pid, pid);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn cancelled_transaction_releases_locks_and_does_not_commit_on_reuse() {
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let db = DedicatedDatabase::new(AsyncPgConnection::establish(&url).await.unwrap(), &url);
        db.lock().await.unwrap().batch_execute(
            "CREATE TABLE IF NOT EXISTS hagency_cancel_test(value integer); DELETE FROM hagency_cancel_test").await.unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let mut operation = Box::pin(async {
            let mut guard = db.lock().await.unwrap();
            (*guard).transaction::<_,diesel::result::Error,_>(async |conn: &mut AsyncPgConnection| {
                conn.batch_execute("SELECT pg_advisory_xact_lock(5210750088328999); INSERT INTO hagency_cancel_test VALUES(1)").await?;
                ready_tx.send(()).unwrap();
                std::future::pending::<()>().await;
                Ok(())
            }).await.unwrap();
        });
        tokio::select! { _=&mut operation=>panic!("must remain pending"), _=ready_rx=>() }
        // Drop the future itself, not merely a Pin<&mut Future>.
        drop(operation);
        let mut observer = AsyncPgConnection::establish(&url).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(4),
            observer
                .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328999); ROLLBACK"),
        )
        .await
        .unwrap()
        .unwrap();
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type=diesel::sql_types::BigInt)]
            total: i64,
        }
        use diesel_async::RunQueryDsl;
        let mut guard = db.lock().await.unwrap();
        (*guard)
            .transaction::<_, diesel::result::Error, _>(async |conn: &mut AsyncPgConnection| {
                conn.batch_execute("INSERT INTO hagency_cancel_test VALUES(2)")
                    .await?;
                Ok(())
            })
            .await
            .unwrap();
        let count =
            diesel::sql_query("SELECT count(*) AS total FROM hagency_cancel_test WHERE value=1")
                .get_result::<Count>(&mut *guard)
                .await
                .unwrap();
        assert_eq!(count.total, 0, "cancelled writes must be rolled back");
        let count =
            diesel::sql_query("SELECT count(*) AS total FROM hagency_cancel_test WHERE value=2")
                .get_result::<Count>(&mut observer)
                .await
                .unwrap();
        assert_eq!(count.total, 1, "replacement transaction must really commit");
        guard
            .batch_execute("DROP TABLE hagency_cancel_test")
            .await
            .unwrap();
    }
}
