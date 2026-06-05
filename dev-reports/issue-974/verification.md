# Issue 974 Verification

## Focused Verification

- `python3 -m unittest tests.test_eval_task_kind`
  - Passed: 17 tests.
- `cargo test -q evidence_runner`
  - Passed: 5 matching tests.
- `cargo test -q terminal_outcome`
  - Passed: 3 matching tests.
- `cargo test -q recovery_job_kind`
  - Passed: 2 matching tests.
- `cargo test -q objective_contract`
  - Passed: 2 matching tests.

## Broader Verification

- `cargo fmt --check`
  - Passed.
- `cargo clippy --all-targets -- -D warnings`
  - Passed.
- `cargo test -q`
  - Passed.
  - Required escalation for local socket binding used by existing test servers.

## Expected Behavior Covered

- A replay record with legacy `final_outcome=missing_evidence`, `runner_bound=true`, `evidence_created=true`, and `rerun_passed=false` now reports:
  - `legacy_terminal_state=missing_evidence`
  - `generic_terminal_state=evidence_failed`
  - `recovery_job_kind=EvidenceFailedJob`
  - `lifecycle_failure_stage=rerun`

## Known Non-Issues Observed

- A broad `cargo test -q summary` filter was not used as a final signal because it also matched unrelated tests with `summary` in their names. Specific terminal/recovery filters passed.
- The first sandboxed broad summary filter hit local socket restrictions in mockito tests; rerun with cargo-test escalation was able to bind local sockets.
