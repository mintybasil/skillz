# Module-Level Documentation

Module-level docs explain the module's **purpose** and how it fits into the **larger system architecture**. They answer: "What is this module for, and why does it exist as a separate unit?"

```
//! Payment processing module.
//!
//! Handles the lifecycle of a payment from authorization through settlement.
//! Acts as the bridge between the API layer and the persistence layer.
//! The gateway adapter is pluggable — see the `Gateway` trait.
```

**Don't include:**
- A list of every public function (doc comments handle that)
- Implementation details (inline comments handle that)
- Changelog or history (see forbidden patterns in SKILL.md)