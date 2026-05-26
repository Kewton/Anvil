# Remaining Completion Work Plan

## Purpose

This plan lists the remaining work to reach the intended state-control quality
bar for Anvil's local-LLM coding-agent loop.

Complete achievement means every run reaches one controlled terminal state:

- `done`: required owned artifacts are complete and the verifier passed.
- actionable safe stop: the controller can explain why completion is unsafe,
  ambiguous, timed out, verifier-missing, or budget-exhausted.

The goal is not to force all runs green. The goal is to eliminate uncontrolled
retry exhaustion, generic `missing_repo_edits` during verifier repair, and
framework-specific repair authority.

## Current State

Already covered:

- `LoopControlAction` is the top-level verifier repair dispatcher.
- `RepairJob::next_action()` owns verifier repair progress.
- `MissingVerifierJob` owns missing-verifier bootstrap instead of generic retry.
- Generic recovery, focused edit recovery, and deterministic fallback are
  blocked while a verifier-owned job is active.
- Structured and legacy shell verifier paths are now bounded by timeout.
- Verifier timeout is converted into verifier failure evidence instead of raw
  transport error.
- `.anvil-state/verifier-python/site` is asserted as controller-owned ignored
  state and no longer inherits arbitrary parent `PYTHONPATH` entries.
- `project_probe` now produces a minimal `ProjectUnit` fact object for
  completion probing. It records root, manifest evidence, current artifact
  roles, verifier candidates, observed stacks, and verifier timeout class.
- Verifier timeout evidence now carries a bounded `timeout_kind` bucket:
  generated test hang, dependency setup timeout, environment stall timeout,
  build command timeout, long-running verifier, or unknown timeout.
- `RepairJob` now stores typed timeout evidence and uses it for the first
  state-machine route: environment-stall timeouts safe-stop without patch
  repair.

Remaining gaps:

- Repair patch convergence still depends on LLM proposal quality and validator
  outcomes; it now has tighter state-machine tests, but large smoke stability
  is not yet proven.
- Generality across non-FastAPI tasks is partially proven by small smoke
  (`Rust CLI` passed), but Python CLI repair convergence is still unstable.
- Latest residual slice implemented:
  - `VerifierDelta::Improved` no longer bypasses target-exhaustion state.
    Repeated "improved but still failing" attempts now re-diagnostic/replan
    before another patch is requested.
  - Diagnostic-budget exhaustion after verifier repair now surfaces as
    `repair_exhausted` instead of raw `verifier_failed`.
  - Unit tests pin the improved-delta target-exhaustion branch and the
    repair-exhausted exit mapping.

## Phase 1: ProjectUnit Verifier Model

Goal: make verifier selection a data-driven project-unit decision rather than a
stack-specific guess.

Work:

- Introduce a small `ProjectUnit` model in or near `project_probe` / `auto_test`:
  - `root`
  - manifest files
  - owned artifact roles in the unit
  - candidate verifier commands
  - timeout class
  - confidence and evidence
- Replace direct verifier command selection in task-contract flow with
  `ProjectUnit` facts.
- Support multiple plausible units without scanning ignored/controller paths.
- Keep stack-specific knowledge as detector facts, not dispatch authority.

Acceptance:

- Project-unit tests cover Python, Rust, Node, docs-only, and multi-directory
  workspaces.
- `.anvil-state`, `.git`, `target`, `node_modules`, and generated controller
  state never become project units or owned artifacts.
- Verifier selection logs explain the selected project unit and candidate
  evidence.

Implemented slice:

- Added `ProjectUnit`, `ProjectUnitVerifierCandidate`, and
  `ProjectUnitTimeoutClass` in `project_probe`.
- `CompletionProbeDecision::RunVerifier` now carries the selected
  `ProjectUnit`.
- Completion probe logs include a bounded project-unit summary.
- Unit tests cover Rust, Node, explicit multi-directory Python, ignored
  `.anvil-state` inputs, and stable timeout-class labels.
- Task-contract verifier selection now passes the current `ProjectUnit` into
  `AutoTestRunner`, and candidate selection is built from verifier sources
  admitted by that project unit instead of falling back to root-level stack
  guesses.
- Unit coverage verifies that a parent/root Cargo manifest cannot steal
  verifier selection from a current Python task unit.
- Unit coverage verifies that a `ProjectUnit` with no verifier candidates does
  not fall back to unrelated root-level verifier guesses.
- Legacy deterministic verifier-repair candidates are no longer invoked by the
  production patch-provider main path. The deterministic admission bridge and
  old candidate builder were removed from `turn.rs`; validator coverage now
  lives around explicit repair-intent admission instead of candidate generation.
