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
| `src/agent/loop_run/turn.rs` | 1036 | 3.03 | 362 | 30 | 1 |
| `src/agent/loop_run/repair_job.rs` | 236 | 2.4 | 26 | 3 | 0 |
| `src/agent/loop_run/model_request.rs` | 9 | 3.11 | 8 | 0 | 0 |
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

## Resolution Approach By Issue

### 1. Resolve `run_actor_loop` By Introducing A Small Turn Driver

Target design:

- keep one public entry point for the existing caller.
- introduce a small `TurnDriver` or `TurnPhaseDriver` that owns phase
  sequencing.
- represent each phase result with a typed enum instead of scattered booleans
  and early returns.
- keep phase implementations small and side-effect bounded.

Proposed phase boundary:

1. `prepare_turn`
   - load session state
   - snapshot active jobs
   - compute display context
2. `select_next_action`
   - ask `ActiveJobArbiter` for the next owner/action
   - no model call, verifier call, or tool call here
3. `execute_next_action`
   - call exactly one narrow executor based on the selected typed action
4. `record_observation`
   - update transcript, job state, ledger, and progress text
5. `decide_terminal_state`
   - return `done`, actionable safe stop, or continue

Migration method:

- first add the phase functions while leaving behavior unchanged.
- move one block at a time from `run_actor_loop` into a phase.
- after each move, add focused tests for the phase outcome.
- keep `run_actor_loop` as a thin shell until it is safe to rename or delete
  the old body.

Tests:

- active repair job suppresses artifact recovery.
- missing verifier job suppresses generic retry.
- terminal safe stop exits before any new model/tool action.
- normal implementation turn still reaches tool execution.

Done criteria:

- `run_actor_loop` no longer contains verifier, repair, request, or tool
  implementation details.
- rough CC for `run_actor_loop` drops below 50.
- branch-heavy logic is covered by phase-level tests.

### 2. Resolve Verifier And Repair Leakage With A Dedicated Job Driver API

Target design:

- `turn.rs` should not know the internals of verifier repair.
- verifier repair should be driven through a narrow API:
  `drive_repair_job_once(context) -> RepairDriveOutcome`.
- the driver owns diagnostic pass, repair pass, patch validation,
  verifier-delta interpretation, re-diagnostic, target switching, and safe
  stop transition.

Proposed modules:

- `verifier_driver`
  - command selection
  - verifier execution
  - stdout/stderr normalization
  - failure packet construction
- `repair_driver`
  - consumes `RepairJob::next_action`
  - runs diagnostic or patch-generation steps
  - applies only validated patch proposals
  - feeds verifier delta back into the job
- `repair_report`
  - formats actionable safe stop and final failure reports

Migration method:

- move `run_task_contract_verifier_once` first because it is a boundary with
  clear inputs and outputs.
- move `run_verifier_diagnostic_pass` next.
- move `run_verifier_repair_pass_and_apply` last because it touches patch
  validation, filesystem edits, and job state.
- keep old function names as temporary wrappers only if tests need smaller
  diffs; remove wrappers after call sites are updated.

Tests:

- compile/import failure selects implementation target.
- assertion mismatch produces an authority-aware repair plan.
- malformed diagnostic result consumes job-level budget and then re-diagnoses
  or safe-stops.
- unsafe test weakening is rejected before apply.
- verifier delta can switch target or stop without falling into generic retry.

Done criteria:

- production verifier repair is dispatched only by the repair driver.
- `turn.rs` cannot directly choose a verifier repair target.
- invalid repair proposal handling is job-level, not loop-level.

### 3. Resolve Model Request Complexity With Message Composer And Request Client

Target design:

- message construction is pure and testable.
- model transport/retry behavior is separate from loop decisions.
- request failures become typed outcomes, not ad hoc control-flow branches.

Proposed modules:

- `message_composer`
  - builds system/developer/user/tool context
  - owns context-budget trimming
  - never performs network calls
- `model_request`
  - sends request to Ollama client
  - handles retry/fallback model selection
  - normalizes timeout, malformed tool call, prose-only response, and transport
    errors into typed results

Migration method:

- extract `build_request_messages` into pure functions first.
- snapshot current prompt/message behavior with tests.
- extract `request_assistant_reply_with_retry` after message composition is
  stable.
