use std::cell::RefCell;

thread_local! {
    static API_METRICS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

/// Record an API call metric.
pub fn record_api_call(duration_ms: f64) {
    API_METRICS.with(|m| {
        let mut metrics = m.borrow_mut();
        metrics.push(duration_ms);
        // Keep at most 200 entries to avoid unbounded growth
        if metrics.len() > 200 {
            let drain_count = metrics.len() - 200;
            metrics.drain(..drain_count);
        }
    });
}

/// Return the average latency (in ms) across all recorded metrics, or 0.0 if none.
pub fn average_latency() -> f64 {
    API_METRICS.with(|m| {
        let metrics = m.borrow();
        if metrics.is_empty() {
            return 0.0;
        }
        let total: f64 = metrics.iter().sum();
        total / metrics.len() as f64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_average_uses_only_the_latest_200_calls() {
        API_METRICS.with(|metrics| metrics.borrow_mut().clear());
        assert_eq!(average_latency(), 0.0);
        for duration in 0..=200 {
            record_api_call(f64::from(duration));
        }
        assert_eq!(average_latency(), 100.5);
        API_METRICS.with(|metrics| assert_eq!(metrics.borrow().len(), 200));
    }
}
