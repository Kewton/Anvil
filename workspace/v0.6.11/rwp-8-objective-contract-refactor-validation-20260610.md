# RWP-8 ObjectiveContract Projection Refactor Validation

Date: 2026-06-10

## Scope

RWP-8 reduced `task_contract.rs` complexity by moving the read-only
`ObjectiveContract` projection into `src/agent/loop_run/objective_contract_projection.rs`.

This is intentionally a no-behavior-change slice:

- no completion authority changes
- no terminal projection changes
- no evidence runner changes
- no new benchmark-specific rule

## Code Change

- Added `objective_contract_projection` module.
- Moved `ObjectiveAuthority`, `ObjectiveAuxiliaryContext`, `ObjectiveContract`,
  and command-evidence projection helpers out of `task_contract.rs`.
- Kept `TaskContract::objective_contract()` as the existing call site boundary.
- Re-exported only the projection types needed by existing in-crate callers.

Complexity effect:

- `task_contract.rs`: 8116 lines before the slice, 7967 lines after the slice.
- New module: 230 lines, including focused tests.

## Deterministic Verification

Commands:

```text
cargo test --lib objective_contract_projection -- --nocapture
cargo test --lib objective_contract -- --nocapture
cargo build
cargo fmt --check
git diff --check
```

Result:

- `objective_contract_projection`: 3 passed / 0 failed
- `objective_contract`: 10 passed / 0 failed
- build/fmt/diff checks passed

## Real LLM Smoke

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --case-sequence docs_runbook,data_json,python_sales --variant no_pam --run-id rwp8-objective-contract-refactor-smoke-20260610 --anvil-bin target/debug/anvil --timeout-secs 480 --chat-timeout-secs 180
```

Result:

- pass: 3/3
- high_quality: 3/3
- false_done: 0/3
- false_missing: 0/3
- repair_exhausted: 0/3
- max_iterations: 0/3

Case details:

| Case | Kind | Result | Terminal |
| --- | --- | --- | --- |
| docs_runbook | docs | pass / high_quality | done |
| data_json | data | pass / high_quality | done |
| python_sales | coding | pass / high_quality | done |

## Assessment

The slice is safe to keep. It moves objective projection out of the largest
contract module without adding pattern matching, changing terminal policy, or
coupling projection to benchmark cases.

The remaining complexity risk is still `task_contract.rs`, but the next safe
step should continue extracting read-only projection boundaries rather than
adding new recovery or terminal rules.
