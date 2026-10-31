//! Dedicated infrastructure sessions for controller leadership and crypto row fences.
//! Ordinary business work uses the bounded Horme pool runtime instead.
use crate::{Error, Result};
use diesel_async::{AsyncConnection, AsyncPgConnection, pooled_connection::PoolableConnection};
/// Pool handles never retain a checked-out connection.
#[derive(Clone)]
pub(crate) struct Database {
    runtime: hagency_db_runtime::Database,
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
            #[cfg(test)]
            url: url.into(),
        })
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
use tokio::sync::{Mutex, MutexGuard};

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
    use diesel_async::SimpleAsyncConnection;

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
