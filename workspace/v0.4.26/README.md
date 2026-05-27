# v0.4.26: Orchestration Responsibility Cleanup

Detailed execution plan:

- `workspace/v0.4.26/execution-plan.md`
- `workspace/v0.4.26/implementation-log.md`

## Purpose

This document organizes concrete solutions for the current structural problem:
`turn.rs` is still the place where final orchestration decisions gather.

The issue is not only file size. Some extraction has worked, but many final
decisions still remain in the actor loop:

- which job owns the next action
- whether verifier repair continues or stops
- whether generic recovery may run
- whether a tool result satisfies artifact progress
- whether a verifier failure becomes repair, retry, or safe stop
- whether the whole turn is `done`

The desired direction is to make `turn.rs` a thin coordinator. Final decisions
should move to typed owners:

- `ActiveJobArbiter`: selects the current owner.
- `RepairJob`: owns repair lifecycle state.
- `VerifierDriver`: owns verifier execution and failure packet construction.
- `RepairDriver`: owns diagnostic / patch / verifier-delta progression.
- `ModelRequest`: owns transport, timeout, retry, and reply classification.
- `ToolExecutionOutcome`: normalizes tool result evidence.
- `TurnDriver`: sequences phases, but does not implement domain-specific
  decisions.

## Current State

Recent work added:

- `scripts/complexity_report.py`
- `src/agent/loop_run/model_request.rs`
- initial extraction of streaming / non-streaming request sizing and
  non-streaming assistant request execution

This reduced some local complexity, but did not remove the main bottleneck.
The current focused complexity report still shows:

- `turn.rs::run_actor_loop`: rough CC `362`
- `turn.rs::build_request_messages`: rough CC `42`
- `turn.rs::request_assistant_reply_with_retry`: rough CC `40`
- `turn.rs::run_task_contract_verifier_once`: rough CC `38`
- `turn.rs::execute_tool_call`: rough CC `31`
- `turn.rs::run_verifier_repair_pass_and_apply`: rough CC `30`

Interpretation:

- helper extraction is working
- extracted modules are mostly low complexity
- `turn.rs` still owns final control decisions
- the next work must move decision ownership, not just helper code

## Problem 1: `run_actor_loop` Still Owns Too Many Final Decisions

### Problem

`run_actor_loop` still mixes:

- turn setup
- active job selection
- model request
- tool execution
- artifact completion
- verifier execution
- verifier repair
- retry control
- safe stop / done terminalization

This makes it difficult to reason about the control flow. A change for one
phase can accidentally change another phase.

### Solution

Introduce a small `TurnDriver` that sequences phases with typed outcomes.

Recommended shape:

```rust
enum TurnPhaseAction {
    RunActiveJob(ActiveJobAction),
    RequestAssistant(ModelRequestAction),
    ExecuteTool(ToolExecutionRequest),
    RunVerifier(VerifierRequest),
    ReturnTerminal(LoopResult),
}

enum TurnPhaseOutcome {
    Continue,
    ObservationRecorded,
    Terminal(LoopResult),
}
```

`run_actor_loop` should eventually do only:

1. initialize turn context
2. ask `TurnDriver` for the next action
3. execute the typed action
4. feed the typed outcome back
5. return terminal result when instructed

### Migration Steps

1. Add `TurnContext` containing the mutable state needed per iteration.
2. Add `TurnDriver::select_next_action`.
3. Move active job selection from `run_actor_loop` into the driver.
4. Move terminalization into `TurnDriver::decide_terminal`.
5. Keep execution details delegated to existing functions until each driver is
   ready.
6. Delete direct fallback/retry checks from `run_actor_loop` after they are
   represented as driver actions.

### Acceptance Criteria

- `run_actor_loop` no longer directly decides verifier repair, artifact
  recovery, generic retry, or safe stop.
- `run_actor_loop` rough CC drops below `50`.
- all production next-action selection goes through one driver call.

## Problem 2: `RepairJob` Is Not Yet The Only Repair Lifecycle Owner

### Problem

`RepairJob::next_action()` exists, but verifier repair execution still depends
on `turn.rs` functions such as:

- `drive_repair_job_verifier`
- `run_verifier_diagnostic_pass`
- `run_verifier_repair_pass_and_apply`

This means `RepairJob` owns some state, while `turn.rs` still owns much of the
repair progression.

### Solution

Create `RepairDriver` as the only production executor for active repair jobs.

Recommended API:

```rust
fn drive_repair_job_once(input: RepairDriveInput) -> RepairDriveOutcome;
```

`RepairDriver` owns:

