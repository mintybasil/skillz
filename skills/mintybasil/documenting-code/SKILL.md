---
name: documenting-code
description: "Use when adding, reviewing, or auditing comments in source code — inline comments, doc comments on public APIs, or module-level documentation. Also trigger when asked to annotate code or review comment quality."
version: 1.1.0
author: Hermes Agent
license: MIT
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [documentation, comments, code-quality, best-practices]
    related_skills: [requesting-code-review, code-wiki]
---

# Documenting Code

## Overview

Comments explain **why** code exists, not **what** it does. Only add a comment when the code's intent can't be inferred from reading it. Self-evident code stays un-commented — leave it alone silently.

Three types of in-codebase documentation (load the reference file when working on that type):
- **Inline comments** — `references/inline-comments.md`
- **Doc comments** (public APIs) — `references/doc-comments.md`
- **Module-level docs** — `references/module-level-docs.md`

## When to Use

- Adding comments to new or existing code
- Reviewing or auditing comment quality
- Writing doc comments for a public API

**Don't use for:** README files, architecture docs, commit messages, PR descriptions, or generated API reference sites.

## The Core Test

> **Can a competent developer infer the code's intent by reading it?**
- **Yes** → No comment. Move on.
- **No** → Add a comment explaining **why**.

This is about intent, not mechanics. A reader may see that a retry loop caps at 3 (mechanics) without knowing *why* it's 3 and not 5 (intent). That gap is where comments belong.

## Forbidden Comment Patterns

### 1. Restating the code in English
If removing the comment loses no information, remove the comment.
```
x += 1  // BAD: increments x by 1
```

### 2. Commented-out code blocks
Dead code belongs in version history, not comments. Delete it. Git remembers.

### 3. Apologizing or self-deprecating
If the code needs an apology, refactor it. Track debt in an issue tracker, not a comment.
```
// This is ugly but it works for now  ← BAD
```

### 4. Documenting the past
**The most common failure mode for generated comments.** Comments must not reference things that no longer exist in the codebase. A future reader never saw the old code — references to it are noise.

**The test:** Does the comment reference something that no longer exists? If yes → delete it.

```
// We removed OldParser in favor of NewParser      ← BAD: narrates history
// Previously used a Vec, switched to HashMap       ← BAD: narrates history
// Uses HashMap because lookup must be O(1)         ← GOOD: current rationale
```

"Switched from X to Y" is history. "Y is used because Z" is rationale. Write rationale, not history.

### 5. TODO and FIXME
Track work in an issue tracker, not in code. TODOs accumulate, go stale, and create false confidence.

## Red Flags

| Thought | Reality |
|---------|---------|
| "I should explain what this does" | Code already says what. Explain why, or say nothing. |
| "More comments are better" | Noise comments train readers to skip all comments. |
| "Future readers need the history" | Future readers need to understand current code. History is in git. |
| "I'll leave this code commented out" | Git is your safety net. Delete it. |
| "A comment about what we removed adds context" | If it's gone, the comment is noise. Explain the current state. |

## Verification Checklist

- [ ] Inline comments explain why, not what
- [ ] Self-evident code left un-commented
- [ ] Every public function has a doc comment (contract, not implementation)
- [ ] Module docs explain purpose and architecture fit
- [ ] No restated code, dead code, apologies, historical narration, or TODOs