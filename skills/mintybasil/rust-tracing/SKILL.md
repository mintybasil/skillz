---
name: rust-tracing
description: >
  Reference guide and style guide for structured logging and tracing in Rust using the `tracing`
  crate. Use this skill whenever writing, reviewing, or refactoring Rust code that involves
  logging, observability, spans, instrumentation, or diagnostics. Covers log level conventions,
  the `#[instrument]` macro, span creation, field attachment, and common anti-patterns to avoid.
  Trigger on any mention of tracing, logging, observability, `tracing::info!`, `log::debug!`,
  spans, or "how should I log this" in a Rust context.
---

# Rust Tracing Style Guide

This skill covers idiomatic structured logging in Rust using the [`tracing`](https://docs.rs/tracing) crate. It is both a reference document and a style guide.

## Core Philosophy

**Spans over fields.** Rather than stuffing context into individual log lines, structure your code so that context flows naturally from the call stack via spans. The `#[instrument]` macro is the primary tool for this. A well-instrumented codebase reads cleanly at the log-site and produces rich, structured diagnostics automatically.

**Log levels are a contract.** Consumers of your logs — operators, on-call engineers, monitoring pipelines — rely on levels having consistent semantics. Noisy `info!` logs erode trust; missing `debug!` logs frustrate debugging. Get the levels right and keep them right.

---

## Log Level Conventions

| Level   | When to use |
|---------|-------------|
| `error!` | Something has gone wrong and requires attention. Unexpected failures, unrecoverable states. |
| `warn!`  | Something unexpected happened but the system is continuing. Degraded state, retries, deprecation notices. |
| `info!`  | Significant, operator-relevant events. Service start/stop, meaningful state transitions, completion of high-level operations. Should be low-volume and always useful. |
| `debug!` | Developer-relevant detail. Intermediate values, control flow decisions, lower-level operation outcomes. Safe to enable in production during an incident. |
| `trace!` | High-frequency, fine-grained events. Per-packet, per-iteration, or per-request inner-loop detail. Should be disabled in all but the most intense tracing sessions. |

### Info level guidance

`info!` should answer the question "what is this service doing right now?" for an operator who has never read the source code. Every `info!` line should be something an operator would plausibly want to see. Avoid:

- Logging every item in a loop at `info!`
- Logging internal intermediate steps at `info!`
- Duplicating information already in a span

```rust
// ❌ Too noisy — these belong at debug! or inside a span
info!("Processing item {}", item.id);
info!("Calling downstream service");
info!("Downstream service responded");

// ✅ One meaningful info! log for the whole operation
info!(item_count = items.len(), "Batch processing complete");
```

### Trace level guidance

`trace!` is for logs you would only enable during an intense debugging or tracing session — typically inner loops, per-message events, or protocol-level detail. It should almost never appear in library code paths that are hot in production.

---

## Spans and the `#[instrument]` Macro

### Prefer `#[instrument]` over manual span creation

The `#[instrument]` macro is the idiomatic way to create spans. It automatically:

- Creates a span named after the function
- Records function arguments as span fields
- Enters and exits the span for the duration of the call
- Handles async correctly (attaches the span to the future)

```rust
use tracing::instrument;

// ✅ Clean — span context is implicit, arguments are captured automatically
#[instrument]
async fn fetch_user(user_id: u64) -> Result<User, Error> {
    debug!("Querying database");
    // ...
}

// ❌ Verbose — manual span creation is rarely needed
async fn fetch_user(user_id: u64) -> Result<User, Error> {
    let span = tracing::info_span!("fetch_user", user_id);
    let _guard = span.enter();
    debug!("Querying database");
    // ...
}
```

### Controlling which fields are recorded

By default, `#[instrument]` records all function arguments. Use `skip` or `skip_all` to exclude fields that are noisy, sensitive, or not `Debug`:

```rust
// Skip fields that are too large or sensitive
#[instrument(skip(password, large_payload))]
async fn authenticate(username: &str, password: &str, large_payload: Bytes) -> Result<Token, Error> {
    // ...
}

// Skip everything and selectively add what matters
#[instrument(skip_all, fields(user_id = %user.id, request_id = %req.id))]
async fn handle_request(user: &User, req: &Request) -> Response {
    // ...
}
```

### Adding computed fields

Use `fields(...)` to add context that isn't directly a function argument:

```rust
#[instrument(fields(otel.kind = "server", http.method = %req.method(), http.path = %req.uri().path()))]
async fn handle_http(req: Request<Body>) -> Response<Body> {
    // ...
}
```

You can also record fields after the fact using `Span::current()`:

```rust
#[instrument]
async fn process_job(job_id: Uuid) -> Result<(), Error> {
    let result = run_job(job_id).await?;
    tracing::Span::current().record("result_code", result.code);
    Ok(())
}
```

### Naming spans explicitly

By default the span name matches the function name. Override it when the function name is ambiguous:

```rust
#[instrument(name = "db.query.user_by_email")]
async fn get_by_email(email: &str) -> Result<Option<User>, DbError> {
    // ...
}
```

---

## Refactoring for Instrumentability

If you find yourself wanting to add context to a log line — an ID, a correlation token, a piece of state — that's often a signal to refactor so that context lives in a span instead.

### Before: context scattered into log lines

```rust
async fn process_order(order_id: u64, customer_id: u64) {
    debug!("Processing order {} for customer {}", order_id, customer_id);
    
    if let Err(e) = validate_order(order_id, customer_id).await {
        error!("Failed to validate order {} for customer {}: {}", order_id, customer_id, e);
        return;
    }

    if let Err(e) = charge_customer(order_id, customer_id).await {
        error!("Failed to charge customer {} for order {}: {}", customer_id, order_id, e);
        return;
    }

    info!("Order {} for customer {} processed successfully", order_id, customer_id);
}
```

### After: context in spans, log lines are clean

```rust
#[instrument]
async fn process_order(order_id: u64, customer_id: u64) {
    if let Err(e) = validate_order(order_id, customer_id).await {
        error!(error = %e, "Order validation failed");
        return;
    }

    if let Err(e) = charge_customer(order_id, customer_id).await {
        error!(error = %e, "Customer charge failed");
        return;
    }

    info!("Order processed successfully");
}

#[instrument]
async fn validate_order(order_id: u64, customer_id: u64) -> Result<(), ValidationError> {
    // ...
}

#[instrument]
async fn charge_customer(order_id: u64, customer_id: u64) -> Result<(), PaymentError> {
    // ...
}
```

`order_id` and `customer_id` now appear automatically on every log line emitted anywhere in the call tree, without being explicitly passed to each log call. Callees don't need to know about them.

This pattern also tends to surface better function boundaries — if a function is hard to `#[instrument]` cleanly, it may be doing too much.

---

## Structured Fields vs. String Formatting

Prefer structured fields over string interpolation. Structured fields can be queried and indexed by log aggregation systems.

```rust
// ❌ Opaque string — hard to query
info!("User {} logged in from {}", user_id, ip);

// ✅ Structured — queryable, indexable
info!(user_id, %ip, "User logged in");
```

Field value sigils:
- No sigil: uses the `Value` trait (primitives, integers, booleans)
- `%value`: uses `Display`
- `?value`: uses `Debug`

Prefer no sigil or `%` over `?` where possible — `Debug` output is often verbose and unstructured.

---

## Error Logging

Log errors at the site where they are handled, not where they are created or propagated. Use structured fields for the error value:

```rust
// ✅ Error logged where it's handled, structured
if let Err(e) = do_thing().await {
    error!(error = %e, "Failed to do thing");
    return Err(e.into());
}

// ❌ Don't log-and-propagate — leads to duplicate log entries
let result = do_thing().await.map_err(|e| {
    error!("Failed: {}", e);  // logged here...
    e
})?;
// ...and potentially logged again by the caller
```

---

## Crate Setup

Add to `Cargo.toml`:

```toml
[dependencies]
tracing = "0.1"

# For binaries — choose a subscriber
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

Minimal subscriber initialization for a binary:

```rust
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // RUST_LOG=myapp=debug,info controls levels at runtime
}
```

For libraries: depend only on `tracing`, never on `tracing-subscriber`. Subscriber setup is the binary's responsibility.

---

## Quick Reference

See `references/patterns.md` for a catalogue of common patterns with annotated examples:
- Async tasks and spawned futures
- Tower middleware / axum handlers
- Background worker loops
- Integrating with OpenTelemetry

See `references/antipatterns.md` for a list of common mistakes and how to fix them.
