# Doc Comments: Public API Contracts

Every public function, method, and type gets a doc comment — no exceptions. Doc comments describe the **contract** (what the function promises), not the **implementation** (how it works internally).

## Required Elements

| Element | Required | Example |
|---------|----------|---------|
| One-line summary | Always | "Compute the hash of the provided data." |
| Parameters | Always | `data` — bytes to hash, must not be empty |
| Return value | Always | The 32-byte hash |
| Errors/panics | If applicable | Panics if `data` is empty |
| Examples | Non-trivial functions | See below |

## Contract, not implementation

```
/// Compute the hash of the provided data using the configured algorithm.
///
/// # Arguments
/// * `data` - The bytes to hash. Must not be empty.
///
/// # Returns
/// The 32-byte hash as a fixed-size array.
///
/// # Panics
/// Panics if `data` is empty — callers must validate input first.
///
/// # Example
/// let hash = hasher.compute(b"hello world");
/// assert_eq!(hash.len(), 32);
pub fn compute(&self, data: &[u8]) -> [u8; 32] { ... }
```

**Bad** — describes implementation:
```
/// Initializes the internal state buffer to zeros, then loops over
/// the data in 64-byte chunks, applying the compression function...
pub fn compute(&self, data: &[u8]) -> [u8; 32] { ... }
```

Include a runnable example for any function where a caller might not immediately see how to use it. Simple getters don't need one; functions with setup requirements or non-obvious calling patterns do.