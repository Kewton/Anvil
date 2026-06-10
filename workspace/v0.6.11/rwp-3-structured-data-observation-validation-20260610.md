# RWP-3 StructuredDataObservation Validation

Date: 2026-06-10

## Scope

RWP-3 introduced a typed `StructuredDataObservation` boundary for structured data schema checks.

The change intentionally does not add benchmark-specific rules. Existing CSV/TSV/JSON/JSONL parser helpers moved out of `verifier.rs` and into `structured_data_observation.rs`, while `verifier.rs` now projects the typed observation back into the existing `VerifierDiagnostic` messages.

## Implementation Summary

- Added `src/agent/loop_run/structured_data_observation.rs`.
- Added `StructuredDataObservation` with:
  - observed columns
  - missing columns
  - extra columns
  - expected rows
  - observed rows
  - typed failure kind
- Kept existing diagnostic messages stable by projecting typed failures to the old text.
- Preserved the existing data artifact accept-tier path so parse-ready data artifacts do not regress into false-missing.
- Moved structured data parser helpers out of `verifier.rs`.
- Kept `task_contract.rs` untouched.

Complexity impact:

- `src/agent/loop_run/verifier.rs`: `24 insertions / 343 deletions`
- `src/agent/loop_run/structured_data_observation.rs`: new focused parser/observation module
- `src/session/eval_log.rs`: format-only cleanup from prior RWP shadow-terminal edits

## Deterministic Verification

Commands:

```text
cargo test --lib structured_data_observation -- --nocapture
cargo test --lib data_schema -- --nocapture
cargo test --lib data_task_exact_columns_blocks_extra_columns_without_literal_rows -- --nocapture
cargo test --lib data_task_does_not_treat_row_count_word_as_column -- --nocapture
cargo test --lib data_schema_mismatch_is_not_ready_just_because_path_exists -- --nocapture
cargo build
cargo fmt --check
git diff --check
```

Results:

- `structured_data_observation`: 5 passed
- `data_schema`: 7 passed
- exact column policy regression: passed
- row-count wording regression: passed
- schema-mismatch path-only regression: passed
- `cargo build`: passed
- `cargo fmt --check`: passed
- `git diff --check`: passed

## Real LLM Verification

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence data_csv,data_csv,data_csv,data_csv,data_csv,data_csv,data_csv,data_csv,data_json,data_json,data_json,data_json,docs_runbook,docs_runbook,python_sales,python_sales \
  --variant no_pam \
  --run-id rwp3-structured-data-observation-smoke-20260610 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

Summary:

- total: 16
- pass: 16/16
- high_quality: 16/16
- verification_pass: 16/16
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- shadow_conflict: 0

By case:

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| data_csv | 8 | 8 | 8 |
| data_json | 4 | 4 | 4 |
| docs_runbook | 2 | 2 | 2 |
| python_sales | 2 | 2 | 2 |

All rows ended with:

- `exit_reason=done`
- `shadow_terminal_class=success`
- `shadow_terminal_conflict=false`

Artifacts:

- `workspace/v0.6.11/eval-runs/rwp3-structured-data-observation-smoke-20260610/results.csv`
- `workspace/v0.6.11/eval-runs/rwp3-structured-data-observation-smoke-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/rwp3-structured-data-observation-smoke-20260610/summary.json`

## Interpretation

The RWP-3 hypothesis is supported at small scale:

- Structured data schema checking can be represented as typed observation without adding benchmark-specific pattern rules.
- Existing strict schema behavior remained stable.
- Data CSV/JSON did not regress.
- Docs/coding smoke did not regress.
- Shadow terminal stayed aligned.

This does not yet justify a success-rate improvement claim. It is a regression guard and architecture-safety result. A 20-run mixed guard is still required before calling it stable, and a 50-run comparison is required before claiming a meaningful success-rate gain.

## Known Limits

- `StructuredDataObservation` is still projected only into verifier diagnostics. It is not yet persisted as first-class evidence in eval logs.
- Repair still receives mostly diagnostic prose, not a structured schema delta.
- Data observation currently covers CSV/TSV/JSON/JSONL. Other structured formats must be added through typed parser boundaries, not request-string pattern rules.
- This WP did not address API behavior observation or repair convergence.
