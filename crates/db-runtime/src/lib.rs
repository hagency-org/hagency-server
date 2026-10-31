//! Pool configuration, timeout scopes and cancellation cleanup for Horme queries.
#![forbid(unsafe_code)]
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use diesel::result::{Error, QueryResult};
use diesel_async::pooled_connection::deadpool::{Hook, HookError, Object, Pool};
use diesel_async::pooled_connection::{AsyncDieselConnectionManager, PoolError};
use diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use horme::GetConnection;
use tokio::sync::Semaphore;

tokio::task_local! {
    // Scope follows the polled future, including nested join!/select! branches.
    // Spawned tasks have independent scopes and remain bounded by admission/timeouts.
    static ACTIVE_DATABASES: Vec<usize>;
}

#[derive(Clone)]
pub struct Database(Arc<Inner>);

struct Inner {
    // Horme owns only the pool handle. Raw provider access never leaves this crate.
    pool: horme::Db<Pool<AsyncPgConnection>>,
    admission: Arc<Semaphore>,
    checkout_timeout: Duration,
    operation_timeout: Duration,
    checkouts: AtomicU64,
    reset: String,
}

fn failure(message: impl Into<String>) -> Error {
    Error::QueryBuilderError(Box::new(std::io::Error::other(message.into())))
}

/// A dropped operation must close its session, never silently recycle an active query.
struct CheckedOut {
    connection: Option<Object<AsyncPgConnection>>,
    completed: bool,
    cleanup_timeout: Duration,
    admission: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for CheckedOut {
    fn drop(&mut self) {
        if !self.completed
            && let Some(connection) = self.connection.take()
        {
            let connection = Object::take(connection);
            // A callback error does not prove SQL has finished: dropping an
            // unfinished row stream can leave its query executing on PostgreSQL.
            cancel_and_discard(connection, self.admission.take(), self.cleanup_timeout);
        }
    }
}

fn cancel_and_discard(
    mut connection: AsyncPgConnection,
    admission: Option<tokio::sync::OwnedSemaphorePermit>,
    cleanup_timeout: Duration,
) {
    let cancellation = connection.cancel_token();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            // Keep admission until cleanup completes, even though the pool slot is removed.
            let _admission = admission;
            let _ = tokio::time::timeout(cleanup_timeout, async {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    cancellation.cancel_query(tokio_postgres::NoTls),
                )
                .await;
                let _ = connection.batch_execute("ROLLBACK").await;
            })
            .await;
            drop(connection);
        });
    }
    // Without a runtime, dropping the owned connection still closes its transport.
}

