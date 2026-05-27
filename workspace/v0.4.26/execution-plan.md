# v0.4.26 Execution Plan

## Goal

Execute the v0.4.26 responsibility cleanup without adding another patchwork
recovery layer.

The key objective is to move final orchestration decisions out of `turn.rs`
and into typed owners. The plan below is intentionally incremental: each slice
must preserve behavior, add tests for the new owner boundary, and remove or
downgrade the old path before moving to the next slice.

## Ground Rules

- Do not add FastAPI, CRUD, pytest, ToDo, or other use-case-specific logic.
- Do not make generated tests green by weakening assertions.
- Do not introduce a provider abstraction.
- Do not make `TurnDriver` a new god object.
- Keep legacy wrappers temporary and delete them within the same slice or the
  next slice.
- Every production dispatch path must have one owner.
- Every phase must end with verification and a complexity report snapshot.

## Verification Gate For Every Slice

Run at minimum:

```bash
cargo fmt --check
cargo test <targeted-test-filter> --lib -q
python3 scripts/complexity_report.py --top 20 \
  src/agent/loop_run/turn.rs \
  src/agent/loop_run/model_request.rs \
  src/agent/loop_run/repair_job.rs
```

Run before finalizing a multi-slice batch:

```bash
cargo clippy --all-targets -- -D warnings
cargo test --lib -q
cargo build --release
```

Note: `cargo test --lib -q` may require normal local permissions because some
tests start mock HTTP servers.

## Slice 0: Architecture Invariant Tests First

### Purpose

Prevent future refactors from only moving code while preserving hidden
multi-owner dispatch.

### Tasks

1. Add `turn_driver_invariants` tests, initially against current public or
   `#[cfg(test)]` seams.
2. Add tests proving active repair suppresses:
   - artifact completion recovery
   - focused edit recovery
   - generic retry
   - deterministic fallback
3. Add tests proving terminal safe stop prevents any later model/tool action.
4. Add tests proving a missing verifier job suppresses generic retry.

### Files

- `src/agent/loop_run/active_job_arbiter.rs`
- new or existing in-crate test module under `src/agent/loop_run/*tests.rs`
- `src/agent/loop_run/turn.rs` only for temporary test seams if unavoidable

### Acceptance

- At least one test fails if a production path bypasses the active owner.
- No production behavior changes yet.

## Slice 1: ToolExecutionOutcome Boundary

### Purpose

Stop `execute_tool_call` from being both the low-level tool dispatcher and the
source of artifact/recovery evidence decisions.

### Tasks

1. Add `tool_execution.rs` or equivalent private module.
2. Introduce:

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

3. Wrap current `execute_tool_call` result conversion into this enum.
4. Move no-op/edit/read evidence classification behind the enum.
5. Keep built-in tool implementations unchanged.
6. Replace loop-side string/tool-name checks with typed outcome matching where
   safe.

### Tests

- read-only tool cannot satisfy required artifact edit evidence.
- `.anvil-state` and controller-owned files are excluded from artifact
  evidence.
- rejected unsafe path does not mutate job state except through rejection
  outcome.
- command output alone does not mark required artifacts complete.

### Acceptance

- `execute_tool_call` becomes dispatch plus conversion.
- artifact evidence updates consume `ToolExecutionOutcome`.
- rough CC of `execute_tool_call` decreases.

## Slice 2: VerifierDriver

### Purpose

Make verifier execution independent from repair and actor-loop decisions.

### Tasks

1. Add `verifier_driver.rs`.
2. Introduce:

```rust
enum VerifierOutcome {
    Passed,
    MissingVerifier(MissingVerifierReport),
    Failed(FailurePacket),
    Inconclusive(VerifierInconclusiveReport),
}
```

3. Move command selection and execution out of
   `run_task_contract_verifier_once`.
4. Move stdout/stderr bounding and failure packet construction into
   `VerifierDriver`.
