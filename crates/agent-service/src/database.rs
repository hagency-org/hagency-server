//! Bounded business transactions and the membership controller leadership session.
//! Ordinary business work uses the bounded Horme pool runtime instead.
use crate::{Error, Result};
use diesel_async::{
    AsyncConnection, AsyncPgConnection, SimpleAsyncConnection,
    pooled_connection::PoolableConnection,
};
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
            #[cfg(test)]
            test_clock: Default::default(),
            #[cfg(test)]
            url: url.into(),
        })
    }
    /// A single bounded Horme checkout covers BEGIN, the policy setting and commit/rollback.
    pub(crate) fn transaction<'a, 'conn, T, F>(
        &'conn self,
        callback: F,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<T>> + Send + 'conn>>
    where
        for<'r> F: horme::TransactionCallback<&'r mut AsyncPgConnection, Result<T>> + Send + 'a,
        T: Send + 'a,
        'a: 'conn,
    {
        Box::pin(async move {
            self.runtime
                .with_connection(async move |db| {
                    let result =
                        horme::transaction(db, async move |conn: &mut AsyncPgConnection| {
                            let setting = if self.pool_chain_lifetime.load(Ordering::Acquire) {
                                "SET LOCAL hagency.pool_chain_lifetime = 'on'"
                            } else {
                                "SET LOCAL hagency.pool_chain_lifetime = 'off'"
                            };
                            conn.batch_execute(setting).await?;
                            #[cfg(test)]
                            if let now @ 1.. = self.test_clock.load(Ordering::Acquire) {
                                conn.batch_execute(&format!(
                                    "SET LOCAL hagency.test_clock_ms = '{now}'"
                                ))
                                .await?;
                            }
                            callback(conn).await
                        })
                        .await;
                    // A completed rollback drained the transaction. Business rejections can reuse
                    // the session after runtime reset. Database failures retain discard semantics.
                    match result {
                        Err(Error::Database(error)) => Err(error),
                        outcome => Ok(outcome),
                    }
                })
                .await?
        })
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
        Box::pin(async move {
            use diesel_async::RunQueryDsl;
            #[derive(diesel::QueryableByName)]
            struct Lock {
                #[diesel(sql_type=diesel::sql_types::Bool)]
                acquired: bool,
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            let _writer = tokio::time::timeout_at(deadline, self.writer.lock())
                .await
                .map_err(|_| Error::Unavailable("domain_writer_busy"))?;
            let mut callback = Some(callback);
            let mut delay = std::time::Duration::from_millis(5);
            loop {
                let result = self
                    .transaction(async |conn| {
                        let lock = diesel::sql_query(
                            "SELECT pg_try_advisory_xact_lock(5210750088328904) AS acquired",
                        )
                        .get_result::<Lock>(conn)
                        .await?;
                        if lock.acquired {
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
                tokio::time::sleep(delay).await;
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
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, MutexGuard};

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
            let current = clone
                .transaction(async |conn| {
                    Ok(diesel::sql_query(
                        "SELECT current_setting('hagency.pool_chain_lifetime', true) AS value",
                    )
                    .get_result::<Setting>(conn)
                    .await?)
                })
                .await
                .unwrap();
            assert_eq!(current.value, expected);
        }
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
