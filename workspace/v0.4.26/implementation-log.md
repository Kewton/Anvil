# v0.4.26 Implementation Log

## Completed Slices

### Slice 0: Architecture Invariant Tests First

Status: reviewed and partially already satisfied by existing arbiter tests.

Existing coverage confirmed:

- `loop_control_repair_job_wins_over_verifier_run_inside_arbiter_module`
- `loop_control_missing_verifier_wins_over_model_turn_inside_arbiter_module`
- `loop_control_plan_mode_never_dispatches_controller_jobs`
- `loop_control_stale_pending_flag_without_owner_requests_model_turn`
- `loop_control_transition_table_covers_controller_owned_states`
- `recovery_owner_gates_lower_level_fallbacks`

Interpretation:

- The `ActiveJobArbiter` layer already pins the most important owner-gate
  invariants.
- Additional invariant work is still useful later at the `TurnDriver` level,
  but the current arbiter tests were sufficient to begin Slice 1 safely.

### Slice 1: ToolExecutionOutcome Boundary

Implemented:

- `src/agent/loop_run/tool_execution.rs`
  - `ToolExecutionOutcome`
  - `RepoEditEvidence`
  - `ReadEvidence`
  - `CommandEvidence`
  - `ToolRejection`
  - `ToolFailure`
  - `NoopReason`
  - `success_outcome_for_call`
  - `rejected_outcome_for_call`
  - `failed_outcome_for_call`

Wired into:

- `src/agent/loop_run.rs`
  - private `tool_execution` module
- `src/agent/loop_run/turn.rs`
  - Write/Edit success path now consumes `ToolExecutionOutcome::RepoEdit`
    when updating touched-file and repo-edit evidence state.
  - policy rejection and execution failure paths now construct typed outcomes
    as a first boundary without changing user-visible behavior.

Tests added:

- read-only tool cannot satisfy artifact edit evidence
- workspace repo edit satisfies artifact evidence
- controller-owned `.anvil-state` edit remains a repo edit but does not become
  artifact evidence
- command output does not satisfy artifact edit evidence
- rejected / failed outcomes are typed

Current effect:

- `execute_tool_call` rough CC dropped from `31` to `28`.
- `tool_execution.rs` remains low complexity:
  - functions: `16`
  - max rough CC: `7`
  - rough CC >= 15: `0`

### Slice 2: VerifierDriver Initial Boundary

Implemented:

- `src/agent/loop_run/verifier_driver.rs`
  - `TaskContractVerifierOutcome`
  - `task_contract_auto_test_result_to_outcome`
  - `task_contract_verifier_transport_error_to_outcome`
  - `task_contract_structured_missing_outcome`
  - `classify_verifier_timeout`
  - `select_task_contract_project_unit`
  - `TaskContractVerifierSelection`
  - `select_task_contract_verifier`

Wired into:

- `src/agent/loop_run.rs`
  - private `verifier_driver` module
- `src/agent/loop_run/turn.rs`
  - verifier pass/fail/transport normalization now comes from
    `verifier_driver`.
  - project-unit selection for task-contract verifier is delegated to
    `verifier_driver`.
  - structured vs legacy verifier command selection is delegated to
    `verifier_driver`.

Tests added:

- auto-test pass/failure maps to typed verifier outcome
- verifier timeout transport error becomes repairable verifier failure
- non-timeout transport error remains transport error
- structured missing distinguishes no owned tests from unbound runner
- project-unit selection requires a workspace scope
- verifier selection returns structured missing when tests are required but no
  owned test artifact exists
- verifier selection returns missing when no runnable verifier candidate exists

Current effect:

- `run_task_contract_verifier_once` rough CC dropped from `38` to `34`.
- `verifier_driver.rs` stays below the high-complexity threshold:
  - functions: `21`
  - max rough CC: `7`
  - rough CC >= 15: `0`

## Verification

Commands run:

```bash
cargo fmt --check
cargo test tool_execution --lib -q
cargo test recovery_owner_gates_lower_level_fallbacks --lib -q
cargo test model_request --lib -q
cargo test verifier_driver --lib -q
python3 -m unittest tests/test_complexity_report.py
cargo clippy --all-targets -- -D warnings
cargo test --lib -q
cargo build --release
python3 scripts/complexity_report.py --top 12 \
  src/agent/loop_run/turn.rs \
  src/agent/loop_run/model_request.rs \
  src/agent/loop_run/tool_execution.rs \
  src/agent/loop_run/verifier_driver.rs \
  src/agent/loop_run/repair_job.rs
```

Results:

- targeted Rust tests passed
- Python complexity-report tests passed
- `cargo clippy --all-targets -- -D warnings` passed
- `cargo test --lib -q` passed: `3070 passed`
- `cargo build --release` passed

## Current Complexity Snapshot

| File | Functions | Avg Rough CC | Max Rough CC | CC >= 15 | CC >= 50 | Function LOC |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `src/agent/loop_run/turn.rs` | 1032 | 3.01 | 362 | 29 | 1 | 33115 |
| `src/agent/loop_run/repair_job.rs` | 236 | 2.37 | 26 | 3 | 0 | 5806 |
| `src/agent/loop_run/model_request.rs` | 9 | 3.11 | 8 | 0 | 0 | 153 |
| `src/agent/loop_run/tool_execution.rs` | 16 | 1.62 | 7 | 0 | 0 | 143 |
| `src/agent/loop_run/verifier_driver.rs` | 21 | 2.24 | 7 | 0 | 0 | 288 |

Top remaining hotspots:

| Rough CC | Function |
| ---: | --- |
| 362 | `turn.rs::run_actor_loop` |
| 42 | `turn.rs::build_request_messages` |
| 40 | `turn.rs::request_assistant_reply_with_retry` |
| 34 | `turn.rs::run_task_contract_verifier_once` |
| 30 | `turn.rs::run_verifier_repair_pass_and_apply` |
| 28 | `turn.rs::execute_tool_call` |

## Remaining Work

Next recommended slice:

1. Continue Slice 2 by moving verifier command selection/execution into
   `VerifierDriver` behind typed reports.
2. Continue Slice 1 opportunistically until `execute_tool_call` becomes
   dispatch plus typed outcome conversion only.
3. Defer `TurnDriver` until `VerifierDriver`, `RepairDriver`, and
   `AssistantRequestDriver` expose typed boundaries.

Important remaining structural issue:

- The dominant bottleneck is unchanged: `run_actor_loop` still owns final
  orchestration decisions. The current work lowers local tool execution
  complexity, but does not yet move final decision ownership out of `turn.rs`.