5. Keep terminal policy outside `VerifierDriver`.
6. Keep safe-stop formatting outside `VerifierDriver`.

### Tests

- passing verifier returns `Passed`.
- missing verifier is distinct from failure.
- pytest assertion failure becomes a structured `FailurePacket`.
- import/runtime failure becomes a structured `FailurePacket`.
- raw output is bounded before entering controller state.

### Acceptance

- `turn.rs` no longer parses raw verifier output.
- verifier execution can be tested without running the full actor loop.

## Slice 3: RepairDriver

### Purpose

Make verifier repair a job-owned state machine instead of a generic recovery
branch.

### Tasks

1. Add `repair_driver.rs`.
2. Introduce:

```rust
enum RepairDriveOutcome {
    Continue,
    VerifierPassed,
    ReDiagnosticRequested,
    TargetSwitched,
    SafeStop(RepairSafeStopReport),
    Exhausted(RepairExhaustedReport),
    InternalError(String),
}
```

3. Move diagnostic pass execution from `turn.rs` into `RepairDriver`.
4. Move patch proposal execution into `RepairDriver`.
5. Move validated patch apply + verifier-delta feedback into `RepairDriver`.
6. Make `RepairDriver` consume `RepairJob::next_action`.
7. Forbid production patch repair without an accepted repair plan.
8. Move invalid patch proposal budget handling to job-level events.

### Tests

- no repair pass runs without `RepairJob::next_action`.
- no patch repair runs without accepted plan.
- invalid patch proposal consumes job-level budget.
- repeated invalid proposals cause re-diagnostic, target switch, or safe stop.
- repair cannot fall through into generic retry.

### Acceptance

- production verifier repair dispatch has one owner.
- `turn.rs` cannot directly select or execute a repair target.

## Slice 4: AssistantRequestDriver

### Purpose

Separate model retry mechanics from artifact/repair state transitions.

### Tasks

