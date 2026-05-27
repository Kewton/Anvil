# Cyclomatic Complexity Cleanup Work Plan

This plan turns the cyclomatic-complexity findings into an executable cleanup
sequence. The goal is not to chase metrics by moving code around. The goal is
to make the control loop easier to reason about, easier to test, and less
likely to let verifier repair, artifact completion, generic retry, and legacy
fallback paths interfere with each other.

## Guiding Principles

- Preserve behavior while moving code.
- Do not add FastAPI, CRUD, pytest, or other use-case-specific branches.
- Prefer typed outcomes over booleans, strings, and implicit side effects.
- Keep one production dispatch owner for each phase.
- Split only around stable responsibilities with tests.
- Do not make CI fail on existing complexity debt immediately.
- Reduce `turn.rs` responsibility first; cosmetic module splitting is not
  enough.

## Success Definition

This work is complete when:

- `run_actor_loop` is a thin coordinator, not the implementation owner for
  verifier, repair, model request, and tool execution.
- verifier repair production flow goes through a dedicated job driver.
- model request/message construction is testable without running the actor
  loop.
- tool execution reports typed outcomes to the loop.
- `RepairJob` remains the public state-machine boundary, with internal
  responsibilities split only where justified.
- a reproducible complexity report exists and can detect regressions.
- existing repair-control tests and generic smoke tests still pass.

## Phase 0: Baseline And Guard Rails

Purpose: make the cleanup measurable and safe.

Tasks:

1. Record the current complexity baseline:
   - top functions by rough cyclomatic complexity
   - largest functions by LOC
   - `turn.rs` total LOC
   - functions above rough CC 15 and rough CC 50
2. Record current verification baseline:
   - `cargo fmt --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo test --lib -q`
   - `cargo build --release`
3. Confirm unrelated dirty files stay outside commits.
4. Identify all tests that directly or indirectly exercise:
   - artifact completion
   - verifier execution
   - verifier repair
   - safe stop
   - generic retry

Deliverables:

- updated baseline note or complexity report output
- list of tests used as regression gates

Acceptance:

- no production behavior changes
- no unrelated workspace files staged
- baseline can be re-run locally

## Phase 1: Complexity Report Script

Purpose: avoid relying on manual one-off complexity checks.

Status: implemented as `scripts/complexity_report.py`; currently
non-blocking. The baseline is recorded in
`workspace/v0.4.25/complexity-baseline.md`.

Tasks:

1. Add a lightweight local script or dev command that reports:
   - largest functions by LOC
   - approximate branch score
   - functions above configured thresholds
   - module-level totals
2. Make it non-blocking:
   - initial output is informational
   - no CI failure on current known debt
3. Add a documented command in the workspace notes.
4. Store the current baseline as a reviewable artifact.

Implementation notes:

- Prefer a simple script unless the repo already has an appropriate `xtask`
  structure.
- The parser does not need to be perfect; it must be stable enough to spot
  trends.
- The script must avoid modifying source files.

Tests/checks:

- run the script on the current repository
- verify it reports `run_actor_loop` as the dominant hotspot
- verify it exits non-zero only on script failure, not because thresholds are
  exceeded

Acceptance:

- developers can reproduce the complexity report locally
- future changes can be compared against a baseline

## Phase 2: Extract Message Composition

Purpose: remove prompt/message construction complexity from `turn.rs` before
changing the actor loop itself.

Tasks:

1. Create a `message_composer` module.
2. Move pure message-building logic from `build_request_messages`.
3. Introduce typed input structs for:
   - user request context
   - active job context
   - tool history context
   - PAM assist context
   - verifier repair context
4. Keep model/network calls out of the composer.
5. Add snapshot-style or structural tests for key prompt shapes.

Migration order:

1. extract helper types with no behavior change
2. move simple message sections
3. move active-job-specific sections
4. remove old `turn.rs` helper once call sites are migrated

Tests:

- artifact completion message includes the required artifact target
- verifier repair message includes accepted repair plan and selected target
- PAM context is present only as assistive context
- tool result history does not leak raw stdout/stderr unnecessarily

Acceptance:

- `build_request_messages` no longer appears as a high-complexity function in
  `turn.rs`
- message composition is testable without actor-loop setup

## Phase 3: Extract Model Request And Retry

Purpose: separate model transport/retry behavior from state transitions.

