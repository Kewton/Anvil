---
name: codex-issue-worker
description: Implement one assigned Anvil issue in a dedicated git worktree.
---

# Codex Issue Worker

Use this skill inside a dedicated issue worktree after `/orchestrate` dispatches the issue.

## Required Flow

1. Read the Issue summary, acceptance criteria, and orchestration notes.
2. Inspect the smallest relevant code surface before editing.
3. Write `dev-reports/issue-<number>/design.md` before editing.
4. Implement the smallest coherent change; avoid broad architecture churn unless the Issue requires it.
5. Add or update focused tests when behavior changes.
6. Run focused verification first.
7. Run broader verification when shared behavior, CI-sensitive code, or release/harness code is touched.
8. Write `dev-reports/issue-<number>/implementation-summary.md`.
9. Write `dev-reports/issue-<number>/verification.md`.
10. Commit the change with a clear issue-scoped message.

## Anvil Verification Guidance

Prefer the narrowest command that proves the change, then broaden as risk grows:

- Rust code: `cargo test <filter> --lib -q`, then `cargo clippy --all-targets -- -D warnings` when shared code is touched.
- CLI/help surface: update and run the relevant snapshot/check script.
- Harness scripts: run the specific Python/unit/smoke check for the touched script.
- Formatting-sensitive changes: run `cargo fmt --all -- --check`.

Keep review lightweight. Ask only blocking questions.
