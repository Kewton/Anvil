# P31: Explicit Test Identity Preflight Validation

Date: 2026-06-07
Branch: develop

## Goal

Fix the false negative found in P30/P31 local LLM validation:

- Anvil created a valid Rust project with `Cargo.toml`, `src/lib.rs`, and `tests/add.rs`.
- Manual `cargo test --manifest-path Cargo.toml` passed.
- The controller still returned `safe_stop_verifier_missing` because `generated_test_preflight` rejected the owned test as `missing_contract_coverage`.

## Root Cause

`generated_test_guard::generated_suite_contract_coverage_gap` used static contract-coverage needles to decide whether generated tests should be admitted to the verifier.

For the request:

> Create Cargo.toml, src/lib.rs with `add(a: i32, b: i32) -> i32`, and tests/add.rs tests.

the explicit test deliverable path `tests/add.rs` was satisfied, and the generated test did exercise `add`. However, generic terms such as `add`, `create`, `library`, `cargo`, and `manifest` are intentionally filtered from coverage needles because they are often operation/setup words.

That made a passing, explicitly requested test artifact look uncovered.

## Change

Keep the generated-test guard, but narrow the coverage gate:

- Explicit I/O requirements such as `stdin`, `stdout`, and `stderr` are still enforced.
- General static coverage is skipped when all explicit required test artifact identities are present in the admitted test source set.

This keeps the guard useful for unsafe or unrelated generated tests, while preventing it from blocking verifier execution for a user-requested test path that the evidence runner can validate directly.

## Unit Validation

Passed:

- `cargo test --offline --lib report_accepts_explicit_test_identity_even_when_generic_terms_are_filtered`
- `cargo test --offline --lib report_rejects_generated_suite_missing_stdin_coverage`
- `cargo test --offline --lib report_rejects_generated_suite_without_contract_coverage`
- `cargo test --offline --lib`
  - Result: `3830 passed; 0 failed`

The new regression test covers the false-negative shape:

- required explicit test identity: `tests/add.rs`
- generated Rust test imports and asserts `add`
- coverage needles would otherwise be too generic
- preflight now admits the test

The existing protection still rejects:

- unrelated test suites
- missing explicit stdin coverage

## Actual Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Workspace:

- `/private/tmp/anvil-p32-coding-work`

Command shape:

- `target/debug/anvil --oneshot --fresh-session --no-footer --trace -y -m qwen3.6:27b-coding-mxfp8 --state-dir /private/tmp/anvil-p32-coding-state --cwd /private/tmp/anvil-p32-coding-work --max-iterations 12 --chat-timeout-secs 180 -p "..."`

Task:

- Create a Rust library.
- Write `Cargo.toml`.
- Implement `src/lib.rs` with `add(a: i32, b: i32) -> i32`.
- Write `tests/add.rs`.
- Pass `cargo test --manifest-path Cargo.toml`.

Result:

- Terminal: `done`
- Iterations: 6/12
- Edited files: `Cargo.lock`, `Cargo.toml`, `src/lib.rs`, `tests/add.rs`
- Manual verifier:
  - `cargo test --offline --manifest-path /private/tmp/anvil-p32-coding-work/Cargo.toml`
  - Result: 1 integration test passed.

Comparison:

- Before this change, the same task shape stopped at `safe_stop_verifier_missing` even though manual cargo test passed.
- After this change, the controller ran the verifier and completed successfully.

## Architectural Impact

This is a small but important move toward the target architecture:

- The evidence runner is now less likely to be blocked by a brittle static test-content heuristic.
- Explicit ObjectiveContract deliverables have authority over generic coverage words.
- The guard remains extensible because explicit I/O coverage and generated-test bug checks still run before the skip.

General-purpose implication:

- For docs, data, shell, and research tasks, explicit deliverable identity should similarly prevent weak static heuristics from blocking evidence execution.
- Evidence should be run when the requested deliverable exists and the runner can validate it; heuristic quality gates should become repairable warnings unless they identify a concrete safety/test-bug issue.

## Remaining Risks

- This does not solve malformed tool-call convergence in TDD runs.
- This does not eliminate extra artifact retry loops after model-run verification.
- It only resolves the false negative where explicit test identity is satisfied but generic coverage terms are filtered.