- reading `RepairJob::next_action`
- running diagnostic LLM pass
- validating accepted repair plan
- running patch proposal pass
- applying validated patch
- rerunning verifier or consuming verifier delta
- deciding re-diagnostic, target switch, safe stop, or exhausted

`turn.rs` should not inspect repair internals. It should only pass context and
consume `RepairDriveOutcome`.

### Migration Steps

1. Move diagnostic pass execution into `repair_driver`.
2. Move patch proposal execution into `repair_driver`.
3. Move patch apply / verifier delta feedback into `repair_driver`.
4. Make invalid patch proposal budget consumption job-level only.
5. Delete production repair dispatch paths from `turn.rs`.

### Acceptance Criteria

- verifier repair cannot run without `RepairJob::next_action`.
- accepted repair plan is required before patch repair.
- invalid repair proposals cannot fall back to generic retry.
- repeated invalid proposals transition to re-diagnostic, target switch, or
  safe stop.

## Problem 3: Request / Retry Still Lives Too Close To Loop Policy

### Problem

`model_request.rs` now owns some transport helpers, but retry orchestration is
still in `turn.rs::request_assistant_reply_with_retry`.

That function still decides:

- native tool downgrade
- format-error retry
- deterministic fallback after format error
- focused-edit timeout retry
- transport retry
- plan materialization

This is still orchestration-heavy.

### Solution

Move retry classification into `model_request`, but keep policy decisions
explicitly typed.

Recommended shape:

```rust
enum AssistantRequestOutcome {
    Reply(AssistantReply),
    Interrupted,
    NativeToolParserFailure(String),
    NativeToolTransportFailure(String),
    ToolCallFormatError(String),
    Timeout(String),
    TransportError(String),
    Exhausted(String),
}

enum AssistantRetryAction {
    RetrySameModel,
    RetryWithoutNativeTools,
    RetryWithSystemNote(String),
    MaterializedReply(AssistantReply),
    ReturnError(String),
}
```

`model_request` classifies request failures. `TurnDriver` or a small
`AssistantRequestDriver` chooses the next retry action.

### Migration Steps

1. Introduce typed `AssistantRequestOutcome`.
2. Wrap current `request_assistant_reply` result into that outcome.
3. Move retry counters into `AssistantRequestDriver`.
4. Make deterministic fallback a separate driver decision, not an inline branch.
5. Remove direct retry loops from `turn.rs`.

### Acceptance Criteria

- model transport failures do not directly mutate artifact or repair state.
- retry behavior is visible in one module.
- deterministic fallback is not reachable unless the active owner allows it.

## Problem 4: Verifier / Repair Boundary Is Still Blurred

### Problem

Verifier execution and repair interpretation are still close together in
`turn.rs`.

Current high-risk functions:

- `run_task_contract_verifier_once`
- `drive_task_contract_verifier`
- `drive_repair_job_verifier`
- `run_verifier_diagnostic_pass`
- `run_verifier_repair_pass_and_apply`

These functions mix:

- command selection
- command execution
- output parsing
- failure classification
- diagnostic prompt construction
- repair target choice
- terminal result construction

### Solution

Split into two layers:

1. `VerifierDriver`
   - runs the verifier
   - normalizes result
   - builds `FailurePacket`
2. `RepairDriver`
   - consumes `FailurePacket`
   - advances `RepairJob`
   - never directly parses raw verifier text unless bounded by
     `FailurePacket`

Recommended verifier result:

```rust
enum VerifierOutcome {
    Passed,
    MissingVerifier(MissingVerifierReport),
    Failed(FailurePacket),
    Inconclusive(VerifierInconclusiveReport),
}
```

### Migration Steps

1. Move command execution and output normalization to `verifier_driver`.
2. Ensure raw stdout/stderr is bounded before entering job state.
3. Make `FailurePacket` the boundary between verifier and repair.
4. Move safe-stop formatting to report builders.
5. Remove verifier-output parsing branches from `turn.rs`.

### Acceptance Criteria

- `turn.rs` never parses verifier output directly.
- verifier failures are represented as `FailurePacket`.
- repair driver consumes structured verifier outcomes only.

## Problem 5: Tool Execution Is Still Too Centralized

### Problem

`execute_tool_call` remains complex and close to control-flow decisions.

It still combines:

- policy validation
- path validation
- tool dispatch
- result conversion
- artifact evidence recording
- feedback and recovery interactions

### Solution

Introduce `ToolExecutionOutcome`.

Recommended shape:

```rust
enum ToolExecutionOutcome {
    RepoEdit(RepoEditEvidence),
    Read(ReadEvidence),
    Command(CommandEvidence),
    Rejected(ToolRejection),
    Failed(ToolFailure),
    Noop(NoopReason),
}
```

