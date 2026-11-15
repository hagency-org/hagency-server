//! Regression: spans entered on one thread and exited on another must not leak
//! OpenTelemetry context guards (demo servers hit "Too many contexts. Max
//! limit is 65535" and flooded their logs).
use opentelemetry::Context;
use tracing_subscriber::layer::SubscriberExt;

const CROSS_THREAD_ENTERS: usize = 70_000;

#[derive(Clone, Debug, PartialEq)]
struct Marker;

#[test]
fn cross_thread_span_enter_does_not_exhaust_otel_context_stack() {
    let subscriber = tracing_subscriber::registry().with(hagency_server::telemetry::otel_layer());
    let dispatch = tracing::Dispatch::new(subscriber);
    let span = tracing::dispatcher::with_default(&dispatch, || tracing::info_span!("cross_thread"));
    let id = span.id().expect("span is enabled");
    for _ in 0..CROSS_THREAD_ENTERS {
        dispatch.enter(&id);
        let (d, i) = (dispatch.clone(), id.clone());
        std::thread::spawn(move || d.exit(&i)).join().unwrap();
    }
    let _guard = Context::current_with_value(Marker).attach();
    assert_eq!(Context::current().get::<Marker>(), Some(&Marker));
}
