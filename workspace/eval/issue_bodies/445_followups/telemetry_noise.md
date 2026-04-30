## Context

Follow-up of #445.

Parent follow-up:
- #489

Issue 445 evaluation showed edited-file telemetry including runtime and verifier artifacts:

- `.anvil-state/.../logs/llm-io.jsonl`
- `.pytest_cache/...`
- `__pycache__/...`

These artifacts inflated edited-file counts and contributed to confusing `missing_repo_edits` summaries.

## Problem

Runtime/cache artifacts are being counted as changed files in user-visible telemetry and progress judgement. This makes it harder to distinguish real implementation progress from internal or verifier side effects.

## Desired Direction

- Separate user repo edits from runtime state and verification cache artifacts.
- Keep raw artifact visibility in debug logs if useful.
- Use filtered repo-edit telemetry for `missing_repo_edits`, summaries, and evaluation metrics.

## Acceptance Criteria

- `.anvil-state`, `.pytest_cache`, `__pycache__`, and similar verifier/runtime artifacts do not count as user implementation edits.
- User-visible turn summaries report only relevant repo files, such as `calculator.py`.
- Structured logs may include both raw changed files and filtered changed files.
- Issue 445 scenarios no longer report `edited 8 files` when the only real implementation edit is one Python file.