The tool layer should own:

- path safety
- tool allow/deny checks
- execution result normalization

The turn driver should only consume typed evidence.

### Migration Steps

1. Introduce `ToolExecutionOutcome`.
2. Wrap current tool results into typed outcomes.
3. Move artifact evidence update behind `ToolExecutionOutcome::RepoEdit`.
4. Move policy rejection reporting into typed `ToolRejection`.
5. Delete loop-side string/tool-name matching where typed evidence is enough.

### Acceptance Criteria

- read-only tools cannot satisfy edit-required artifact progress.
- `.anvil-state` and controller-owned files cannot become artifact evidence.
- rejected tools do not mutate job state except through explicit rejection
  outcomes.

## Problem 6: Legacy Recovery / Fallback Paths Still Create Control Noise

### Problem

Even with `ActiveJobArbiter` and `RepairJob`, old recovery paths can still
exist as inline exceptions.

This makes it hard to prove:

- repair owns repair
- artifact completion owns artifact recovery
- generic retry cannot interrupt active repair
- deterministic fallback cannot override the active owner

### Solution

Classify every fallback path with an owner and an expiry.

Allowed categories:

- `ProductionOwned`
- `ValidatorAssist`
- `TelemetryOnly`
- `CompatibilityWrapper`
- `DeletionCandidate`

Every fallback must declare:

- owner
- allowed phase
- guard condition
- expiry condition
- tests proving it cannot run under another active owner

### Migration Steps

1. Add an inventory table for all fallback/recovery paths.
2. Add owner assertions in tests.
3. Move allowed fallback dispatch into `ActiveJobArbiter` / driver actions.
4. Convert deterministic repair paths to validator assist or telemetry.
5. Delete compatibility wrappers after equivalent driver behavior is tested.

### Acceptance Criteria

- no production fallback runs without owner approval.
- no fallback path can return `done`.
- deterministic fallback cannot run during active verifier repair unless the
  repair driver explicitly asks for it.

## Problem 7: Tests Verify Behavior But Not Architecture Enough

### Problem

The repository has many tests, but many are behavior-level. They do not fully
prove architectural invariants such as:

- all repair dispatch goes through `RepairDriver`
- `RepairJob::next_action` is the only repair lifecycle transition source
- generic retry cannot interrupt active repair
- tool evidence must be typed before it can satisfy artifact progress

### Solution

Add architecture-level invariant tests.

Recommended test groups:

1. `turn_driver_invariants`
   - active job selection priority
   - terminal result stops all later actions
2. `repair_driver_invariants`
   - no repair without accepted plan
   - invalid patch consumes job budget
   - exhausted repair safe-stops
3. `fallback_owner_invariants`
   - fallback cannot run under wrong owner
   - deterministic fallback cannot produce final done
4. `tool_outcome_invariants`
   - read-only outcome cannot count as edit
   - internal path excluded from artifact evidence

### Migration Steps

1. Add invariant tests before deleting old paths.
2. Move one production path at a time behind typed owner.
3. Delete the old path only after invariant tests fail without the new owner.
4. Keep smoke tests for end-to-end behavior, but do not rely on them for
   architecture.

### Acceptance Criteria

- architecture tests fail if a new direct repair path is added to `turn.rs`.
- architecture tests fail if generic retry runs under active repair.
- architecture tests fail if untyped tool output marks artifact progress.

## Recommended Implementation Order

1. Add architecture invariant tests around current behavior.
2. Introduce `ToolExecutionOutcome` and normalize tool evidence.
3. Extract `VerifierDriver`.
4. Extract `RepairDriver`.
5. Move retry orchestration into `AssistantRequestDriver`.
6. Extract `TurnDriver` after the lower-level drivers expose typed APIs.
7. Delete legacy/fallback paths classified as deletion candidates.
8. Re-run complexity report and generic smoke evaluation.

This order is intentional. Extracting `TurnDriver` too early would move the
large branch structure without reducing it. The lower-level drivers need typed
boundaries first.

## Expected Result

After these changes:

- `turn.rs` becomes a coordinator.
- `run_actor_loop` stops being the final decision center.
- verifier repair is controlled by `RepairJob` + `RepairDriver`.
- fallback behavior is owner-gated and auditable.
- complexity moves from one giant function into small typed state machines.
- future regressions are easier to catch because architecture invariants become
  testable.

## Non-Goals

- Do not add use-case-specific repair rules.
- Do not weaken tests to make repair green.
- Do not replace typed state transitions with prompt-only control.
- Do not create many tiny modules without a clear owner and invariant tests.
