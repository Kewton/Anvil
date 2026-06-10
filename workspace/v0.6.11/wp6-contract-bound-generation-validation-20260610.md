# WP6 Contract-Bound Generation Validation

Date: 2026-06-10

## Scope

WP6 introduced the smallest contract-bound generation slice for hard coding tasks:

- add an explicit `failure_taxonomy` field to the Contract-Bound Generation packet;
- keep taxonomy generic, not benchmark-specific;
- block a newly observed false-done path where an existing test suite passed with zero repository edits.

This is not a success-rate improvement claim. It is an observability and safety slice.

## Code Changes

- `src/agent/loop_run/contract_bound_generation.rs`
  - includes `failure_taxonomy` in the model-visible contract-bound generation policy;
  - maps coding source+test+evidence tasks to:
    `missing_deliverable|missing_evidence|evidence_runner_binding|evidence_failed|authoring_style_mismatch|contract_expectation_drift`;
  - keeps non-coding and no-evidence paths on the same generic taxonomy with fewer labels.
- `src/agent/loop_run/task_contract.rs`
  - refuses `Done` for coding change contracts when verifier evidence exists but no fresh repo edit evidence was recorded this turn.
- `src/agent/loop_run/verifier_orchestration.rs`
  - re-checks the TaskContract after structured verifier pass and converts fresh-edit-less coding success into `missing_repo_edits`.

## Deterministic Validation

Passed:

- `cargo fmt --check`
- `cargo test --lib contract_bound_generation`
- `cargo test --lib coding_change_request_with`
- `cargo test --lib verifier_pass_guard`
- `cargo test --lib task_contract` with sandbox escalation because mockito needs a local server
- `cargo build`

## Real LLM Validation

Model: `qwen3.6:27b-coding-nvfp4`

### Hard Python merge config

Workdir: `/private/tmp/anvil-wp6-smoke/hard-python-1`

Result: `repair_exhausted`.

Observation:

- generated implementation and tests;
- verifier ran;
- repair did not converge;
- manual pytest showed 7 failures and 3 passes;
- core defects were eager default evaluation in `dict.get`, missing global-key handling, and trailing newline mismatch;
- repair proposal was repeatedly rejected with `ambiguous_authority`.

Conclusion: WP6 taxonomy made the failure observable, but did not solve repair convergence.

### TDD Python median

Workdir: `/private/tmp/anvil-wp6-smoke/tdd-python-1`

Result: `done`, 3 tests passed.

Observation:

- contract-bound packet included `failure_taxonomy`;
- model produced implementation and pytest tests;
- verifier completed successfully.

Known gap:

- the first generated file order did not strictly follow the user-requested TDD order in the first attempt path. Final artifacts were valid.

### TOML merge

Workdir: `/private/tmp/anvil-wp6-smoke/toml-1`

Result: `done`.

Observation:

- no TOML-special branch was needed;
- generated Python implementation/tests passed with `python3 -m pytest -q -p no:cacheprovider`;
- contract-bound packet included the same generic taxonomy.

### Rust word counter

Workdir: `/private/tmp/anvil-wp6-smoke/rust-word-1`

Anvil result: `safe_stop_verifier_missing`.

Manual postcheck: `cargo test -q` passed.

Observation:

- generated `Cargo.toml`, `src/lib.rs`, and `tests/lib.rs`;
- internal generated-test preflight rejected the integration test as `unsupported_contract_assertion`;
- verifier selection became `structured_missing` even though the project was runnable.

Conclusion: verifier/test-artifact binding remains too narrow for Rust integration tests.

### Node JSON formatter

Workdir: `/private/tmp/anvil-wp6-smoke/node-json-1`

Anvil result: `missing_repo_edits`.

Manual postcheck: `node tests/format_json.test.js` passed.

Observation:

- generated implementation and runnable Node test;
- artifact completion budget exhausted on test shape recognition;
- internal outcome did not run the verifier.

Conclusion: non-Python runnable test recognition still needs a typed schema/runner boundary, not more ad hoc test-shape patterns.

### Feature Improvement, Existing Python Project

Workdirs:

- before guard: `/private/tmp/anvil-wp6-smoke/feature-python-2` and `feature-python-3`;
- after guard: `/private/tmp/anvil-wp6-smoke/feature-python-4`.

Before guard:

- Anvil returned `done` with `edited 0 files`;
- existing tests passed, but requested `multiply(a, b)` was not implemented.

After guard:

- Anvil returned `missing_repo_edits`;
- edited files remained 0;
- false-done was blocked.

Conclusion: WP6 now fails closed when a coding change request is verified without fresh edit evidence.

## Assessment

WP6 improves safety and observability:

- hard task failures now expose a generic failure taxonomy in the generation packet;
- a concrete false-done path is blocked;
- no benchmark-specific branch was added.

WP6 does not yet improve hard-task success rate:

- repair convergence still depends on downstream RepairTargetDecision quality;
- Rust/Node verifier binding still rejects runnable tests;
- feature-improvement tasks can still fail with `missing_repo_edits` rather than recovering into the correct implementation edit.

These are carried into WP7 and later.