impl Database {
    pub fn connect(url: &str, pool_size: usize) -> QueryResult<Self> {
        Self::with_timeouts(
            url,
            pool_size,
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
    }

    pub fn with_timeouts(
        url: &str,
        pool_size: usize,
        checkout_timeout: Duration,
        operation_timeout: Duration,
    ) -> QueryResult<Self> {
        if pool_size == 0 {
            return Err(failure("database pool size must be positive"));
        }
        if checkout_timeout.is_zero() || operation_timeout.is_zero() {
            return Err(failure("database timeouts must be positive"));
        }
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
        let statement_ms = operation_timeout.as_millis().max(1);
        let setup = format!(
            "SET TIME ZONE 'UTC'; SET CLIENT_ENCODING TO 'UTF8'; SET statement_timeout = {statement_ms}; SET lock_timeout = {statement_ms}; SET idle_in_transaction_session_timeout = {statement_ms};"
        );
        let reset = format!("ROLLBACK; SELECT pg_advisory_unlock_all(); RESET ALL; {setup}");
        let pool = Pool::builder(manager)
            .max_size(pool_size)
            .runtime(deadpool::Runtime::Tokio1)
            .post_create(Hook::async_fn(
                move |connection: &mut AsyncPgConnection, _| {
                    let setup = setup.clone();
                    Box::pin(async move {
                        connection
                            .batch_execute(&setup)
                            .await
                            .map_err(|e| HookError::Backend(PoolError::QueryError(e)))
                    })
                },
            ))
            .build()
            .map_err(|e| failure(e.to_string()))?;
        Ok(Self(Arc::new(Inner {
            pool: horme::Db::new(pool),
            admission: Arc::new(Semaphore::new(pool_size.saturating_mul(2))),
            checkout_timeout,
            operation_timeout,
            checkouts: AtomicU64::new(0),
            reset,
        })))
    }

    /// Borrow once for this callback; return the connection before returning its result.
    /// Keep callbacks limited to database work. Same-task reentry into this pool is
    /// rejected before checkout; pass this connection through helpers instead.
    /// External waits and separately spawned tasks still require caller discipline
    /// and remain bounded by admission and operation deadlines.
    ///
    /// Results cannot borrow the callback's connection:
    ///
    /// ```compile_fail
    /// use hagency_db_runtime::Database;
    /// use horme::diesel_async::AsyncPgConnection;
    ///
    /// async fn escape(db: &Database) -> &mut AsyncPgConnection {
    ///     db.with_connection(async |conn| Ok::<_, diesel::result::Error>(conn))
    ///         .await.unwrap()
    /// }
    /// ```
    #[allow(
        clippy::manual_async_fn,
        reason = "Borrowed async callbacks require an explicit Send future bound"
    )]
    pub fn with_connection<'a, 'conn, R, E, F>(
        &'conn self,
        callback: F,
    ) -> impl Future<Output = Result<R, E>> + Send + 'conn
    where
        for<'r> F: AsyncFnOnce(&'r mut AsyncPgConnection) -> Result<R, E>
            + horme::TransactionCallback<&'r mut AsyncPgConnection, Result<R, E>>
            + Send
            + 'a,
        E: From<Error> + Send + 'a,
        R: Send + 'a,
        'a: 'conn,
    {
        async move {
            let identity = Arc::as_ptr(&self.0) as usize;
            let mut active = ACTIVE_DATABASES.try_with(Clone::clone).unwrap_or_default();
            if active.contains(&identity) {
                return Err(failure(
                    "nested acquisition of the same database pool; reuse the callback connection",
                )
                .into());
            }
            active.push(identity);
            let admission = self
                .0
                .admission
                .clone()
                .try_acquire_owned()
                .map_err(|_| failure("database admission capacity exceeded"))?;
            // Acquire through Horme only while the callback runs.
            let mut provider = self.0.pool.clone();
            let connection = tokio::time::timeout(self.0.checkout_timeout, provider.get_mut_conn())
                .await
                .map_err(|_| failure("database connection checkout timed out"))?
                .map_err(|error| match error {
                    horme::Error::Query(error) => error,
                    horme::Error::Acquire(error) => Error::QueryBuilderError(error),
                })?;
            self.0.checkouts.fetch_add(1, Ordering::Relaxed);
            let mut checked = CheckedOut {
                connection: Some(connection),
                completed: false,
                cleanup_timeout: self
                    .0
                    .operation_timeout
                    .saturating_add(Duration::from_secs(2)),
                admission: Some(admission),
            };
            let result = tokio::time::timeout(
                self.0.operation_timeout,
                ACTIVE_DATABASES.scope(active, async {
                    let connection = checked.connection.as_mut().expect("checked out");
                    let output = callback(connection).await?;
                    // Restore session state and release session locks before publishing the result.
                    // Cleanup is part of the operation deadline, not delayed until the next borrower.
                    connection
                        .batch_execute(&self.0.reset)
                        .await
                        .map_err(E::from)?;
                    Ok::<_, E>(output)
                }),
            )
            .await
            .map_err(|_| failure("database operation timed out"))?;
            // An error can leave a transaction or protocol state behind: discard conservatively.
            checked.completed = result.is_ok();
            drop(checked);
            result
        }
    }

    pub fn status(&self) -> deadpool::managed::Status {
        self.0.pool.pool().status()
    }
    pub fn checkout_count(&self) -> u64 {
        self.0.checkouts.load(Ordering::Relaxed)
    }
}

/// Preserve upstream Diesel errors, including NotFound, for application helpers.
pub fn query_error(error: horme::Error) -> Error {
    match error {
        horme::Error::Query(error) => error,
        horme::Error::Acquire(error) => Error::QueryBuilderError(error),
    }
}
