//! Shared shutdown and health state; no authority, queue bodies or credentials.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Clone)]
pub struct WorkerRuntime {
    inner: Arc<Inner>,
}
struct Inner {
    stop: watch::Sender<bool>,
    queues: Mutex<BTreeMap<&'static str, QueueState>>,
}
struct QueueState {
    failures: u64,
    last_success: Option<Instant>,
    last_activity: Instant,
    last_log: Option<Instant>,
    running: bool,
    error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    name: &'static str,
    running: bool,
    consecutive_failures: u64,
    last_success_age_ms: Option<u64>,
    last_activity_age_ms: u64,
    error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub healthy: bool,
    pub stopping: bool,
    queues: Vec<QueueSnapshot>,
}
impl Default for WorkerRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                stop: watch::channel(false).0,
                queues: Mutex::new(BTreeMap::new()),
            }),
        }
    }
}
impl WorkerRuntime {
    pub fn begin_shutdown(&self) {
        self.inner.stop.send_replace(true);
    }
    pub fn is_stopping(&self) -> bool {
        *self.inner.stop.borrow()
    }
    pub(crate) async fn stopped(&self) {
        let mut receiver = self.inner.stop.subscribe();
        receiver.wait_for(|stopping| *stopping).await.ok();
    }
    pub fn health(&self) -> HealthSnapshot {
        let now = Instant::now();
        let queues = self.inner.queues.lock().unwrap_or_else(|e| e.into_inner());
        let healthy = !self.is_stopping()
            && !queues.is_empty()
            && queues.values().all(|q| {
                q.running
                    && q.failures < 3
                    && now.duration_since(q.last_activity) < Duration::from_secs(60)
            });
        HealthSnapshot {
            healthy,
            stopping: self.is_stopping(),
            queues: queues
                .iter()
                .map(|(&name, q)| QueueSnapshot {
                    name,
                    running: q.running,
                    consecutive_failures: q.failures,
                    last_success_age_ms: q
                        .last_success
                        .map(|t| now.duration_since(t).as_millis() as u64),
                    last_activity_age_ms: now.duration_since(q.last_activity).as_millis() as u64,
                    error: q.error.clone(),
                })
                .collect(),
        }
    }
    pub(crate) fn queue(&self, name: &'static str) -> QueueContext {
        self.inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                name,
                QueueState {
                    failures: 0,
                    last_success: None,
                    last_activity: Instant::now(),
                    last_log: None,
                    running: true,
                    error: None,
                },
            );
        QueueContext {
            runtime: self.clone(),
            name,
            pass_error: Mutex::new(None),
        }
    }
}
pub(crate) struct QueueContext {
    runtime: WorkerRuntime,
    name: &'static str,
    pass_error: Mutex<Option<&'static str>>,
}
impl QueueContext {
    /// Recheck immediately before each new claim. Current claimed IO is drained.
    pub(crate) fn stopping(&self) -> bool {
        self.progress();
        self.runtime.is_stopping()
    }
    pub(crate) fn progress(&self) {
        if let Some(q) = self
            .runtime
            .inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(self.name)
        {
            q.last_activity = Instant::now();
        }
    }
    pub(crate) fn observe_error(&self, error: &crate::Error) {
        if matches!(
            error,
            crate::Error::Database(_) | crate::Error::Unavailable(_)
        ) {
            self.failure("worker_dependency_unavailable");
        }
    }
    pub(crate) fn failure(&self, code: &'static str) {
        *self.pass_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
    }
    pub(crate) fn finish(&self, result: &crate::Result<()>) {
        let noted = self
            .pass_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let error = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .or_else(|| noted.map(str::to_owned));
        let now = Instant::now();
        let mut queues = self
            .runtime
            .inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let q = queues.get_mut(self.name).expect("registered worker queue");
        q.last_activity = now;
        if let Some(error) = error {
            q.failures = q.failures.saturating_add(1);
            q.error = Some(error.clone());
            if q.last_log
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(30))
            {
                tracing::warn!(queue=self.name,code=%error,consecutive_failures=q.failures,"Agent worker pass failed");
                q.last_log = Some(now);
            }
        } else {
            if q.failures >= 3 {
                tracing::info!(
                    queue = self.name,
                    previous_failures = q.failures,
                    "Agent worker recovered"
                );
            }
            q.failures = 0;
            q.error = None;
            q.last_success = Some(now);
        }
    }
}
impl Drop for QueueContext {
    fn drop(&mut self) {
        if let Some(q) = self
            .runtime
            .inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(self.name)
        {
            q.running = false;
            if !self.runtime.is_stopping() {
                q.error = Some("worker_exited".into());
                q.failures = q.failures.max(3);
                tracing::error!(queue = self.name, "Agent worker exited unexpectedly");
            }
        }
    }
}

/// Stop new passes, allow owned IO and its receipt to finish, then abort on deadline.
/// Timeout cancellation never resets persistent queue state or authorizes a retry.
pub async fn drain_workers(
    runtime: &WorkerRuntime,
    workers: &mut [tokio::task::JoinHandle<()>],
    deadline: Duration,
) -> bool {
    runtime.begin_shutdown();
    let result = tokio::time::timeout(deadline, async {
        let mut clean = true;
        for worker in workers.iter_mut() {
            if let Err(error) = worker.await {
                clean = false;
                tracing::error!(
                    code = "worker_join_failed",
                    cancelled = error.is_cancelled(),
                    panicked = error.is_panic(),
                    "Agent worker did not stop normally"
                );
            }
        }
        clean
    })
    .await;
    if result.is_err() {
        tracing::warn!(
            code = "worker_drain_timeout",
            "Agent worker drain deadline expired; uncertain operations retain their original state"
        );
        for worker in workers.iter() {
            worker.abort();
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            for worker in workers.iter_mut() {
                // Earlier handles may already have been consumed before the
                // shared deadline elapsed. A completed JoinHandle is not polled twice.
                if !worker.is_finished() {
                    let _ = worker.await;
                }
            }
        })
        .await;
        false
    } else {
        result.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shared_drain_deadline_cancels_owned_future_after_an_earlier_worker_finished() {
        struct OwnedWait(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for OwnedWait {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owned = dropped.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let blocked = tokio::spawn(async move {
            let _owned = OwnedWait(owned);
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered_rx.await.unwrap();
        let completed = tokio::spawn(async {});
        while !completed.is_finished() {
            tokio::task::yield_now().await;
        }
        let runtime = WorkerRuntime::default();
        let started = Instant::now();
        assert!(
            !drain_workers(
                &runtime,
                &mut [completed, blocked],
                Duration::from_millis(10)
            )
            .await
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
}
