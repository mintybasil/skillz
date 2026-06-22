---
name: rust-git-hooks
description: Install a pre-commit git hook that runs `cargo fmt --all` automatically before every commit in a Rust repo. Prevents cargo fmt CI failures. Always set this up when working in any Rust repository.
version: 1.0.0
author: zeroklaw
license: MIT
metadata:
  hermes:
    tags: [Rust, Git, Hooks, Formatting, CI]
    related_skills: [github-pr-workflow]
---

# Rust Git Hooks

Always install this when starting work in any Rust repository. One command, no dependencies beyond rustfmt.

## Trigger

Set this up whenever you:
- Clone a Rust repo for the first time
- Start working in a Rust repo that lacks a pre-commit hook
- See a `cargo fmt` / `rustfmt` CI failure

## Install the pre-commit hook

```bash
REPO_ROOT=$(git rev-parse --show-toplevel)
HOOK="$REPO_ROOT/.git/hooks/pre-commit"

cat > "$HOOK" << 'EOF'
#!/bin/sh
# Run cargo fmt --all before every commit.
# Aborts the commit if rustfmt is not installed.

if ! command -v cargo >/dev/null 2>&1; then
  echo "pre-commit: cargo not found, skipping fmt check" >&2
  exit 0
fi

if ! cargo fmt --version >/dev/null 2>&1; then
  echo "pre-commit: rustfmt not installed (run: rustup component add rustfmt)" >&2
  exit 1
fi

cargo fmt --all

# Re-stage any files that were just reformatted so the commit
# contains the formatted versions, not the originals.
git diff --name-only | xargs -r git add

exit 0
EOF

chmod +x "$HOOK"
echo "pre-commit hook installed at $HOOK"
```

## Fix an existing fmt CI failure

```bash
cd <repo-root>
cargo fmt --all
git diff --stat          # confirm what changed
git add -u
git commit -m "style: cargo fmt --all"
git push
```

## Verify rustfmt is available

```bash
rustup component add rustfmt   # idempotent — safe to run if unsure
cargo fmt --version
```

## Pitfalls

- The hook uses `git diff --name-only | xargs -r git add` to re-stage reformatted files.
  On macOS, `xargs` does not support `-r`. If on macOS, replace with:
  `git diff --name-only | xargs git add` (works fine when there are no unstaged files).
- If the repo uses a workspace, `cargo fmt --all` must be run from the workspace root,
  not from an individual crate directory.
- git hooks are local to the clone — they are not committed to the repo.
  Each fresh clone needs this hook re-installed.
