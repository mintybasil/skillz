# Common Tracing Patterns

## Async Tasks and Spawned Futures

Spans do not automatically follow futures across `tokio::spawn`. Use `.in_current_span()` to propagate:

```rust
use tracing::Instrument;

#[instrument]
async fn start_background_task(job_id: u64) {
    let span = tracing::Span::current();

    tokio::spawn(
        async move {
            debug!("Background task started");
            do_work(job_id).await;
        }
        .in_current_span(),  // ← carries the span into the spawned future
    );
}
```

Without `.in_current_span()`, logs from the spawned task will have no parent span and no `job_id` field.

## Axum / Tower HTTP Handlers

Instrument your handler functions directly. The framework will call them per-request:

```rust
#[instrument(
    skip(state, body),
    fields(
        http.method = %method,
        http.path = %uri.path(),
        user_id = tracing::field::Empty,  // will be filled in after auth
    )
)]
async fn create_widget(
    method: Method,
    uri: Uri,
    State(state): State<AppState>,
    body: Json<CreateWidgetRequest>,
) -> Result<Json<Widget>, AppError> {
    let user = authenticate(&state, &body).await?;
    tracing::Span::current().record("user_id", user.id);

    info!("Creating widget");
    // ...
}
```

For cross-cutting span enrichment (e.g., adding a request ID to every request's span), use a Tower middleware layer rather than repeating the pattern in every handler.

## Background Worker Loops

For long-running loops, create a span per iteration rather than one span for the entire loop. This keeps trace data manageable and avoids unbounded span durations:

```rust
#[instrument(skip(rx))]
async fn worker(rx: mpsc::Receiver<Job>) {
    info!("Worker started");

    while let Some(job) = rx.recv().await {
        // Create a child span per job — do NOT instrument the whole loop as one span
        process_job(job).await;
    }

    info!("Worker stopped");
}

#[instrument(fields(job.kind = %job.kind, job.id = %job.id))]
async fn process_job(job: Job) {
    debug!("Processing");
    // ...
    debug!("Done");
}
```

## Conditional Span Fields

Use `tracing::field::Empty` as a placeholder for fields that may or may not be populated:

```rust
#[instrument(fields(cache_hit = tracing::field::Empty))]
async fn get_data(key: &str) -> Data {
    if let Some(cached) = cache_lookup(key) {
        tracing::Span::current().record("cache_hit", true);
        return cached;
    }
    tracing::Span::current().record("cache_hit", false);
    fetch_from_db(key).await
}
```

## OpenTelemetry Integration

To export traces to an OTLP collector, add:

```toml
[dependencies]
tracing-opentelemetry = "0.24"
opentelemetry = "0.23"
opentelemetry-otlp = { version = "0.16", features = ["tonic"] }
opentelemetry_sdk = { version = "0.23", features = ["rt-tokio"] }
```

```rust
use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn init_tracing() -> opentelemetry_sdk::trace::Tracer {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint("http://localhost:4317");

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .expect("Failed to install OTel tracer");

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer.clone()))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracer
}
```

Span names set via `#[instrument(name = "...")]` map directly to OTel span names. Fields map to OTel span attributes.

## Testing Spans

Use `tracing-test` to assert on span and event output in unit tests:

```toml
[dev-dependencies]
tracing-test = "0.2"
```

```rust
#[cfg(test)]
mod tests {
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_fetch_emits_debug_log() {
        fetch_user(42).await.unwrap();
        assert!(logs_contain("Querying database"));
    }
}
```
