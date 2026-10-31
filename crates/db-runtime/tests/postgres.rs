use std::time::Duration;

use diesel::{QueryableByName, sql_types::Bool};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use hagency_db_runtime::Database;

fn url() -> String {
    std::env::var("DB_TEST_DATABASE_URL").expect("an isolated DB_TEST_DATABASE_URL is required")
}

async fn ping(db: &Database) {
    tokio::time::timeout(
        Duration::from_secs(3),
        db.with_connection(async |connection| {
            diesel::sql_query("SELECT 1").execute(connection).await
        }),
    )
    .await
    .expect("the next operation must make progress")
    .unwrap();
}

#[tokio::test]
async fn nested_same_pool_fails_before_checkout_and_independent_pools_work() {
    let db = Database::connect(&url(), 1).unwrap();
    let clone = db.clone();
    db.with_connection(async |connection| {
        let error = tokio::time::timeout(
            Duration::from_millis(200),
            clone.with_connection(async |inner| diesel::sql_query("SELECT 1").execute(inner).await),
        )
        .await
        .expect("nested acquisition must fail without waiting for the held slot")
        .unwrap_err();
        assert!(error.to_string().contains("nested acquisition"), "{error}");
        diesel::sql_query("SELECT 1").execute(connection).await
    })
    .await
    .unwrap();
    assert_eq!(db.checkout_count(), 1);
    assert_eq!(db.status().available, 1);

    // Task-local scope must not mark unrelated futures as nested or leak after an error.
    let ((), ()) = tokio::join!(ping(&db), ping(&clone));
    let other = Database::connect(&url(), 1).unwrap();
    db.with_connection(async |connection| {
        ping(&other).await;
        diesel::sql_query("SELECT 1").execute(connection).await
    })
    .await
    .unwrap();
    ping(&db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_running_query_releases_database_lock_and_pool_slot() {
    let db = Database::connect(&url(), 1).unwrap();
    let task_db = db.clone();
    let (ready, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        task_db
            .with_connection(async |connection| {
                connection
                    .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328998)")
                    .await?;
                ready.send(()).unwrap();
                diesel::sql_query("SELECT pg_sleep(20)")
                    .execute(connection)
                    .await
            })
            .await
    });
    started.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    #[derive(QueryableByName)]
    struct Acquired {
        #[diesel(sql_type = Bool)]
        acquired: bool,
    }
    let mut observer = AsyncPgConnection::establish(&url()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let acquired =
                diesel::sql_query("SELECT pg_try_advisory_xact_lock(5210750088328998) AS acquired")
                    .get_result::<Acquired>(&mut observer)
                    .await
                    .unwrap()
                    .acquired;
            if acquired {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("cancellation must stop SQL and release transaction locks");
    ping(&db).await;
}

#[tokio::test]
async fn operation_deadline_releases_slot_and_unpolled_or_completed_futures_do_not_hold_it() {
    let db = Database::with_timeouts(
        &url(),
        1,
        Duration::from_secs(1),
        Duration::from_millis(200),
    )
    .unwrap();
    let future = db.with_connection(async |connection| {
        diesel::sql_query("SELECT 1").execute(connection).await
    });
    drop(future);
    assert_eq!(db.checkout_count(), 0);
    let mut completed = Box::pin(db.with_connection(async |connection| {
        diesel::sql_query("SELECT 1").execute(connection).await
    }));
    completed.as_mut().await.unwrap();
    assert_eq!(
        db.status().available,
        1,
        "release before the completed future is dropped"
    );
    let error = db
        .with_connection(async |_connection| {
            std::future::pending::<diesel::QueryResult<()>>().await
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("operation timed out"));
    ping(&db).await;
}

#[test]
fn zero_pool_size_or_timeouts_are_rejected() {
    assert!(Database::connect("postgres://unused", 0).is_err());
    assert!(
        Database::with_timeouts(
            "postgres://unused",
            1,
            Duration::ZERO,
            Duration::from_secs(1)
        )
        .is_err()
    );
    assert!(
        Database::with_timeouts(
            "postgres://unused",
            1,
            Duration::from_secs(1),
            Duration::ZERO
        )
        .is_err()
    );
}

#[tokio::test]
async fn terminated_idle_connection_is_replaced_before_the_next_query() {
    #[derive(QueryableByName)]
    struct Backend {
        #[diesel(sql_type = diesel::sql_types::Integer)]
        pid: i32,
    }
    let db = Database::connect(&url(), 1).unwrap();
    let previous = db
        .with_connection(async |connection| {
            diesel::sql_query("SELECT pg_backend_pid() AS pid")
                .get_result::<Backend>(connection)
                .await
        })
        .await
        .unwrap()
        .pid;
    let mut observer = AsyncPgConnection::establish(&url()).await.unwrap();
    diesel::sql_query("SELECT pg_terminate_backend($1)")
        .bind::<diesel::sql_types::Integer, _>(previous)
        .execute(&mut observer)
        .await
        .unwrap();
    let replacement = db
        .with_connection(async |connection| {
            diesel::sql_query("SELECT pg_backend_pid() AS pid")
                .get_result::<Backend>(connection)
                .await
        })
        .await
        .unwrap()
        .pid;
    assert_ne!(previous, replacement);
    assert_eq!(db.status().available, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_callback_with_abandoned_stream_stops_sql_and_releases_locks() {
    // Flush an initial result batch before the backend sleeps, so load_stream
    // returns while SQL is still executing rather than waiting for its first row.
    const QUERY: &str = "SELECT true AS acquired FROM generate_series(1,1000) UNION ALL SELECT true AS acquired FROM pg_sleep(20) /* callback_error_probe */";
    #[derive(QueryableByName)]
    struct Acquired {
        #[diesel(sql_type = Bool)]
        acquired: bool,
    }
    let db = Database::connect(&url(), 1).unwrap();
    let task_db = db.clone();
    let (ready, started) = tokio::sync::oneshot::channel();
    let (finish, released) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        task_db
            .with_connection(async |connection| {
                connection
                    .batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328996)")
                    .await?;
                let stream = diesel::sql_query(QUERY)
                    .load_stream::<Acquired>(connection)
                    .await?;
                ready.send(()).unwrap();
                released.await.unwrap();
                drop(stream);
                Err::<(), _>(diesel::result::Error::QueryBuilderError(Box::new(
                    std::io::Error::other("callback failed"),
                )))
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), started)
        .await
        .unwrap()
        .unwrap();
    let mut observer = AsyncPgConnection::establish(&url()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let running = diesel::sql_query("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND state='active' AND wait_event='PgSleep' AND query=$1) AS acquired")
                .bind::<diesel::sql_types::Text,_>(QUERY).get_result::<Acquired>(&mut observer).await.unwrap().acquired;
            if running { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("observe SQL execution before abandoning the stream");
    finish.send(()).unwrap();
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("callback failed")
    );
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if diesel::sql_query("SELECT pg_try_advisory_xact_lock(5210750088328996) AS acquired")
                .get_result::<Acquired>(&mut observer)
                .await
                .unwrap()
                .acquired
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an error must cancel unfinished SQL and release its transaction lock");
    ping(&db).await;
}
