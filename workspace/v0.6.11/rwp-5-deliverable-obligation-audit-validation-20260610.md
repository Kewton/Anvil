# RWP-5 DeliverableObligation Admission Audit Validation

Date: 2026-06-10

## Scope

RWP-5 adds a read-only audit projection for deliverable obligations so non-coding leakage can be inspected without moving completion authority or adding benchmark-specific rules.

Implemented:

- `deliverable_obligation_audit` module.
- Audit entries for required artifact identities and role-only required artifacts.
- Per-entry source, authority, task kind, role, kind, path, and fresh-repo-edit pressure.
- `post_tool_reconciliation` logging now includes `deliverable_obligations`.
- Existing fresh-edit predicate remains the single implementation; it is reused by the audit projection instead of being duplicated.

No completion behavior was intentionally changed.

## Deterministic Verification

Commands:

- `cargo test --lib deliverable_obligation_audit -- --nocapture`
- `cargo test --lib post_tool_reconciliation -- --nocapture`
- `cargo build`
- `cargo fmt --check`
- `git diff --check`

Result:

- `deliverable_obligation_audit`: 5/5 passed.
- `post_tool_reconciliation`: 3/3 passed.
- Build and formatting checks passed.

Covered assertions:

- Research report requires a report artifact, not a source edit.
- Ops command report requires a command observation/report artifact, not a source edit.
- Data output requires a data artifact/schema-oriented obligation, not source/test edits.
- Coding feature work still carries fresh source/test edit pressure.
- Verification-only coding does not require a fresh edit.

## Real LLM Validation

Run:

- `workspace/v0.6.11/eval-runs/rwp5-deliverable-obligation-audit-smoke-20260610/results.csv`

Case sequence:

- `research_cache` x2
- `ops_health` x2
- `data_csv` x1
- `data_json` x1
- `feature_discount` x2

Result:

- Overall: 6/8 pass, 6/8 high_quality.
- Research: 2/2 high_quality, no unexpected source/test edits.
- Data: 2/2 high_quality, no unexpected source/test edits.
- Ops: 2/2 high_quality artifacts, but both ended at `max_iterations`; shadow terminal remained `not_observed`.
- Feature improvement: 0/2, both `missing_repo_edits`; false-done remained 0.

## Interpretation

The RWP-5 audit boundary is useful for observability and does not introduce new leakage in the smoke set. It should not be treated as a success-rate improvement by itself because it is projection-only.

Residual issues:

- Ops can produce an acceptable report artifact but still fail to converge terminally (`max_iterations` with shadow `not_observed`).
- Feature improvement still has a current-turn edit/action adherence problem and can stop at `missing_repo_edits`.
- These are not solved by obligation audit and should flow into typed action admissibility and terminal/evidence adoption work rather than adding more task-kind string rules.

## Complexity Assessment

The new module is read-only and typed. It uses `ArtifactRole`, `DeliverableKind`, `TaskKind`, and `ObjectiveAuthority` rather than raw prompt text. The only existing method visibility change exposes the current fresh-edit predicate to avoid duplicated logic.

RWP-5 is complete as an audit slice.
