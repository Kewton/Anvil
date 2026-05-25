# Final Achievement Work Plan

## Purpose

Reach a structurally reliable local-LLM coding-agent flow.

Complete achievement does not mean forcing every run to green. It means every
run reaches one of these states through a single controlled path:

- `done`: owned artifacts are complete and a verifier passed.
- actionable safe stop: Anvil can explain why repair is unsafe, ambiguous,
  timed out, or budget-exhausted.

These terminal states remain unacceptable:

- verifier-owned `missing_repo_edits`.
- repeated generic retry while `RepairJob` or `MissingVerifierJob` owns the
  loop.
- green by weakening tests without accepted semantic authority.
- framework-specific repair branches that only work for FastAPI CRUD.

## Current Baseline

Verified controller work:

- `LoopControlAction` owns verifier repair dispatch.
- `RepairJob` / `MissingVerifierJob` owners block generic repo-change recovery,
  focused-edit recovery, and deterministic fallback.
- Synthetic verifier tests cover pass, malformed diagnostics, invalid patch,
  ambiguous authority, and missing-verifier bootstrap.
- Source guard prevents production dispatch from calling the legacy verifier
  repair decision bridge.

Empirical smoke baseline:

- FastAPI CRUD no-PAM: 4/5 `done`.
- FastAPI CRUD PAM live: 4/5 `done`.
- Rust CLI: verifier command timed out after 300s.
- Python data script: safe stop `patch_rejected_repeatedly`.
- Node utility: safe stop `patch_rejected_repeatedly`.

## Remaining Root Problems

1. Verifier environment isolation is incomplete.
   One failed no-PAM run imported code from
   `.anvil-state/verifier-python/site/...`, so verifier helper state can still
   leak into task runtime behavior.

2. Verifier execution is not yet project-unit aware enough.
   The Rust CLI generality smoke reached artifact completion but the verifier
   command timed out instead of producing a bounded, classified failure.

3. Repair patch convergence is still weak.
   Several runs stay inside `RepairJob`, but consume many iterations through
   invalid patch proposals before either passing or safe stopping.

4. Generality is not proven.
   The controller is phase-based, but verifier discovery and patch repair still
   behave better on the repeated FastAPI CRUD shape than on Rust/Python/Node
   generality tasks.

5. Legacy fallback code still exists.
   It is no longer the verifier dispatch authority, but it must stay visibly
   outside the main production path or be deleted after parity coverage exists.

## Phase 1: Verifier Environment Isolation

Goal: task code and verifier helper code must not contaminate each other.

Tasks:

- Audit all places that create or pass verifier Python paths, including
  `.anvil-state/verifier-python/site`.
- Add tests proving `.anvil-state` is excluded from:
  - artifact discovery
  - changed-file classification
  - owned artifact projection
  - test target selection
  - runtime `PYTHONPATH` for user tests, unless explicitly sandboxed
- Ensure verifier helper dependencies are not importable as task-local modules.
- Convert verifier-environment timeout or contamination into classified safe
  stop when isolation cannot be guaranteed.

Acceptance:

- No traceback from user verifier runs contains `.anvil-state/verifier-python`
  as the imported application dependency path.
- `.anvil-state` never appears as an owned implementation/test/docs artifact.
- Regression tests fail if `.anvil-state` re-enters artifact or verifier
  target selection.

## Phase 2: Project-Unit Verifier Selection

Goal: choose and run a verifier as a bounded project-unit operation, not as a
stack-specific guess.

Tasks:

- Define a small `ProjectUnit` model:
  - root path
  - manifest files
  - candidate verifier commands
  - expected timeout class
  - artifact roles contained in the unit
- Move verifier command selection behind this model.
- Add timeout classes for short unit tests, build commands, and dependency
  setup commands.
- Classify verifier timeout as:
  - environment timeout
  - long-running command mismatch
  - likely generated-test hang
  - unknown timeout
- Make timeout classification feed `RepairJob` or safe stop, not transport
  error when the agent has already produced artifacts.

Acceptance:

- Rust CLI smoke does not end as raw `transport_error` for verifier timeout.
- Timeout outcomes are classified and actionable.
- Verifier command selection is described by project facts, not framework
  literals.

