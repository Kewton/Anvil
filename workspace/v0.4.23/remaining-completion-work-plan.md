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

Remaining gaps:

- `ProjectUnit` now filters task-contract verifier selection when available,
  but broader verifier discovery still needs to move behind the same model.
- Timeout evidence is classified, but the classification is not yet attached to
  `FailurePacket` / `RepairJob` as a typed field.
- Repair patch convergence still depends on several old helper paths.
- Legacy deterministic repair is not fully deleted or telemetry-only.
- Generality across non-FastAPI tasks is not proven by a stable smoke gate.

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
  `AutoTestRunner`, and candidate selection is filtered to verifier sources
  admitted by that project unit.
- Unit coverage verifies that a parent/root Cargo manifest cannot steal
  verifier selection from a current Python task unit.

Not implemented yet:

- Make `ProjectUnit` the only verifier discovery model rather than an optional
  task-contract filter.
- Add docs-only/no-code project-unit behavior.
- Add confidence scoring beyond candidate source and evidence summary.

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

Not implemented yet:

- Attach timeout kind as a typed field on `FailurePacket` / `RepairJob`.
- Re-run Rust CLI smoke to confirm timeout no longer exits as raw
  `transport_error`.

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

1. Implement `ProjectUnit` facts and tests.
2. Route verifier timeout through structured timeout kinds.
3. Normalize repair proposal evidence into `RepairCandidate`.
4. Tighten `RepairJob` transitions for repeated invalid patch proposals.
5. Integrate or clearly quarantine `MissingVerifierJob`.
6. Delete/quarantine legacy verifier repair paths behind source guards.
7. Run the small smoke gate.
8. Run the full evaluation gate only after the small gate has no controller
   regressions.
