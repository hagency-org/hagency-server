//! OpenTelemetry bridge for the tracing subscriber.
//!
//! Context activation is disabled. With it on, tracing-opentelemetry 0.33
//! attaches an OTel context into a thread-local guard stack on span enter and
//! pops it on exit. Palpo/Pasion futures enter a span on one Tokio worker and
//! exit it on another, so the guard is never popped on the entering thread.
//! Each worker's stack grows until opentelemetry refuses with
//! `Context.AttachFailed "Too many contexts. Max limit is 65535"`, logged on
//! every later enter (millions of lines on the demo servers). Span parentage
//! and `OpenTelemetrySpanExt::context()` do not need activation.
use tracing_subscriber::registry::LookupSpan;

/// The OpenTelemetry layer used by the server's tracing subscriber.
pub fn otel_layer<S>()
-> tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry::global::BoxedTracer>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    tracing_opentelemetry::layer()
        .with_tracer(opentelemetry::global::tracer("hagency-server"))
        .with_context_activation(false)
}