- `build_arbiter_candidates()` now short-circuits after a verifier-owned
  candidate. Lower-priority recovery candidates are not built while
  `RepairJob` / `MissingVerifierJob` owns the turn.
- AutoTest verifier detection filters controller-owned changed files at the
  entrypoint, preventing `.anvil-state` paths from becoming verifier or Python
  surface evidence.
- RepoEdit observation and ArtifactLedger seed helpers now reject
  controller-owned paths at the boundary, so `.anvil-state` cannot become
  edited-path evidence, artifact evidence, verifier observations, or
  repair-target admission input.

Implemented in the final cleanup slice:

- Post-loop verifier execution now receives the current `ProjectUnit` and uses
  the same project-unit-bound verifier discovery as task-contract verifier
  execution.
- `ProjectUnit` now carries a small confidence label. Docs-only/no-code units
  are low-confidence and do not fall back to unrelated root-level verifier
  guesses.
- The old deterministic repair candidate builder and its candidate-generation
  tests were deleted. Remaining verifier-repair tests validate the admitted
  `VerifierRepairIntent` path and `RepairJob` transitions.
- Small smoke exposed and fixed a generic Python dependency inference bug:
  `import types` is now recognized as stdlib and is not converted into a
  non-existent `pip install types` dependency.

## Phase 2: Timeout Classification

Goal: route timeout failures to the right controlled outcome.

Work:

- Split the broad verifier-timeout bucket into bounded enum categories:
  - `VerifierTimeoutKind::LongRunningVerifier`
  - `VerifierTimeoutKind::GeneratedTestHang`
  - `VerifierTimeoutKind::DependencySetupTimeout`
  - `VerifierTimeoutKind::EnvironmentStall`
  - `VerifierTimeoutKind::Unknown`
- Feed timeout kind into `FailurePacket` / `RepairJob` as structured evidence.
- Ensure timeout evidence can result in:
  - target repair when a generated test or implementation hang is likely
  - verifier command repair when command shape is wrong
  - safe stop when timeout is environment-level or ambiguous

Acceptance:

- Rust CLI smoke no longer ends as raw `transport_error`.
- Timeout safe stops include command, timeout class, project unit, and next
  user-action hint.
- No timeout path re-enters generic repo-change recovery while a repair job is
  active.

Implemented slice:

- Added `VerifierTimeoutKind` in `turn.rs`.
- Timeout verifier evidence now includes `timeout_kind`, redacted command, and
  a next-action hint.
- Structured Python dependency setup timeout is wrapped with dependency setup
  context before classification.
- `FailurePacket` now extracts `timeout_kind` into a typed
  `FailurePacketTimeoutKind` field and serializes it as structured diagnostic
  input.
- `RepairJob` now stores `timeout_kind` as typed state.
- `verifier_repair_context_from_failure()` extracts timeout kind while building
  the repair job.
- The duplicated timeout-kind enum in `turn.rs` has been removed; timeout
  classification, failure-packet payloads, and repair-state routing now share
  `FailurePacketTimeoutKind`.
- `RepairJob::next_action()` routes environment-stall timeout evidence to a
  verifier-timeout safe stop instead of patch repair.

Implemented in the final cleanup slice:

- Synthetic transition coverage now asserts that repairable timeout classes
  (`generated_test_hang`, `dependency_setup_timeout`, `build_command_timeout`,
  `long_running_verifier`, and `unknown_timeout`) continue into diagnostic
  repair instead of being prematurely safe-stopped.

## Phase 3: Repair Patch Convergence

Goal: avoid repeated invalid patch cycles and make every repair attempt
advance, replan, switch target, or safe stop.

Work:

- Promote repair proposal metadata into a single `RepairCandidate` shape:
  - accepted plan id / failure cluster id
  - target path
  - target role
  - allowed change kind
  - normalized patch fingerprint
  - rejection reason or verifier delta
- Require production patch repair to reference the current accepted
  `SemanticRepairPlan`.
- On repeated invalid proposals:
  - retry same target only with fresh evidence
  - re-diagnostic with invalid proposal summary
  - switch to next admitted target when available
  - safe stop with rejected proposal summary when no safe target remains
- Keep test edits blocked unless semantic authority explicitly allows them.

Acceptance:

- Synthetic tests cover repeated identical invalid patch, repeated no-op patch,
  same-target no-progress, target switch, and safe stop.
- A verifier-owned run cannot end in `missing_repo_edits`.
- A run cannot become green by weakening generated tests without accepted test
  authority.

