## Summary

Runtime artifacts and absolute-path Bash commands still pollute quality signals.

Issue 448 E2E runs include `.anvil-state`, generated files, and `__pycache__` in changed-file telemetry. Some model Bash commands also used absolute `cd` despite repository-relative path guidance.

## Problem

Dirty changed-file telemetry makes AnvilScore, CaseRecord, and follow-up evaluation less reliable. Generated artifacts can look like user-visible changes, and absolute commands make local LLM behavior less portable.

## Acceptance Criteria

- Exclude `.anvil-state/**`, `target/**`, `.pytest_cache/**`, `__pycache__/**`, and common generated artifacts from user-facing changed-file telemetry.
- Keep raw session logs available, but separate runtime artifacts from repository edits in milestone events.
- Reinforce or enforce repository-relative Bash where possible.
- Add E2E assertions that a simple Python verification does not count `__pycache__` as a user edit.

## Evidence

- Issue 448 additional src-scoped ANVIL run changed `src/app.py` and generated `src/__pycache__/app.cpython-312.pyc`.
- Earlier Issue 448 Rust fixture telemetry included generated `Cargo.lock`/target artifacts for small test runs.