- keep loop-level policy as a consumer of typed outcomes only.

Tests:

- artifact completion prompt includes required artifact target.
- verifier repair prompt includes accepted repair plan and target.
- PAM context is included as assistive context but not authority.
- malformed response outcome does not directly mutate repair state.

Done criteria:

- `build_request_messages`, `request_assistant_reply_with_retry`, and
  `request_assistant_reply` are no longer high-complexity functions in
  `turn.rs`.
- prompt construction regressions are caught by unit tests.
- retry policy is visible in one place.

### 4. Resolve Tool Execution Centralization With Typed Tool Outcomes

Target design:

- tool execution remains generic.
- path/security validation remains inside the tool layer.
- actor loop receives a normalized `ToolExecutionOutcome` and does not inspect
  low-level tool details unless required for job evidence.

Proposed shape:

```rust
enum ToolExecutionOutcome {
    RepoEdit(RepoEditEvidence),
    Read(ReadEvidence),
    Command(CommandEvidence),
    Noop(NoopReason),
    Rejected(ToolRejection),
    Failed(ToolFailure),
}
```

Migration method:

- extract preflight validation from `execute_tool_call`.
- extract per-tool result conversion.
- keep existing built-in tool implementations unchanged at first.
- replace loop-side conditionals with outcome matching.

Tests:

- repo edit evidence excludes `.anvil-state` and other internal files.
- command output does not become artifact evidence by itself.
- rejected unsafe path does not mutate job state.
- read-only tool calls cannot satisfy required artifact edit evidence.

Done criteria:

- `execute_tool_call` is mostly dispatch plus outcome conversion.
- security-sensitive checks are closer to tool implementation.
- actor loop branches on typed outcomes, not string/tool-name patterns.

### 5. Resolve `repair_job.rs` Conceptual Size Through Controlled Splitting

Target design:

- keep `RepairJob` as the public state-machine boundary.
- split internal responsibilities only when the extracted module has a stable
  domain concept and test surface.
- avoid scattering transition logic across many files.

Safe split order:

1. `repair_job::state`
   - job fields, budgets, cluster state, active target
2. `repair_job::transition`
   - `next_action`
   - event application
   - retry/re-diagnostic/safe-stop transitions
3. `repair_job::admission`
   - accepted repair plan validation
   - authority consistency checks
4. `repair_job::report`
   - actionable safe-stop report construction

Keep together for now:

- transition policy and budget consumption, until tests prove the boundary is
  stable.
- authority decisions and plan admission, unless duplication appears.

Tests:

- every transition consumes or preserves budget intentionally.
- repeated invalid patch proposals cannot loop indefinitely.
- target change requires accepted plan or explicit diagnostic reason.
- safe stop report includes blocker, target, evidence, and next action.

Done criteria:

- `repair_job.rs` can be understood by reading state, transition, admission,
  and report modules independently.
- public API remains small.
- no new path bypasses `RepairJob::next_action`.

### 6. Resolve Missing Complexity Regression Gate With A Baseline Report

Target design:

- complexity reporting starts as a non-blocking developer command.
- CI should initially fail only on measurement script failure, not on current
  complexity debt.
- once the baseline is stable, CI can block new regressions above threshold.

Proposed implementation:

- add a small script under `scripts/` or `dev-tools/` that reports:
  - largest functions by approximate LOC
  - rough branch score
  - functions above threshold
  - module totals
- store current baseline in `workspace/v0.4.25` or a dedicated
  `dev-reports/complexity-baseline.json`.
- add a documented command:
  `cargo xtask complexity` only if an `xtask` already exists or is justified;
  otherwise keep a simple script.

Threshold policy:

- warn when any changed production function exceeds rough CC 15.
- require explicit exception when any production function exceeds rough CC 50.
- fail CI only when a touched function increases above the stored baseline.
- do not fail CI merely because existing `turn.rs` debt still exists.

Tests/checks:

- script handles Rust comments, strings, macros, and nested functions well
  enough for stable trend reporting.
- script exits non-zero only on parser/runtime failure in the first phase.
- baseline update is manual and reviewable.

Done criteria:

- complexity report can be reproduced locally.
- new hotspots are visible before review.
- current debt is tracked without blocking unrelated work.

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
