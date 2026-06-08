# v0.6.8 Failure Packet Candidate Validation

Date: 2026-06-08

## Scope

This continuation followed the previous evidence-scope validation.

Goal:

- keep moving toward typed objective/evidence control
- avoid task-specific repair exceptions
- verify with actual local LLM runs
- include hard coding tasks and non-coding tasks

## Minimal Implementation

### 1. Output-named artifacts in FailurePacket

Added `FailurePacket::from_repair_job_with_work_root(work_root, job)`.

The diagnostic payload now includes safe existing artifacts named by verifier
output in `failure_packet.candidate_artifacts`.

Example:

- verifier output names `tests/test_sales.py::test_total`
- `tests/test_sales.py` exists under the workspace
- candidate artifact becomes:
  - role: `test`
  - path: `tests/test_sales.py`
  - reason: `verifier output names this failure artifact`

The legacy `FailurePacket::from_repair_job(job)` remains available for callers
that do not have a workspace root. This keeps the change localized.

### 2. Output-named artifacts in diagnostic primary candidates

The diagnostic prompt's `changed_candidates` now also includes safe
verifier-output artifacts with:

- `candidate_kind: verifier_output_failure_artifact`

This makes the candidate visible to the short-lived diagnostic LLM without
forcing the controller to choose it.

### 3. Current target vs failure location metadata

`evidence_scope` now includes:

- current repair target path/role
- verifier failure location path/role
- whether they differ
- whether this is a post-repair rerun

The prompt tells the LLM to re-evaluate the failure-location artifact when a
post-repair rerun still fails and the failure location differs from the current
repair target.

This is still semantic delegation to the LLM. The controller provides typed
facts and admission boundaries; it does not hard-code the repair target.

## Unit Validation

Passed:

- `cargo test failure_packet_includes_verifier_output_named_existing_artifact --lib`
- `cargo test evidence_scope --lib`
- `cargo test verifier_diagnostic_payload --lib`
- `cargo build --release`

## Actual Local LLM Validation

Model configuration:

- main: `qwen3.6:27b-coding-nvfp4`
- sidecar: `qwen3.5:9b`

### Feature Improvement: Python Existing Test Suite

Fixture:

- `sales.py` initially returns `{"total": sum(items)}`
- existing `tests/test_sales.py` asserts exact old return shape
- request asks to return both `total` and `average`, update tests as needed,
  and pass full pytest

Result after the new candidate plumbing:

- terminal: `repair_exhausted`
- final failure: existing `tests/test_sales.py::test_total`
- edited: `sales.py`, `tests/test_main.py`
- independent pytest: failed

Interpretation:

The added candidates made the controller input more complete, but repair still
did not converge. The diagnostic/repair lifecycle keeps choosing or applying
implementation-target repairs even after project-suite evidence points to an
old test assertion that conflicts with the new objective.

This means the next step is not more candidate visibility. The next step is a
typed no-progress target reassessment lifecycle:

- repeated same-failure after same target repair
- verifier-output failure artifact differs from current target
- controller requests a diagnostic target switch or safe-stops with the target
  conflict, instead of continuing same-target repair until budget exhaustion

### TDD: Rust Slug Library

Prompt:

- create Rust library crate
- write tests first
- implement slugify
- run `cargo test`

Result:

- terminal: `done`
- independent `cargo test`: passed
- 5 integration tests passed

Interpretation:

The candidate plumbing did not regress a hard coding/TDD task. Diagnostic repair
successfully fixed a Rust implementation failure and then full `cargo test`
passed.

### Data Output: CSV File

Prompt:

- create `data/output.csv`
- columns `id,total`
- rows `1,100` and `2,250`
- do not create source code

Result:

- terminal: `done`
- edited: `data/output.csv`, `output.csv`
- independent check: failed

Observed files:

`data/output.csv`:

```text
id,total,rows,1,100,2,250
1,100,1,1,100,,
2,250,2,,2,250
```

`output.csv`:

```text
id,total,rows,1,100,2,250
1,100
2,250
```

Interpretation:

This is a separate P0 false-done for non-coding data tasks. The architecture
cannot treat "file exists" as sufficient data evidence. It needs typed data
evidence:

- requested output path must be exact
- unexpected sibling/root output files should not satisfy the request
- CSV header must match requested columns
- row count and requested rows must match when explicitly provided

This should be implemented as a data EvidenceRunner / schema packet, not as a
coding verifier or prompt-only rule.

## Architecture Update

The current architecture direction remains valid, but the priority ordering
should be adjusted:

1. P0: prevent non-coding false-done by adding typed data evidence checks.
2. P1: add repair no-progress target reassessment for coding feature-improvement
   tasks.
3. P2: continue improving completion credit only after evidence scope and
   typed data evidence are reliable.

The most important distinction from this validation:

- Coding repair convergence is now mainly a lifecycle/target-reassessment issue.
- Data false-done is an evidence-runner completeness issue.

These should not be solved by the same mechanism.
