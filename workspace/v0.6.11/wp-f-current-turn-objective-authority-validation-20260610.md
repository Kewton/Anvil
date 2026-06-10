# WP-F: Current-Turn ObjectiveContract Authority Validation

Date: 2026-06-10

## Scope

WP-F reduces stale session/working-memory contamination by making the current user request explicit in the ObjectiveContract authority boundary.

Implemented changes:

- `ObjectiveContract` now carries `authority=current_user_request` and `auxiliary_context=session_context`.
- The ObjectiveContract prompt includes the same authority statement so the model sees the current request as the sealed contract source.
- Semantic candidate shadow logs include `contract_authority` and `auxiliary_context`.
- `repo_change_request_text` now prefers the latest explicit user request over stale `working_memory.active_task`, while preserving plan approval recovery.

## Deterministic Validation

Commands:

- `cargo test --lib objective_contract -- --nocapture`
  - Result: 7 passed.
- `cargo test --lib repo_change_request_text -- --nocapture`
  - Result: 3 passed.
- `cargo test --lib task_classification -- --nocapture`
  - Result: 0 matched tests, compile path passed.
- `cargo build`
  - Result: passed.
- `python3 -m py_compile workspace/v0.6.11/wp_f_turn_authority_eval.py`
  - Result: passed.
- `git diff --check`
  - Result: passed.

## Real LLM Validation

Command:

```sh
python3 workspace/v0.6.11/wp_f_turn_authority_eval.py \
  --run-id wp-f-current-turn-authority-smoke3-20260610 \
  --timeout-secs 360 \
  --chat-timeout-secs 180
```

The first sandboxed attempt failed before model invocation because `127.0.0.1:11434` was blocked. The accepted run was executed with local Ollama access.

Results:

- total: 4
- pass: 4/4
- high_quality: 4/4
- verification_pass: 4/4

Cases:

| case | turn kinds | turn-2 changed files | result |
| --- | --- | --- | --- |
| `coding_to_data` | coding -> data | `output/summary.json` | pass |
| `coding_to_docs` | coding -> docs | `docs/runbook.md` | pass |
| `docs_to_coding` | docs -> coding | `src/slugify.py`, `tests/test_slugify.py`, pytest cache | pass |
| `data_to_tdd` | data -> coding/TDD | `src/stats.py`, `tests/test_stats.py`, pytest cache | pass |

The adopted raw results are under:

- `workspace/v0.6.11/eval-runs/wp-f-current-turn-authority-smoke3-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/wp-f-current-turn-authority-smoke3-20260610/summary.json`
- `workspace/v0.6.11/eval-runs/wp-f-current-turn-authority-smoke3-20260610/results.csv`

## Interpretation

This validates the narrow WP-F hypothesis: in continued-session runs, a stale prior task did not override a new data/docs/coding/TDD request. The second turn projected the current request into the ObjectiveContract and wrote the expected turn-2 artifact.

This is not a broad success-rate claim. It is a focused regression guard for current-turn authority and session-contamination behavior.

## Known Issues

- `docs_to_coding` externally passed but both turns reported `repair_safe_stop`. Terminal projection can still disagree with external artifact/evidence success.
- The first turn in `coding_to_data` asked for `package.json` plus `src/add.js`, but the contract required only `src/add.js`. This smoke uses the first turn only to create stale coding context; it should not be treated as coding deliverable completeness evidence.
- Local LLM validation requires unsandboxed localhost Ollama access.