## Phase 3: Repair Patch Convergence

Goal: fewer repeated invalid patch cycles, and clearer stop conditions when
repair is not converging.

Tasks:

- Track repair attempts by failure cluster, target path, allowed change kind,
  and normalized patch fingerprint.
- Escalate repeated invalid proposals through this order:
  - retry same target only while fresh evidence exists
  - re-diagnostic with the invalid patch reason included
  - switch to the next valid target in the accepted plan
  - safe stop with rejected proposal summary
- Require a patch proposal to be tied to the current accepted `RepairPlan`.
- Reject test edits unless the accepted plan carries explicit test-authority.
- Add synthetic tests for:
  - repeated identical invalid patch
  - repeated invalid patch with no new verifier delta
  - target switch after patch budget exhaustion
  - safe stop instead of late `verifier_failed`

Acceptance:

- `patch_rejected_repeatedly` stops are actionable and include the rejected
  target/change kind.
- Repeated invalid patch attempts cannot burn all iterations silently.
- Successful FastAPI runs need fewer repair cycles on average, without adding
  FastAPI-specific rules.

## Phase 4: Generality Smoke Before Scale

Goal: prove the flow is not tuned only to FastAPI CRUD.

Tasks:

- Maintain a small fixed smoke set:
  - FastAPI CRUD
  - Rust CLI
  - Python data script
  - Node utility
  - docs-only/no-code task
- For each run, classify:
  - artifact completion
  - verifier selected
  - verifier result
  - repair entered
  - terminal state
  - whether terminal state was acceptable
- Do not run 20/20 evaluation until the small smoke set has no verifier-control
  regressions.

Acceptance:

- All non-FastAPI tasks end in `done` or actionable safe stop.
- No generality task reaches verifier-owned `missing_repo_edits`.
- No production branch mentions FastAPI, ToDo, CRUD, or a specific generated
  app shape as repair authority.

## Phase 5: Legacy Path Deletion Or Quarantine

Goal: keep production control flow comprehensible.

Tasks:

- Search production code for legacy verifier decision helpers and deterministic
  repair authority.
- Delete legacy compatibility helpers once transition-table and synthetic tests
  cover their old behavior.
- If deletion is too risky, quarantine them as:
  - validator assist
  - telemetry
  - test-only migration seam
- Add source guards for the final forbidden paths.

Acceptance:

- There is one production verifier dispatch source:
  `LoopControlAction -> RepairJob/MissingVerifierJob next_action`.
- Legacy code cannot decide verifier repair progress.
- The source guards describe the intended architecture in test names.

## Phase 6: Evaluation Gate

Run in this order:

1. `cargo fmt --check`
2. targeted controller / repair / project-unit tests
3. `cargo test task_contract --lib`
4. `cargo test repair_job --lib`
5. `cargo test --lib`
6. `cargo clippy --all-targets -- -D warnings`
7. `cargo build --release`
8. no-PAM small smoke set
9. PAM small smoke set
10. no-PAM 20 / PAM 20 only after the small smoke set is stable

Final quality bar:

- FastAPI CRUD reaches at least 18/20 acceptable terminal states without PAM.
- FastAPI CRUD reaches at least 18/20 acceptable terminal states with PAM.
- The fixed non-FastAPI smoke set reaches 100% acceptable terminal states.
- No accepted run weakens tests without semantic authority.
- Failures are actionable safe stops, not uncontrolled retry exhaustion.

## Work Order

1. Implement Phase 1 first.
   Environment contamination can invalidate all verifier observations.

2. Implement Phase 2 next.
   Without project-unit verifier selection, generality evaluation will keep
   mixing command-selection bugs with repair-control bugs.

3. Implement Phase 3 after verifier evidence is trustworthy.
   Patch convergence should consume clean verifier deltas, not contaminated or
   timed-out verifier output.

4. Run Phase 4 small smoke.
   Stop and analyze if any run ends in uncontrolled failure.

5. Perform Phase 5 cleanup only after Phase 4 is stable.
   Deleting old paths before parity evidence risks losing fallback diagnostics.

6. Run Phase 6 full gate and commit.

