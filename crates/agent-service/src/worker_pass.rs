//! Pass-level error reporting for the background worker queues.

/// Logs a failed worker pass with its queue name, so a database or authority
/// error that stops a queue is visible to operators instead of discarded.
/// The loop still retries on its next tick; the return value says whether
/// this pass failed.
pub(crate) fn report_pass<T>(queue: &'static str, result: crate::Result<T>) -> bool {
    match result {
        Ok(_) => false,
        Err(error) => {
            tracing::warn!(stage = "worker_pass", queue, %error, "Worker pass failed; retrying next tick");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::report_pass;
    use crate::Error;

    #[test]
    fn failed_worker_pass_is_reported_not_dropped() {
        assert!(report_pass::<()>("progress_status", Err(Error::Unavailable("db_down"))));
        assert!(!report_pass("progress_status", Ok(3usize)));
    }
}
