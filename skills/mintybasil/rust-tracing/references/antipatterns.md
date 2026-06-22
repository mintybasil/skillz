# Tracing Anti-Patterns

## Repeating context in every log line

**Problem:** Manually threading IDs through every log call creates noise and diverges as the codebase evolves.

```rust
// ❌
debug!("Fetching order {order_id} for user {user_id}");
debug!("Order {order_id} found, validating for user {user_id}");
debug!("Validation passed for order {order_id}, user {user_id}");
```

**Fix:** Add `#[instrument]` to the enclosing function. The fields appear automatically on all events emitted within its scope.

---

## Logging in libraries at info! or above

**Problem:** Library crates that emit `info!` or `warn!` logs appear in every application that uses them, regardless of whether the operator wants that noise.

```rust
// ❌ In a library crate
pub fn connect(addr: &str) -> Connection {
    info!("Connecting to {addr}");  // operators can't easily suppress this
    // ...
}
```

**Fix:** Use `debug!` or `trace!` in libraries. Let the application control what surfaces at `info!`.

---

## Entering spans in async code with `span.enter()`

**Problem:** `span.enter()` returns a synchronous guard. Holding it across an `.await` point will cause the span to be entered in one task and exited in another, corrupting the trace.

```rust
// ❌ Broken with async
async fn do_work() {
    let span = info_span!("work");
    let _guard = span.enter();  // guard held across await — wrong!
    some_async_call().await;
}
```

**Fix:** Use `#[instrument]`, or `.instrument(span)` / `.in_current_span()` on the future directly.

```rust
// ✅
#[instrument]
async fn do_work() {
    some_async_call().await;
}

// ✅ Manual alternative
async fn do_work() {
    let span = info_span!("work");
    some_async_call().instrument(span).await;
}
```

---

## Log-and-propagate (double logging errors)

**Problem:** Logging an error when it's created and again when it's handled produces duplicate entries and makes log aggregation harder.

```rust
// ❌ Error logged twice
fn parse_config(raw: &str) -> Result<Config, Error> {
    let config = serde_json::from_str(raw).map_err(|e| {
        error!("Config parse error: {e}");  // logged here
        e
    })?;
    Ok(config)
}

fn start() {
    if let Err(e) = parse_config(RAW) {
        error!("Failed to start: {e}");  // and here
    }
}
```

**Fix:** Log errors only at the point where they are handled and the operation is abandoned. Let errors propagate silently until then.

---

## Using `?` with no context in instrumented functions

**Problem:** Returning errors with `?` gives you no log output at all unless the caller logs them, leading to silent failures in traces.

**Fix:** Log at the error site before propagating, or use a crate like `tracing-error` to attach span context to errors:

```rust
#[instrument]
async fn risky_operation() -> Result<(), Error> {
    let result = inner().await;
    if let Err(ref e) = result {
        warn!(error = %e, "Inner operation failed");
    }
    result
}
```

---

## Overusing `?` fields (Debug formatting)

**Problem:** `?value` uses `Debug`, which often produces extremely verbose output with unnecessary internal detail.

```rust
// ❌ Dumps entire struct
debug!(?response, "Got response");
```

**Fix:** Use `%` (Display) or record specific fields:

```rust
// ✅
debug!(status = %response.status(), "Got response");
```

---

## Span-per-loop-iteration using `#[instrument]` on the outer loop

**Problem:** A single span that spans an entire loop iteration over thousands of items creates one enormous span in your trace backend.

```rust
// ❌ One giant span for the whole function
#[instrument]
async fn process_all(items: Vec<Item>) {
    for item in items {
        process_one(&item).await;
    }
}
```

**Fix:** Instrument the inner function so each iteration gets its own span:

```rust
async fn process_all(items: Vec<Item>) {
    for item in items {
        process_one(&item).await;  // each call gets its own span
    }
}

#[instrument(fields(item.id = %item.id))]
async fn process_one(item: &Item) {
    // ...
}
```

---

## Missing `skip_all` on functions with non-Debug arguments

**Problem:** `#[instrument]` will fail to compile if any argument doesn't implement `Debug`, and will produce verbose output for large or complex types.

```rust
// ❌ Won't compile if Body doesn't implement Debug
#[instrument]
async fn handle(state: Arc<AppState>, body: Body) -> Response { ... }
```

**Fix:** Use `skip_all` and manually specify the fields you care about:

```rust
// ✅
#[instrument(skip_all, fields(path = %req.uri().path()))]
async fn handle(state: Arc<AppState>, req: Request<Body>) -> Response { ... }
```
