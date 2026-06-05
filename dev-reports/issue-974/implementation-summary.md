# Issue 974 Implementation Summary

## Summary

Implemented a deterministic replay/report normalization for the Objective/Evidence/Recovery lifecycle:

- `scripts/analyze_run.py` now refines `generic_terminal_state` from worker lifecycle observations.
- A legacy `missing_evidence` / `missing_verification` outcome remains visible as `legacy_terminal_state`.
- If worker lifecycle data proves `runner_bound=true` and evidence was created or rerun failed, the generic projection becomes `evidence_failed`.
- When normalization changes the generic state, `recovery_job_kind` is recomputed from the normalized state so stale `MissingEvidenceJob` projections do not survive.

This keeps backward-compatible labels while satisfying the issue slice that runner-present failures must not be routed as missing evidence.

## Changed Files

- `scripts/analyze_run.py`
  - Added `_normalize_generic_terminal_state_from_worker_lifecycle`.
  - Applied the normalization before recovery job projection.
- `tests/test_eval_task_kind.py`
  - Added a regression for bound-runner compile failure logged with legacy `missing_evidence`.
- `dev-reports/issue-974/design.md`
  - Design note written before code edits.
- `dev-reports/issue-974/implementation-summary.md`
  - This summary.
- `dev-reports/issue-974/verification.md`
  - Verification record.

## Notes

No provider abstraction was added. No task-kind-specific branch was introduced; the normalization is keyed only on generic worker lifecycle observations.