## Phase 4: MissingVerifierJob Integration

Goal: keep bootstrap control first-class without becoming another parallel
dispatch source.

Work:

- Add an explicit bootstrap next-action enum comparable to `RepairNextAction`.
- Route missing-verifier actions through the same top-level
  `LoopControlAction` boundary.
- Decide whether missing-verifier bootstrap remains separate or becomes a
  `RepairJob` variant.

Acceptance:

- Missing-verifier bootstrap has transition-table tests equivalent to
  verifier repair.
- Generic retry cannot run while bootstrap owns the loop.
- Safe stop includes missing verifier reason and target setup evidence.

## Phase 5: Legacy Path Deletion Or Quarantine

Goal: keep production control flow understandable.

Work:

- Inventory remaining production references to old verifier repair decision
  helpers, deterministic repair helpers, and controller-applied patch shortcuts.
- Delete paths covered by transition-table and synthetic tests.
- For paths that remain, mark one of:
  - test-only compatibility
  - telemetry
  - validator assist
  - explicit user-selected fallback
- Add source guards preventing production dispatch from calling quarantined
  helpers.

Acceptance:

- There is one production verifier repair dispatch source:
  `LoopControlAction -> RepairJob/MissingVerifierJob next_action`.
- Legacy code cannot choose a repair target or decide verifier repair progress.
- Production source guards fail if FastAPI/ToDo/CRUD-specific repair authority
  re-enters the main path.

Implemented in the final cleanup slice:

- The deterministic candidate builder was removed rather than left as a
  quarantine block.
- Production patch repair still requires an accepted semantic repair context
  and validated `VerifierRepairIntent` edits.
- Source guards continue to assert that the production patch provider cannot
  call legacy deterministic repair authority.
- Repeated `AppliedImproved` outcomes on the same semantic target are now
  consumed by `RepairJob::next_action()` before it can request another patch
  for that exhausted target.
- Repair-terminal surfacing now distinguishes controlled repair exhaustion
  (`repair_exhausted`) from an uncontrolled raw verifier failure.

## Phase 6: Small Generality Smoke Gate

Goal: prove the controller is not tuned only to FastAPI CRUD before running
large evaluations.

Work:

- Maintain a fixed smoke set:
  - FastAPI CRUD API
  - Rust CLI
  - Python data script
  - Node utility
  - docs-only/no-code task
- Run no-PAM first, then PAM.
- For each run record:
  - artifacts generated
  - project unit selected
  - verifier command and timeout class
  - repair job transitions
  - terminal state
  - whether terminal state is acceptable

Acceptance:

- All smoke tasks end in `done` or actionable safe stop.
- No smoke task ends in verifier-owned `missing_repo_edits`.
- No smoke task reaches green through test weakening.
- Full 20/20 evaluation only starts after this gate is stable.

## Phase 7: Full Evaluation Gate

Run after Phases 1-6 pass.

Required checks:

1. `cargo fmt --check`
2. targeted project-unit / timeout / repair-job tests
3. `cargo test task_contract --lib`
4. `cargo test repair_job --lib`
5. `cargo test --lib`
6. `cargo clippy --all-targets -- -D warnings`
7. `cargo build --release`
8. no-PAM small smoke set
9. PAM small smoke set
10. no-PAM 20 / PAM 20 only after the small smoke set is stable

Final quality bar:

- FastAPI CRUD: at least 18/20 acceptable terminal states without PAM.
- FastAPI CRUD: at least 18/20 acceptable terminal states with PAM.
- Fixed non-FastAPI smoke set: 100% acceptable terminal states.
- No accepted run weakens tests without semantic authority.
- Failures are actionable safe stops, not uncontrolled retry exhaustion.

## Recommended Work Order

1. Implement `ProjectUnit` facts and tests. Done for current verifier
   entrypoints.
2. Route verifier timeout through structured timeout kinds. Done.
3. Keep repair proposal evidence in the accepted semantic plan +
   validated `VerifierRepairIntent` path; do not reintroduce deterministic
   candidate generation.
4. Tighten `RepairJob` transitions for repeated invalid patch proposals. Done
   for synthetic transition coverage.
5. Keep `MissingVerifierJob` first-class through its bootstrap next-action
   enum and `LoopControlAction` projection.
6. Delete/quarantine legacy verifier repair paths behind source guards. Done
   for deterministic candidate generation.
7. Run the small smoke gate. Partially done: FastAPI CRUD and Rust CLI passed;
   Python CSV analyzer reached controlled verifier repair but did not converge.
8. Run the full evaluation gate only after the small gate has no controller
   regressions.
