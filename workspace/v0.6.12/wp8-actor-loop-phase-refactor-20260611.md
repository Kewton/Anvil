# WP8 Actor Loop Phase Decision Refactor

Date: 2026-06-11

## Intent

WP8 was a no-op refactor. The goal was to reduce `actor_loop_flow.rs` responsibility without changing terminal behavior, repair behavior, or log payloads.

## Implementation

Added:

- `src/agent/loop_run/actor_loop_phase_decision.rs`

Moved pure terminal-projection helpers out of `actor_loop_flow.rs`:

- `task_contract_safe_stop_clear_tag`
- `task_contract_verifier_safe_stop_mapping`
- `task_contract_continue_requires_tool_recovery`
- `task_contract_action_completion_exit`

Updated consumers:

- `actor_loop_flow.rs`
- `verifier_orchestration.rs`
- `turn_tests.rs`
- `truncate_tests.rs`

Responsibility sentence:

> `actor_loop_phase_decision` owns pure actor-loop phase projection decisions that map typed contract/safe-stop state to existing terminal reasons, tags, and completion-blocking decisions.

## Complexity Impact

Line counts after the change:

| File | Lines |
| --- | ---: |
| `src/agent/loop_run/actor_loop_flow.rs` | 5884 |
| `src/agent/loop_run/actor_loop_phase_decision.rs` | 105 |

The change removes match-heavy terminal projection from the actor-loop dispatcher and keeps behavior-compatible tests in the new module.

## Deterministic Verification

Commands:

```text
cargo fmt --check
cargo test --lib actor_loop_phase_decision -- --nocapture
cargo test --lib task_contract_verifier_safe_stop -- --nocapture
cargo test --lib task_contract_completion_gate -- --nocapture
cargo test --lib task_contract_continue_no_tool_requires_targeted_recovery -- --nocapture
git diff --check
cargo build
```

Results:

- `actor_loop_phase_decision`: 3/3 passed
- `task_contract_verifier_safe_stop`: 2/2 passed
- `task_contract_completion_gate`: 0 tests matched after test relocation; covered by `actor_loop_phase_decision`
- `task_contract_continue_no_tool_requires_targeted_recovery`: 1/1 passed
- `cargo build`: passed

## Real LLM Smoke

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_sales,docs_runbook,data_json \
  --variant no_pam \
  --run-id wp8-actor-loop-phase-refactor-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Case | Pass | HQ | Terminal | Notes |
| --- | --- | --- | --- | --- |
| python_sales | true | false | `repair_exhausted` | external grader pass true; verifier failed due an invented no-CSV expectation |
| docs_runbook | true | true | `done` | no regression |
| data_json | true | true | `done` | no regression |

Overall:

- pass: 3/3
- high_quality: 2/3
- false_done: 0
- false_missing: 0
- repair_exhausted: 1
- max_iterations: 0

## Assessment

The refactor is behavior-compatible. The one non-HQ result matches an existing coding repair/test expectation drift signature, not a new terminal projection signature:

- terminal reason remained `repair_exhausted`
- false-done stayed 0
- docs/data remained stable

This WP does not claim success-rate improvement. It improves maintainability by isolating a small pure decision surface that future terminal/repair work can extend without adding more branches inside the actor-loop dispatcher.

## Carried-Forward Issues

- Python coding can still invent unsupported negative-path test expectations and end `repair_exhausted`.
- Terminal/evidence convergence remains the next precision bottleneck for coding tasks.
