# v0.6.8 Evidence Scope / Repair Target Validation

Date: 2026-06-08

## Commit Baseline

Committed before this continuation:

- `9e46cb1 Improve completion evidence reconciliation`

That commit included:

- terminal completion credit reconciliation
- Python stdlib pytest full-suite verifier scope
- v0.6.8 direction and validation notes

## Minimal Implementation In This Continuation

### 1. Diagnostic evidence scope packet

Added a small `VerifierEvidenceScopePacket` to the verifier diagnostic payload.

It records:

- `kind`: `project_suite`, `artifact_filtered`, or `unknown`
- whether the command is a completion verifier command
- whether the command directly references a changed artifact path
- changed candidate counts
- changed test candidate count
- verifier-reported failure location path/role when available

This is structural verifier metadata, not completion authority. It does not
decide `done`, and it does not force a repair target.

### 2. Output-named failure artifacts enter diagnostic excerpts

Actual validation showed that `RepairJob.target_hint` could point to a changed
implementation artifact even when the verifier output clearly named a failing
existing test artifact.

The diagnostic excerpt builder now:

- extracts safe path-like tokens from verifier output
- admits only existing workspace artifacts
- includes those artifacts before changed-file candidates

This keeps the mechanism generic. It does not special-case Python or a benchmark
task. It only says: if the verifier output names a safe existing artifact, the
diagnostic LLM should be able to inspect it.

### 3. Diagnostic prompt authority-conflict guidance

The diagnostic prompt now tells the LLM:

- `evidence_scope` is controller-computed verifier-scope metadata
- `project_suite` failures are project-level evidence failures
- `artifact_filtered` failures are narrower artifact evidence failures
- if a project-suite failure points at a test artifact whose assertion conflicts
  with an explicit higher-authority user request or behavior contract, classify
  it as `test_bug` and target the test artifact while preserving coverage

This is an LLM semantic judgment point. The controller supplies structured
evidence; the LLM decides whether there is an authority conflict.

## Unit Validation

Passed:

- `cargo test evidence_scope --lib`
- `cargo test verifier_diagnostic_payload --lib`
- `cargo build --release`

The targeted tests pin:

- full pytest command without changed path reference -> `project_suite`
- pytest command filtered to a changed test path -> `artifact_filtered`
- verifier output path `tests/test_sales.py` becomes
  `evidence_scope.failure_location_path`
- diagnostic payload includes `evidence_scope`

## Actual Local LLM Validation

Model configuration:

- main: `qwen3.6:27b-coding-nvfp4`
- sidecar: `qwen3.5:9b`
- command family: `anvildev -m ... --sidecar-model ... -y --fresh-session --no-footer --deterministic-fallback full-template`

### Feature Improvement: Python Existing Test Suite

Fixture:

- `sales.py` initially returns `{"total": sum(items)}`
- existing `tests/test_sales.py` asserts exact old return shape
- request asks to return both `total` and `average`, update tests as needed,
  and pass full pytest

Run 1:

- terminal: `repair_exhausted`
- final verifier: full pytest failed on existing `tests/test_sales.py`
- result: no false-done, but repair repeatedly targeted `sales.py`

Run 2:

- after authority-conflict prompt guidance
- terminal: `repair_exhausted`
- final verifier: same existing-test failure
- result: prompt-only guidance was not enough

Unit inspection after Run 2:

- `evidence_scope.failure_location_path` was still `sales.py`
- `safe_file_excerpts` did not include `tests/test_sales.py`
- root cause: output-named failing artifact was not entering the structured
  diagnostic candidate set

Run 3:

- after output-named failure artifacts were added to diagnostic excerpts
- terminal: `missing_repo_edits`
- edited files: none
- result: this run did not reach verifier repair, so it does not prove repair
  convergence; it exposed a separate early artifact role-policy stop

Interpretation:

The direction is correct, but not sufficient yet. The system must not rely on
prompt wording alone. The controller needs to provide the LLM with complete,
typed failure artifact candidates. Once those candidates are present, the next
validation must confirm that the diagnostic worker actually selects the failing
test artifact when higher-authority objective evidence contradicts an old exact
assertion.

### TDD: Rust Slug Library

Prompt:

- create Rust library crate
- write tests first
- implement slugify
- run `cargo test`

Result:

- terminal: `done`
- independent `cargo test`: passed
- test result: 6 integration tests passed

Interpretation:

The new diagnostic evidence-scope plumbing did not regress a hard TDD coding
task. This is important because the architecture must remain coding-capable
while becoming more general.

## Architecture Insight

The next repair milestone should not be another prompt-only iteration.

The actual sequence showed three layers:

1. **Evidence scope**: project-suite failure must be distinguished from a
   generated-test-only failure.
2. **Failure artifact candidate**: the verifier-reported failing artifact must
   enter typed candidate/excerpt data even if it was not edited this turn.
3. **Authority conflict judgment**: the LLM can then decide whether the failing
   artifact is stale/contradictory or whether implementation behavior is wrong.

Layer 1 and 2 are controller responsibilities. Layer 3 is LLM semantic judgment.

This keeps the design maintainable:

- no provider abstraction
- no benchmark-specific branches
- no Python-only repair exception
- no terminal success relaxation
- no direct controller override of the LLM repair target

## Revised Next Steps

1. Promote verifier-output failure artifacts into `FailurePacket.candidate_artifacts`
   as typed candidates, not only `safe_file_excerpts`.
2. Add diagnostic acceptance tests proving that a selected test artifact is
   admitted when it is present in verifier output and conflicts with explicit
   objective evidence.
3. Re-run the Python feature-improvement fixture after that change.
4. Investigate the separate `missing_repo_edits` early stop where the model read
   `tests/test_sales.py` but made no edit.
5. Continue hard-task validation with Rust TDD, feature improvement, Node setup,
   data, docs, and non-coding evidence tasks.