Status: started. Focused-edit request sizing, streaming-transport selection,
non-streaming timeout selection, and non-streaming request execution have been
moved into `src/agent/loop_run/model_request.rs`. Retry orchestration still
remains in `turn.rs`.

Tasks:

1. Create a `model_request` module.
2. Move request/retry/fallback handling from:
   - `request_assistant_reply_with_retry`
   - `request_assistant_reply`
3. Return typed outcomes:
   - `Reply`
   - `MalformedToolCall`
   - `ProseOnly`
   - `Timeout`
   - `TransportError`
   - `ModelUnavailable`
4. Keep actor-loop policy as a consumer of outcomes, not the owner of retry
   internals.

Migration order:

1. wrap current request behavior behind a typed result
2. move retry loop into the new module
3. update actor loop to match on typed outcome
4. remove old wrappers from `turn.rs`

Tests:

- timeout maps to typed timeout outcome
- malformed reply does not directly mutate repair state
- fallback model usage is explicit and bounded
- request error reporting remains actionable

Acceptance:

- model transport concerns no longer decide artifact or repair transitions
  directly
- retry behavior is visible in one module

## Phase 4: Extract Verifier Execution Driver

Purpose: make verifier execution and failure-packet construction independent
from the actor loop.

Tasks:

1. Create a `verifier_driver` module.
2. Move command selection and verifier execution from
   `run_task_contract_verifier_once`.
3. Normalize verifier results into typed output:
   - pass
   - missing verifier
   - command failure
   - assertion failure
   - import/compile/runtime failure
   - timeout
   - inconclusive
4. Build `FailurePacket` in the verifier driver.
5. Keep terminal policy outside the low-level verifier executor.

Migration order:

1. introduce typed verifier result
2. wrap existing function with the new result type
3. move normalization logic
4. update `turn.rs` to consume typed result

Tests:

- pytest assertion failure becomes assertion failure packet
- import failure becomes import/runtime failure packet
- missing verifier is distinct from verifier failed
- verifier stdout/stderr is bounded and sanitized

Acceptance:

- verifier execution can be unit-tested without the full actor loop
- `turn.rs` no longer parses verifier output directly

## Phase 5: Extract Repair Driver

Purpose: make verifier repair a job-owned state machine, not a generic retry
branch.

Tasks:

1. Create a `repair_driver` module.
2. Move repair-specific execution from:
   - `drive_repair_job_verifier`
   - `run_verifier_diagnostic_pass`
   - `run_verifier_repair_pass_and_apply`
3. Make the driver consume `RepairJob::next_action`.
4. Ensure patch repair cannot run without an accepted repair plan.
5. Feed verifier delta back into `RepairJob`.
6. Return typed `RepairDriveOutcome`:
   - continue
   - verifier passed
   - re-diagnostic requested
   - target switched
   - safe stop
   - exhausted
   - internal error

Migration order:

1. move diagnostic pass execution
2. move patch proposal execution
3. move apply-and-verify feedback handling
4. remove direct repair dispatch from `turn.rs`
5. assert that generic retry/focused edit cannot run while repair driver owns
   the active job

Tests:

- invalid patch proposal consumes job-level budget
- repeated invalid proposals transition to re-diagnostic or safe stop
- safe stop includes blocker, target, evidence, and next action
- test weakening is rejected before apply
- implementation repair can proceed when authority is clear

Acceptance:

- production repair dispatch has one owner
- verifier repair cannot fall through into generic retry
- `turn.rs` cannot directly select a repair target

## Phase 6: Normalize Tool Execution Outcomes

Purpose: reduce `execute_tool_call` complexity and make loop decisions rely on
typed evidence.

Tasks:

1. Introduce `ToolExecutionOutcome`.
2. Extract tool preflight validation.
3. Extract per-tool result conversion.
4. Keep path/security validation inside the tool layer.
5. Make actor loop consume normalized evidence:
   - repo edit
   - read
   - command
   - no-op
   - rejection
   - failure

Migration order:

1. add outcome enum and conversion helpers
2. wrap existing tool execution results
3. migrate loop-side branches to typed outcomes
4. reduce `execute_tool_call` to dispatch plus conversion

Tests:

- read-only tools cannot satisfy required artifact edit evidence
- internal files such as `.anvil-state` are excluded from artifact evidence
- unsafe path rejection does not mutate job state
- command output alone does not mark an artifact complete

Acceptance:

- actor loop no longer branches on low-level tool names where typed evidence is
  available