1. Extend `model_request.rs` with:

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
```

2. Add `AssistantRequestDriver` for retry counters and retry action selection.
3. Move retry counters out of `request_assistant_reply_with_retry`.
4. Keep deterministic fallback as a typed action that must be owner-gated.
5. Keep system-note generation as typed retry advice, not inline loop control.

### Tests

- timeout maps to typed timeout outcome.
- malformed tool call maps to format-error outcome.
- native-tool parser failure selects native-tool downgrade action.
- fallback action is not available when active owner forbids fallback.
- exhausted retry returns typed error without mutating repair state.

### Acceptance

- `request_assistant_reply_with_retry` is either removed or becomes a thin
  wrapper over `AssistantRequestDriver`.
- model transport failures do not directly mutate artifact or repair state.

## Slice 5: Fallback Owner Registry

### Purpose

Classify and constrain legacy recovery/fallback paths before deleting them.

### Tasks

1. Create a local inventory in code or tests for fallback paths:
   - production owned
   - validator assist
   - telemetry only
   - compatibility wrapper
   - deletion candidate
2. Add owner checks for:
   - deterministic fallback
   - focused edit recovery
   - generic repo change recovery
   - format-error finish path
3. Make each fallback check an explicit typed action or explicit rejection.
4. Delete paths classified as deletion candidates after equivalent driver
   behavior exists.

### Tests

- deterministic fallback cannot run during active repair unless repair driver
  explicitly requests it.
- focused edit recovery cannot steal active repair.
- generic retry cannot run while a missing verifier job is active.
- fallback path cannot return final `done` without verifier/report owner.

### Acceptance

- every fallback path has an owner and an expiry classification.
- no fallback dispatch is hidden inside `turn.rs`.

## Slice 6: RepairJob Internal Split

### Purpose

Reduce conceptual size without scattering transition ownership.

### Tasks

1. Split report formatting first:
   - `repair_job/report.rs` or private sibling module
2. Split admission helpers next:
   - accepted plan validation
   - authority checks
3. Split state structs only if imports stay manageable.
4. Split transition logic last, after `RepairDriver` is stable.
5. Preserve `RepairJob::next_action` as the public transition boundary.

### Tests

- budget consumption occurs exactly once per failed attempt.
- target switch requires accepted plan or diagnostic reason.
- safe-stop report includes blocker, target, evidence, and next action.
- no path bypasses `RepairJob::next_action`.

### Acceptance

- `RepairJob` public API remains small.
- transition ownership remains obvious.
- file size/conceptual breadth is reduced without introducing a new god module.

## Slice 7: TurnDriver Extraction

### Purpose

Only after lower-level owners exist, shrink `run_actor_loop` into a coordinator.

### Tasks

1. Add `turn_driver.rs`.
2. Introduce:

```rust
enum TurnPhaseAction {
    RunActiveJob(ActiveJobAction),
    RequestAssistant(AssistantRequestAction),
    ExecuteTool(ToolExecutionRequest),
    RunVerifier(VerifierRequest),
    ReturnTerminal(LoopResult),
}
```

3. Move active owner selection into `TurnDriver`.
4. Move terminal decision into `TurnDriver`.
5. Move observation recording decisions into `TurnDriver`.
6. Keep domain execution delegated to:
   - `VerifierDriver`
   - `RepairDriver`
   - `AssistantRequestDriver`
   - tool execution layer
7. Remove obsolete booleans and ad hoc early-return branches from
   `run_actor_loop`.

### Tests

- active repair owner wins over artifact completion.
- terminal result prevents later model/tool calls.
- normal implementation still reaches model request and tool execution.
- verifier pass returns `done`.
- verifier repair safe stop returns actionable stop.

### Acceptance

- `run_actor_loop` rough CC is below `50`.
- `turn.rs` no longer owns final repair/artifact/fallback decisions.
- dispatch source can be explained from one typed driver path.

## Slice 8: Complexity And Generic Evaluation

### Purpose

Verify the cleanup improved structure without overfitting to a single
FastAPI/CRUD workflow.

### Tasks

1. Re-run complexity report and update baseline.
2. Run targeted unit/invariant tests.
3. Run generic smoke cases:
   - Python CLI with tests/docs
   - Rust CLI/library with tests/docs
   - Node/TypeScript utility with tests/docs
   - README-only documentation task
   - verifier-missing setup task
4. Classify each case:
   - verifier passed
   - actionable safe stop
   - false-positive done
   - uncontrolled retry
   - wrong artifact scope

### Acceptance

- no false-positive `done`.
- no uncontrolled verifier repair retry loop.
- safe stop is actionable when repair cannot be completed.
- no new use-case-specific production branch.
- `turn.rs` hotspot severity decreases materially.

## Dependency Order

Recommended execution order:

1. Slice 0: invariant tests
2. Slice 1: tool outcome boundary
3. Slice 2: verifier driver
4. Slice 3: repair driver
5. Slice 4: assistant request driver
6. Slice 5: fallback owner registry
7. Slice 6: RepairJob internal split
8. Slice 7: TurnDriver extraction
9. Slice 8: complexity and generic evaluation

Reasoning:

- `TurnDriver` should be last among structural extractions. If introduced too
  early, it will simply become a second `turn.rs`.
- `RepairDriver` needs `VerifierDriver` output to be structured first.
- fallback deletion must wait until driver-owned equivalents exist.
- `RepairJob` splitting is safer after the repair driver clarifies which
  responsibilities are internal state and which are execution.

## Stop Conditions

Pause and reassess if:

- a slice requires behavior weakening to pass tests.
- a new driver starts accumulating unrelated responsibilities.
- a fallback path remains because no owner can be identified.
- complexity decreases in `turn.rs` but reappears as a large new god module.
- generic smoke improves only for FastAPI/CRUD but regresses other task shapes.
