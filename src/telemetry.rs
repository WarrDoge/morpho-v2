//! Traces and metrics. Spans and events are `tracing`; OpenTelemetry appears only here and
//! only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
use std::sync::{LazyLock, OnceLock};

use anyhow::Result;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram},
    trace::TracerProvider as _,
};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

static PROVIDERS: OnceLock<(SdkTracerProvider, SdkMeterProvider)> = OnceLock::new();
static RUN: OnceLock<String> = OnceLock::new();

/// Installs the log subscriber, plus OTLP export when an endpoint is configured.
pub fn init(service: &str, default: LevelFilter, run: Option<&str>) -> Result<()> {
    if let Some(run) = run {
        let _ = RUN.set(run.to_string());
    }
    let filter = EnvFilter::builder()
        .with_default_directive(default.into())
        .from_env_lossy();
    // The log filter is per layer: exported spans stay at INFO whatever the console shows.
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter);
    let registry = tracing_subscriber::registry().with(fmt);
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none() {
        registry.init();
        return Ok(());
    }
    let mut resource = Resource::builder().with_service_name(service.to_string());
    if let Some(run) = RUN.get() {
        resource = resource.with_attribute(KeyValue::new("run", run.clone()));
    }
    let resource = resource.build();
    let spans = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()?;
    let tracer = SdkTracerProvider::builder()
        .with_batch_exporter(spans)
        .with_resource(resource.clone())
        .build();
    let metrics = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .build()?;
    let meter = SdkMeterProvider::builder()
        .with_periodic_exporter(metrics)
        .with_resource(resource)
        .build();
    global::set_meter_provider(meter.clone());
    registry
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer.tracer(service.to_string()))
                .with_filter(LevelFilter::INFO),
        )
        .init();
    global::set_tracer_provider(tracer.clone());
    let _ = PROVIDERS.set((tracer, meter));
    Ok(())
}

/// Flushes exporters; a no-op without an endpoint.
pub fn shutdown() {
    if let Some((tracer, meter)) = PROVIDERS.get() {
        let _ = tracer.force_flush();
        let _ = tracer.shutdown();
        let _ = meter.shutdown();
    }
}

struct Instruments {
    calls: Counter<u64>,
    prompt: Histogram<u64>,
    completion: Histogram<u64>,
    duration: Histogram<f64>,
    changes: Counter<u64>,
    attempts: Counter<u64>,
    attempt_prompt: Counter<u64>,
    attempt_completion: Counter<u64>,
}

static INSTRUMENTS: LazyLock<Instruments> = LazyLock::new(|| {
    let m = global::meter("morpho");
    Instruments {
        calls: m.u64_counter("llm.calls").build(),
        prompt: m.u64_histogram("llm.prompt_tokens").build(),
        completion: m.u64_histogram("llm.completion_tokens").build(),
        duration: m.f64_histogram("llm.duration").with_unit("s").build(),
        changes: m.u64_counter("changes").build(),
        attempts: m.u64_counter("llm.attempts").build(),
        attempt_prompt: m.u64_counter("llm.attempt.prompt_tokens").build(),
        attempt_completion: m.u64_counter("llm.attempt.completion_tokens").build(),
    }
});

fn attrs(mut kv: Vec<KeyValue>) -> Vec<KeyValue> {
    if let Some(run) = RUN.get() {
        kv.push(KeyValue::new("run", run.clone()));
    }
    kv
}

/// One model call: `cache` is `live` (provider), `hit` (replayed) or `fake`.
pub fn record_call(
    kind: &str,
    model: &str,
    cache: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    seconds: f64,
) {
    let i = &*INSTRUMENTS;
    let a = attrs(vec![
        KeyValue::new("kind", kind.to_string()),
        KeyValue::new("model", model.to_string()),
        KeyValue::new("cache", cache.to_string()),
    ]);
    i.calls.add(1, &a);
    i.prompt.record(prompt_tokens, &a);
    i.completion.record(completion_tokens, &a);
    i.duration.record(seconds, &a);
}

pub fn record_attempt(
    kind: &str,
    model: &str,
    finish_reason: &str,
    outcome: &str,
    attempt: i64,
    prompt_tokens: u64,
    completion_tokens: u64,
) {
    let span = tracing::info_span!(
        "llm.attempt",
        kind,
        model,
        finish_reason,
        outcome,
        attempt,
        provider_prompt_tokens = prompt_tokens,
        provider_completion_tokens = completion_tokens,
    );
    let _entered = span.enter();
    let a = attrs(vec![
        KeyValue::new("kind", kind.to_string()),
        KeyValue::new("model", model.to_string()),
        KeyValue::new("finish_reason", finish_reason.to_string()),
        KeyValue::new("outcome", outcome.to_string()),
        KeyValue::new("attempt", attempt),
    ]);
    INSTRUMENTS.attempts.add(1, &a);
    INSTRUMENTS.attempt_prompt.add(prompt_tokens, &a);
    INSTRUMENTS.attempt_completion.add(completion_tokens, &a);
}

pub fn record_change(agent: &str, operation: &str, accepted: bool) {
    INSTRUMENTS.changes.add(
        1,
        &attrs(vec![
            KeyValue::new("agent", agent.to_string()),
            KeyValue::new("operation", operation.to_string()),
            KeyValue::new("accepted", accepted),
        ]),
    );
}