- `execute_tool_call` drops below the high-risk complexity threshold

## Phase 7: Controlled `RepairJob` Internal Split

Purpose: reduce conceptual size without hiding the state machine.

Tasks:

1. Split stable internal responsibilities only after Phases 4 and 5 clarify
   driver boundaries.
2. Preferred internal modules:
   - `repair_job::state`
   - `repair_job::transition`
   - `repair_job::admission`
   - `repair_job::report`
3. Keep `RepairJob` as the public API.
4. Preserve `RepairJob::next_action` as the transition boundary.

Migration order:

1. move report formatting first if it is isolated
2. move admission validation next
3. move state structs only if imports remain manageable
4. move transition code last, after tests stabilize

Tests:

- budgets are consumed exactly once per failed attempt
- target switching requires accepted plan or diagnostic reason
- no path bypasses `RepairJob::next_action`
- report construction works for exhausted, ambiguous, and unsafe cases

Acceptance:

- `repair_job.rs` can be read by responsibility area
- public API remains small
- transition ownership remains obvious

## Phase 8: Turn Driver Extraction

Purpose: collapse the remaining actor-loop hotspot after dependent pieces are
extracted.

Tasks:

1. Introduce a `TurnPhase` or `LoopControlAction` based driver.
2. Move these decisions out of `run_actor_loop`:
   - active job selection
   - next action execution
   - observation recording
   - terminal decision
3. Keep the old entry point but delegate to the driver.
4. Delete obsolete flags and wrappers once covered by tests.

Migration order:

1. add phase enum and no-op wrapper
2. move active job selection
3. move action execution dispatch
4. move terminal decision
5. shrink `run_actor_loop` to setup and driver invocation

Tests:

- active repair job wins over artifact completion
- active missing-verifier job wins over generic retry
- terminal result prevents additional tool/model calls
- normal task still completes through implementation, tests/docs, verifier

Acceptance:

- `run_actor_loop` rough CC drops below 50
- dispatch source can be explained from one driver path
- no hidden production recovery path bypasses the driver

## Phase 9: Regression Evaluation

Purpose: prove the cleanup preserved behavior and improved maintainability.

Required local checks:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --lib -q`
- `cargo build --release`
- complexity report command

Targeted tests:

- message composer tests
- model request outcome tests
- verifier driver tests
- repair driver tests
- tool outcome tests
- turn driver phase tests

Smoke evaluation:

1. Run no-PAM generic smoke cases:
   - FastAPI CRUD
   - small Rust CLI
   - Node/TypeScript utility
   - README-only documentation task
   - verifier-missing project setup task
2. Run PAM-enabled smoke cases after no-PAM behavior is stable.
3. Classify each result:
   - verifier passed
   - actionable safe stop
   - false-positive done
   - uncontrolled retry
   - wrong artifact scope

Acceptance:

- no false-positive `done`
- no uncontrolled verifier repair retry loop
- safe stop is actionable when repair cannot be completed
- complexity report shows reduced `turn.rs` hotspot severity

## Execution Order

Recommended order:

1. Phase 0: Baseline And Guard Rails
2. Phase 1: Complexity Report Script
3. Phase 2: Extract Message Composition
4. Phase 3: Extract Model Request And Retry
5. Phase 4: Extract Verifier Execution Driver
6. Phase 5: Extract Repair Driver
7. Phase 6: Normalize Tool Execution Outcomes
8. Phase 7: Controlled `RepairJob` Internal Split
9. Phase 8: Turn Driver Extraction
10. Phase 9: Regression Evaluation

This order avoids extracting the top-level driver before its dependencies have
clear typed boundaries. It also avoids splitting `RepairJob` too early, which
would make state ownership harder to see.

## Risk Controls

- Commit each phase separately.
- Keep wrappers temporarily only when needed for safe migration.
- Remove wrappers in the same phase or the next phase; do not leave permanent
  compatibility layers.
- Do not merge a phase that only moves code without tests around the moved
  responsibility.
- Treat any new use-case-specific branch as a design regression.
- Treat any new production dispatch path outside the active owner as a blocker.

## Stop Conditions

Pause implementation and reassess if:

- a phase requires changing user-visible behavior to complete extraction.
- tests require weakening safety checks.
- the new driver needs more branches than the old code.
- complexity moves from `turn.rs` into another god module.
- smoke evaluation improves FastAPI CRUD but regresses unrelated generic tasks.
