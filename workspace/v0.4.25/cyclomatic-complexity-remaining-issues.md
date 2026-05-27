# Cyclomatic Complexity Remaining Issues

This note records the remaining work identified from the current
cyclomatic-complexity review. The measurements are approximate: no dedicated
Rust cyclomatic-complexity tool was installed, so the review used a read-only
function-level scan that counted branching constructs such as `if`, `match`,
loops, boolean operators, match arms, and `?`. The numbers are not compiler
grade, but they are useful for locating structural risk.

## Summary

The recent cleanup reduced surrounding complexity by moving progress text,
recent tool history, and malformed repair attempt handling out of `turn.rs`.
However, the main complexity problem is not fully solved. Risk is still
concentrated in a small number of large orchestration functions, especially in
`src/agent/loop_run/turn.rs`.

The current state is improving, but still structurally high risk:

- `turn.rs` remains a god orchestrator.
- `run_actor_loop` still owns too many control-flow phases.
- verifier, repair, model request, tool execution, artifact completion, and
  terminalization are not sufficiently separated.
- extracted helper modules are healthier, but they do not yet remove the main
  dispatch complexity.
- average function complexity is misleading because many small helper and test
  functions dilute a few very complex hotspots.

## Current Complexity Hotspots

Approximate module-level results:

| File | Functions | Avg Rough CC | Max Rough CC | Rough CC >= 15 | Rough CC >= 30 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `src/agent/loop_run/turn.rs` | 1042 | 3.0 | 362 | 30 | 6 |
| `src/agent/loop_run/repair_job.rs` | 236 | 2.4 | 26 | 3 | 0 |
| `src/agent/loop_run/active_job_arbiter.rs` | 79 | 2.0 | 11 | 0 | 0 |
| `src/agent/loop_run/repair_patch_validation.rs` | 118 | 3.1 | 15 | 1 | 0 |
| `src/agent/loop_run/tool_policy.rs` | 26 | 3.5 | 14 | 0 | 0 |
| `src/agent/loop_run/tool_history.rs` | 23 | 3.1 | 11 | 0 | 0 |
| `src/agent/loop_run/progress_text.rs` | 25 | 2.4 | 9 | 0 | 0 |

Highest-risk functions:

| Rough CC | Approx LOC | Function |
| ---: | ---: | --- |
| 362 | 2836 | `turn.rs::run_actor_loop` |
| 42 | 300 | `turn.rs::build_request_messages` |
| 40 | 197 | `turn.rs::request_assistant_reply_with_retry` |
| 38 | 459 | `turn.rs::run_task_contract_verifier_once` |
| 31 | 255 | `turn.rs::execute_tool_call` |
| 30 | 320 | `turn.rs::run_verifier_repair_pass_and_apply` |
| 28 | 42 | `turn.rs::answer_only_script_command_allowed` |
| 26 | 43 | `repair_job.rs::rejected_reason_for_repair_error` |
| 25 | 347 | `turn.rs::drive_task_contract_verifier` |
| 25 | 239 | `turn.rs::drive_repair_job_verifier` |
| 22 | 331 | `turn.rs::run_verifier_diagnostic_pass` |
| 21 | 117 | `turn.rs::request_assistant_reply` |

## Remaining Structural Issues

### 1. `run_actor_loop` Still Has Too Many Reasons To Change

`run_actor_loop` still mixes:

- session and turn setup
- model request orchestration
- tool-call parsing and dispatch
- active job arbitration
- artifact completion
- verifier execution
- verifier repair
- retry and safe-stop decisions
- user-facing progress/result formatting

This makes it difficult to reason about whether repair control is truly
single-sourced. It also makes small fixes risky because a local condition can
change behavior across unrelated phases.

Remaining work:

- split `run_actor_loop` into explicit phase functions or a small typed turn
  driver.
- make phase transitions visible in tests.
- keep `turn.rs` as a coordinator, not the implementation owner for each
  phase.

