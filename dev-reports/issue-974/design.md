# Issue 974 Design Note

## Scope

Issue #974 is a parent issue for the generic Objective/Evidence/Recovery lifecycle. The current tree already contains partial generic lifecycle vocabulary: task kinds, objective deliverable/evidence projections, generic terminal states, recovery job labels, and an `EvidenceRunner` adapter.

This issue slice tightens the replay/report classifier around one acceptance criterion:

- do not classify a run as `missing_evidence` when the replay record says an evidence runner was bound and the rerun failed.

## Approach

Keep legacy terminal labels intact for compatibility, but derive the generic terminal state from structured worker lifecycle observations when they prove the old label is too weak. In particular:

- `final_outcome=missing_evidence` or `missing_verification` remains the `legacy_terminal_state`.
- If `worker_lifecycle.runner_bound=true` and either `evidence_created=true` or `rerun_passed=false`, report `generic_terminal_state=evidence_failed`.
- Recovery projection then naturally becomes `EvidenceFailedJob`.
- Missing tests/evidence with `runner_bound=false` remain `missing_evidence` / `MissingEvidenceJob`.

This is intentionally report-side and deterministic. It does not add provider abstraction, does not introduce task-kind special cases, and preserves existing log compatibility.

## Verification Plan

- Add a focused regression in `tests/test_eval_task_kind.py` for a legacy `missing_evidence` record with bound runner failure.
- Run the focused Python reporting tests.
- Run Rust focused lifecycle tests touching `summary`, `task_contract`, and `evidence_runner`.
- Run broader formatting and clippy checks because terminal/recovery projection is shared evaluation vocabulary.