### 2. Verifier And Repair Flow Still Leaks Through `turn.rs`

The repair model has improved, but the highest-risk verifier functions still
live in `turn.rs`:

- `run_task_contract_verifier_once`
- `drive_task_contract_verifier`
- `drive_repair_job_verifier`
- `run_verifier_repair_pass_and_apply`
- `run_verifier_diagnostic_pass`

This keeps verifier repair control coupled to the general actor loop. The
result is that repair-job state can still be affected by generic loop concerns
such as retries, progress handling, and terminal result construction.

Remaining work:

- extract a `verifier_driver` module for verifier execution and verifier
  result normalization.
- extract a `repair_driver` module for diagnostic pass, repair pass, patch
  application, verifier delta, and safe-stop transition.
- make `turn.rs` call a narrow driver API such as `drive_active_job_once`.

### 3. Model Request Construction Is Still Mixed With Loop Control

`build_request_messages`, `request_assistant_reply_with_retry`, and
`request_assistant_reply` remain among the highest-complexity functions. They
combine prompt/message assembly, retry behavior, model switching, and error
normalization.

Remaining work:

- move message assembly into a `message_composer` module.
- move request/retry behavior into a `model_request` module.
- expose typed request outcomes to the actor loop.
- avoid letting model transport errors directly decide repair or artifact
  state transitions.

### 4. Tool Execution Is Still Too Centralized

`execute_tool_call` still has rough CC above the high-risk threshold. Tool
execution should remain generic, but the current shape keeps dispatch,
validation, result conversion, and side-effect reporting close together.

Remaining work:

- extract tool-call validation and normalized execution result conversion.
- ensure tool execution reports only typed outcomes to the loop.
- keep security/path validation close to the tool layer, not embedded in actor
  control flow.

### 5. `repair_job.rs` Is Conceptually Large Even If Function CC Is Controlled

`repair_job.rs` does not show the same cyclomatic spike as `turn.rs`, but it is
large and broad. Its risk is conceptual breadth rather than individual branch
count.

Remaining work:

- keep splitting by responsibility only where tests justify it:
  - state
  - driver transitions
  - diagnostic/admission
  - patch proposal validation
  - reporting
- avoid creating many tiny modules that hide the state machine.
- preserve a single public repair-job API for the actor loop.

### 6. Complexity Is Not Yet A Regression Gate

The current complexity review was manual and approximate. That is useful for
planning, but it will not prevent future regressions.

Remaining work:

- add a lightweight complexity report script or documented command.
- track at least:
  - largest functions by LOC
  - rough branch count
  - functions above agreed thresholds
  - `turn.rs` total size
- make the report non-blocking at first.
- later fail CI only on regressions over a baseline, not on the current debt.

## Recommended Priority Order

1. Extract `run_actor_loop` phases into a typed turn driver.
2. Extract verifier and repair driver responsibilities from `turn.rs`.
3. Extract message composition and model request retry handling.
4. Extract tool execution result normalization.
5. Continue controlled `repair_job.rs` decomposition.
6. Add a complexity regression report.

## Acceptance Criteria

The cyclomatic-complexity cleanup is complete when:

- `run_actor_loop` is no longer the dominant hotspot.
- no production function has rough CC above 50 without an explicit exception.
- new or modified production functions normally stay below rough CC 15.
- verifier repair dispatch is readable from one driver path.
- generic retry, focused edit recovery, and verifier repair cannot interleave
  through hidden branches in `turn.rs`.
- complexity reporting can be reproduced locally.
- existing repair-control tests still pass.

## Non-Goals

- Do not add FastAPI, CRUD, ToDo, pytest, or other use-case-specific repair
  branches to reduce apparent complexity.
- Do not split modules purely to lower numbers while keeping hidden coupling.
- Do not replace typed state transitions with prompt-only control.
- Do not remove safety validation in order to simplify flow.

