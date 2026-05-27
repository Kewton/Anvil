# v0.4.25 Work Plan: Complete Remaining Repair-Control Cleanup

## Purpose

This plan closes the remaining structural gaps in Anvil's local-LLM control
loop. The goal is not to add another heuristic repair branch. The goal is to
make task completion controlled, auditable, and maintainable.

The expected final behavior is:

1. `done` only when request-aligned artifacts exist and the correct verifier
   passes.
2. actionable safe stop when the task cannot be safely completed.
3. no false-positive `done`.
4. no uncontrolled verifier repair retry loop.
5. no legacy or deterministic recovery path overriding the active job owner.

## Current Gaps

### Functional Gaps

- Safe stop is controlled but not actionable enough.
- Repair convergence is still weak for objective verifier failures.
- Verifier delta is not yet the central signal for target switching,
  re-diagnostic, or safe stop.
- Ambiguous generated assertion expectations are rejected, but the final report
  does not explain the unresolved authority gap clearly enough.
- Generic artifact completion can still spend iterations on invalid or
  mis-targeted model turns.
- Mixed project shapes need broader regression coverage beyond the current
  smoke set.

### Structural Gaps

- `turn.rs` is still too large and owns too many responsibilities.
- `RepairJob` has become the conceptual center, but its driver, state,
  validation, admission, and reporting concerns are still partly entangled.
- Repair validation logic still lives substantially in `turn.rs`.
- Legacy/fallback paths remain numerous and are not all mechanically classified
  as production, assist, telemetry, or deletion candidates.
- Active job dispatch is improved but not yet fully enforced as a single source
  of truth.
- Some state is still represented by loose flags, strings, and `Option`
  combinations rather than typed transitions.
- Evaluation remains too manual and too expensive to use as a tight regression
  gate.

## Non-Goals

- Do not add FastAPI, ToDo, CRUD, Rust slug, or other business-specific
  templates.
- Do not make generated tests green by weakening assertions.
- Do not introduce provider abstraction.
- Do not replace the local-LLM loop with a large external orchestration system.
- Do not delete all legacy paths blindly before proving equivalent or safer
  behavior.

## Success Criteria

The work is complete only when all of the following are true:

- `turn.rs` no longer owns repair validation or verifier repair dispatch
  details.
- Verifier repair production dispatch goes through one active-job path.
- Legacy deterministic repair cannot run as a production verifier repair path.
- Patch validation rejects unsafe edits before apply, with typed reasons.
- Repeated invalid patch proposals transition through `RepairJob` to
  re-diagnostic, target switch, or actionable safe stop.
- Safe stop reports include concrete blocker class, affected files, failing
  assertions or diagnostics, authority status, and next user action.
- Wrong-stack verifier false positives remain blocked.
- Tests cover both successful repair and safe-stop paths.
- A small generic evaluation gate passes without false-positive `done`.

## Phase 0: Baseline And Guard Rails

Goal: freeze the current state and make later cleanup measurable.

Tasks:

- Record current `git status` and keep unrelated dirty files out of commits.
- Run and record:
  - `cargo fmt --check`
  - `cargo test --lib -q`
  - `cargo build --release`
- Add or update a short baseline note with:
  - current known success pattern
  - current known safe-stop pattern
  - current known repair-convergence gap
- Confirm `.anvil-state`, logs, and evaluation output are excluded from changed
  file and artifact evidence.

Deliverables:

- `workspace/v0.4.25/baseline.md`

Acceptance:

- Baseline is reproducible.
- No unrelated workspace files are staged.

## Phase 1: Dispatch Source Inventory And Deletion Plan

Goal: remove ambiguity about who owns the next action.

Tasks:

- Inventory every production call site in `turn.rs` that can:
  - continue artifact completion
  - continue missing verifier repair
  - continue verifier repair
  - invoke focused edit recovery
  - invoke generic retry
  - invoke deterministic fallback
  - return `done`
  - return `missing_repo_edits`
  - return `verifier_failed`
  - return `repair_exhausted`
  - return `repair_safe_stop`
- Classify each path as:
  - active production path
  - compatibility wrapper
  - validator assist
  - telemetry only
  - test only
  - deletion candidate
- Identify all production paths that bypass the active-job owner.
- Produce a removal order that never removes the only safe stop path before a
  replacement is tested.

Deliverables:

- `workspace/v0.4.25/dispatch-source-inventory.md`
- `workspace/v0.4.25/legacy-removal-plan.md`

Acceptance:

- Every verifier repair dispatch path has exactly one owner.
- Every remaining fallback path has an explicit allowed owner and expiry plan.

## Phase 2: Extract ActiveJobArbiter As Single Dispatch Source

Goal: make the loop ask one component what should happen next.

Design:

`ActiveJobArbiter` returns a typed `LoopControlAction`.

Priority:

1. terminal result / interrupt
2. active `RepairJob`
3. active `MissingVerifierJob`
4. active `ArtifactCompletionJob`
5. setup/bootstrap job
6. normal model turn

Tasks:

- Move active-job selection out of the middle of `turn.rs`.
- Make focused edit recovery and deterministic fallback consult the arbiter
  owner before they can run.
- Add tests proving:
  - repair job suppresses artifact completion
  - repair job suppresses focused edit recovery
  - repair job suppresses deterministic fallback
  - missing verifier job suppresses generic retry
  - stale flags without an active owner cannot dispatch repair
- Replace direct `turn.rs` dispatch checks with arbiter results.

Deliverables:

- `src/agent/loop_run/active_job.rs` or equivalent
- focused arbiter tests

Acceptance:

- No verifier repair production action is dispatched outside the arbiter-owned
  repair path.
- Generic retry cannot take over an active repair job.

## Phase 3: Split RepairJob Into State, Driver, Admission, Validation, Report

Goal: reduce the size and responsibility load of `RepairJob` and `turn.rs`.

Proposed modules:

- `repair_job/state.rs`
  - job state
  - event history
  - budgets
  - active cluster/target
- `repair_job/driver.rs`
  - `next_action`
  - event application
  - transition policy
- `repair_job/admission.rs`
  - accepts or rejects diagnostic repair plans
  - authority checks at plan level
- `repair_job/patch_validation.rs`
  - validates patch proposal before apply
  - detects unsafe edits, weakening, path mismatch, duplicate/noop edits
- `repair_job/report.rs`
  - actionable safe-stop and final repair summary

Tasks:

- Move `validate_verifier_repair_plan_admission` out of `turn.rs`.
- Move `validate_verifier_repair_intents*` out of `turn.rs`.
- Move duplicate-binding guard out of `turn.rs`.
- Move test weakening and implementation weakening detectors out of `turn.rs`.
- Keep `turn.rs` responsible only for calling the driver and applying validated
  edits.
- Add module-level tests for each extracted responsibility.

Deliverables:

- smaller repair modules
- reduced `turn.rs` repair section
- regression tests moved close to the logic they validate

Acceptance:

- `turn.rs` no longer contains the main patch safety validator.
- `RepairJob` state transition tests do not need to instantiate a full agent.

## Phase 4: Make VerifierDelta A First-Class Transition Input

Goal: repair should be guided by what changed after each patch.

Tasks:

- Define `VerifierDelta`:
  - previous failure fingerprint
  - current failure fingerprint
  - fixed failures
  - new failures
  - unchanged failures
  - verifier command and project unit
- Feed `VerifierDelta` into `RepairJob`.
- Add transition rules:
  - improvement: continue same plan or verify again
  - unchanged failure after valid patch: reduce target confidence or replan
  - same invalid patch pattern repeated: reject and switch target/re-diagnose
  - new unrelated failure: re-diagnostic
  - ambiguous assertion without authority: actionable safe stop
- Add tests for:
  - compile error fixed then assertion remains
  - import error unchanged after implementation edit
  - assertion expectation ambiguous
  - repeated invalid patch proposal
  - target switch after non-improving patch

Deliverables:

- `VerifierDelta` type
- `RepairJob` transition tests

Acceptance:

- Repair loop decisions are based on verifier delta, not only retry counters.

## Phase 5: Actionable Safe Stop Report

Goal: safe stop should be useful, not merely safer than a wrong edit.

Tasks:

- Define `SafeStopReport`:
  - blocker class
  - verifier command
  - project unit
  - affected files
  - observed failure summary
  - authority status
  - attempted actions
  - reason repair was not safe
  - next user action
- Render concise final messages from `SafeStopReport`.
- Include exact ambiguity examples for assertion failures without overquoting
  entire logs.
- Add tests for:
  - ambiguous generated test expectation
  - missing verifier/setup metadata
  - repeated invalid patch
  - unsafe test weakening
  - wrong-stack verifier rejection

Deliverables:

- `safe_stop_report` module or equivalent
- user-visible report rendering tests

Acceptance:

- `repair_exhausted` and `repair_safe_stop` always include actionable reason.

## Phase 6: Legacy And Deterministic Path Reduction

Goal: delete or demote paths that can override the new control model.

Tasks:

- Use the Phase 1 inventory to process each legacy/fallback path.
- For each path, choose one:
  - delete
  - test-only fixture
  - telemetry-only adapter
  - validator assist
  - explicit opt-in compatibility fallback
- Remove production access to legacy deterministic verifier repair.
- Ensure deterministic scaffold/fallback, where retained, is never completion
  evidence by itself.
- Add tests that legacy fallback cannot run while:
  - `RepairJob` is active
  - `MissingVerifierJob` is active
  - artifact completion has a required target

Deliverables:

- updated legacy inventory
- deleted or isolated fallback paths

Acceptance:

- No production repair patch is created by legacy deterministic repair.
- No legacy fallback can make the task `done`.

## Phase 7: Artifact Evidence And Done Gate Hardening

Goal: make `done` harder than safe stop.

Tasks:

- Audit artifact evidence sources:
  - implementation
  - tests
  - usage docs
  - setup/config
  - verifier
- Require evidence to be:
  - request-aligned
  - project-unit aligned
  - path-confined
  - non-placeholder
  - produced or modified in this task context unless explicitly accepted as
    existing relevant artifact
- Preserve support for multi-directory applications by using project units
  rather than single-file assumptions.
- Add tests for:
  - multi-directory Python app
  - Rust library
  - Node package
  - docs-only task
  - existing files in unrelated subdirectories
  - `.anvil-state` and evaluation files ignored

Deliverables:

- hardened done gate tests
- artifact evidence tests

Acceptance:

- Wrong stack or wrong project-unit verifier cannot produce `done`.
- Placeholder tests/docs cannot satisfy required roles.

## Phase 8: Generic Evaluation Harness

Goal: replace ad hoc manual evaluation with a repeatable smoke gate.

Tasks:

- Define a small case matrix:
  - Python/FastAPI CRUD API
  - Rust library
  - Node CLI/package
  - docs-only documentation task
  - existing-project modification
  - ambiguous assertion repair case
  - missing verifier/setup case
  - multi-directory app case
- For each case record:
  - prompt
  - expected terminal class: `done` or actionable safe stop
  - forbidden outcomes
  - verifier command expectation
  - artifact roles expected
- Build a runner script or documented command sequence that creates isolated
  directories and captures result summaries.
- Keep PAM and no-PAM runs separate.
- Add a small default gate:
  - no-PAM 5 cases
  - PAM 5 cases
- Add expanded gate:
  - no-PAM 20 cases
  - PAM 20 cases

Deliverables:

- `workspace/v0.4.25/evaluation-matrix.md`
- optional evaluation runner script if it stays simple and local

Acceptance:

- Evaluation result is reproducible and comparable across commits.
- False-positive `done` is tracked as a critical failure.

## Phase 9: Full Verification And Regression Evaluation

Goal: prove the cleanup did not regress core behavior.

Required checks:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --lib -q`
- `cargo build --release`

Evaluation:

1. Run no-PAM 5-case smoke.
2. Run PAM 5-case smoke.
3. If no critical failures:
   - run no-PAM 20-case evaluation
   - run PAM 20-case evaluation
4. Summarize:
   - success rate
   - actionable safe-stop rate
   - false-positive done count
   - verifier mismatch count
   - repair loop exhaustion count
   - average iteration count
   - top residual failure classes

Deliverables:

- `workspace/v0.4.25/evaluation-results.md`

Acceptance:

- Zero false-positive `done`.
- Zero wrong-stack verifier success.
- Verifier repair failures end as actionable safe stop, not generic
  `missing_repo_edits`.
- Any remaining non-completion has a classified reason.

## Phase 10: Final Cleanup And Commit Hygiene

Goal: leave the repository reviewable.

Tasks:

- Split commits by concern if the diff becomes too large:
  1. arbiter / dispatch ownership
  2. repair module extraction
  3. verifier delta and safe stop report
  4. legacy path deletion/demotion
  5. evaluation docs/harness
- Keep unrelated files out of commits:
  - `.claude/scheduled_tasks.lock`
  - unrelated `.agents/skills/*`
  - unrelated scripts
- Re-run final verification after the last commit.
- Update this README with final status and residual risk.

Acceptance:

- `git status --short` contains only known unrelated files or is clean.
- Final report states what is complete, what remains, and why.

## Risk Register

### Risk: Over-tightening causes too many safe stops

Mitigation:

- Treat safe stop as acceptable only if actionable.
- Use verifier delta to continue objective repairs when authority is clear.

### Risk: Removing legacy paths regresses currently working flows

Mitigation:

- Classify and test before deletion.
- Keep compatibility wrappers only when they cannot override active jobs.

### Risk: Modularization changes behavior accidentally

Mitigation:

- Extract with characterization tests first.
- Keep pure functions where possible.

### Risk: Evaluation becomes too expensive

Mitigation:

- Maintain a small 5+5 gate for development.
- Run 20+20 only after structural changes pass unit tests.

### Risk: More modules hide complexity rather than reduce it

Mitigation:

- Each module must have one explicit responsibility.
- Avoid creating pass-through wrappers with no ownership.
- Delete old code as new ownership becomes covered by tests.

## Work Order

Recommended sequence:

1. Phase 0 baseline.
2. Phase 1 inventory.
3. Phase 2 active-job arbiter.
4. Phase 3 repair extraction.
5. Phase 4 verifier delta.
6. Phase 5 safe-stop report.
7. Phase 6 legacy reduction.
8. Phase 7 done gate hardening.
9. Phase 8 evaluation harness.
10. Phase 9 verification and evaluation.
11. Phase 10 cleanup and commit hygiene.

Do not start expanded 20+20 evaluation until Phases 2 through 7 have targeted
tests. Otherwise the result will be expensive but not diagnostic.

## Definition Of Complete

This v0.4.25 cleanup is complete when:

- repair dispatch ownership is singular and tested;
- unsafe or ambiguous repair cannot mutate the repo indefinitely;
- objective repair can still proceed when evidence is sufficient;
- legacy production repair paths are deleted or explicitly isolated;
- `done` requires request/project/verifier alignment;
- safe stop is actionable;
- generic evaluation has no false-positive completion.

## Implementation Log

### 2026-05-26 Slice 1

Applied:

- Moved the pre-model loop-control vocabulary out of `turn.rs` and into
  `active_job_arbiter.rs`:
  - `LoopControlAction`
  - `LoopControlInputs`
  - `RecoveryOwner`
  - `determine_loop_control_action`
  - missing-verifier setup ownership helper
- Extracted safe-stop payload rendering into `safe_stop_payload.rs`.
- Added actionable safe-stop payload fields:
  - `blocker_class`
  - `authority_status`
  - `next_user_action`
- Added v0.4.25 planning artifacts:
  - `baseline.md`
  - `dispatch-source-inventory.md`
  - `legacy-removal-plan.md`
  - `evaluation-matrix.md`

Verification:

- `cargo test loop_control_action_tests --lib -q`: pass
- `cargo test build_safe_stop_payload --lib -q`: pass
- `cargo test repair_rejection_next_action --lib -q`: pass
- `cargo test --lib -q`: pass, 2989 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass

Evaluation:

- no-PAM smoke `Python CSV CLI`: controlled `repair_safe_stop`, no
  false-positive `done`, no generic retry takeover.
- The smoke exposed weak console actionability for `role_mismatch`; the
  rejection terminal message now appends a concrete `next_action`.

Assessment:

- This slice reduces `turn.rs` by moving dispatch vocabulary and safe-stop
  report formatting out of the actor loop file.
- It does not yet complete the entire v0.4.25 plan. The remaining high-value
  work is repair validation extraction, legacy path deletion/demotion, and
  generic evaluation.

### 2026-05-26 Slice 2

Applied:

- Extracted verifier repair plan admission from `turn.rs` into
  `repair_plan_admission.rs`.
- Kept the production contract unchanged:
  - diagnostic/legacy brief input is still only a proposal;
  - `FailurePacket` + authority evidence must accept it;
  - admission rejection maps back into `RepairJobEvent`.
- Added module-level admission tests:
  - missing assessment forces re-diagnostic;
  - ambiguous authority maps to an authority safe-stop event;
  - malformed action-level rejection maps to re-diagnostic.
- Added arbiter owner-gate test proving:
  - `RepairJob` and `MissingVerifierJob` deny generic retry, focused edit, and
    deterministic fallback;
  - artifact completion allows only focused edit recovery;
  - no active owner is the only state that allows generic/deterministic
    fallback.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_plan_admission --lib -q`: pass
- `cargo test recovery_owner_gates_lower_level_fallbacks --lib -q`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib -q`: pass, 2993 tests when rerun outside sandbox
- `cargo build --release`: pass

Sandbox note:

- A sandboxed full lib test run failed because `mockito` could not bind its
  local test server (`Operation not permitted`). The same command passed with
  normal permissions.

Assessment:

- This slice moves another repair-control responsibility out of `turn.rs`
  without adding a new recovery branch.
- Legacy deterministic repair remains outside the production patch provider,
  but direct `recovery_owner.allows_*` checks are still scattered in `turn.rs`.
  The next cleanup should consolidate those checks behind a small dispatch
  wrapper before deleting more legacy branches.
- A non-FastAPI no-PAM Rust library smoke reached verified `done` after
  missing-verifier setup and one controller repair. This is a useful signal
  that the current changes are not only tuned to the original CRUD case.

### 2026-05-26 Slice 3

Applied:

- Added `repair_patch_validation.rs` for pure patch-admission checks.
- Moved the accepted-plan target authorization logic out of `turn.rs`:
  - target path match;
  - target role match;
  - insufficient-evidence rejection;
  - ambiguous authority rejection for behavior-changing repairs.
- Kept a thin `turn.rs` wrapper only to translate the typed rejection into the
  existing `ValidationFailure` shape used by the patch-apply pipeline.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass
- `cargo test accepted_plan --lib -q`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2996 tests when rerun outside sandbox

Assessment:

- This is intentionally not a full extraction of
  `validate_verifier_repair_intents_inner`; that function still depends on
  filesystem checks, cheap project verification, weakening detectors, duplicate
  binding guards, and legacy test helpers.
- The safer cleanup path is to keep extracting pure admission/validation
  sub-decisions first, then move the filesystem-heavy validator after each
  sub-decision has tests in its own module.

### 2026-05-27 Slice 4

Applied:

- Added `RecoveryDispatchGate` in `active_job_arbiter.rs`.
- Production recovery/fallback branches now consult the gate instead of calling
  `RecoveryOwner::allows_*` directly.
- Moved pure repair-intent input checks into `repair_patch_validation.rs`:
  - safe workspace-relative path input;
  - empty/no-op edit rejection;
  - total edit byte cap;
  - tool/markdown markup rejection;
  - secret introduction rejection;
  - suspicious shell payload rejection outside shell-capable files.
- Left filesystem-bound checks in `turn.rs` for now:
  - canonical path resolution;
  - file size/read checks;
  - exact edit application;
  - cheap project verifier;
  - weakening and duplicate-binding detectors.

Verification:

- `cargo fmt --check`: pass
- `cargo test loop_control --lib -q`: pass, 5 tests
- `cargo test recovery_owner_gates_lower_level_fallbacks --lib -q`: pass
- `cargo test repair_patch_validation --lib -q`: pass
- `cargo test validate_verifier_repair_intents --lib -q`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2987 tests when rerun outside sandbox

Assessment:

- This reduces policy drift without changing the repair flow.
- Loop-control tests now live with the arbiter instead of `turn.rs`.
- The remaining large extraction should focus on the filesystem-bound patch
  validator and weakening detector dispatch. Those need characterization tests
  before moving because they encode many safety decisions.

### 2026-05-27 Slice 5

Applied:

- Moved the duplicate-binding repair guard out of `turn.rs` and into
  `repair_patch_validation.rs`.
- Kept the production rejection shape unchanged by mapping the typed
  `DuplicateBindingRepairError` back to `RepairRejectionSignal::Duplicate` at
  the `turn.rs` boundary.
- Added module tests for:
  - rejecting unresolved duplicate bindings;
  - accepting a candidate that reduces the duplicate binding.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 8 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2987 tests when rerun outside sandbox

Assessment:

- This is another pure validation extraction, not a new recovery heuristic.
- `turn.rs` still owns the filesystem-bound patch validator and the
  test/implementation weakening detector dispatch. Those remain the next
  cleanup targets.

### 2026-05-27 Slice 6

Applied:

- Moved filesystem-bound patch target validation into
  `repair_patch_validation.rs`:
  - selected target path safety;
  - workspace canonicalization;
  - target file existence and size checks;
  - UTF-8 target read;
  - intent path to selected target matching.
- Moved cheap candidate-content validation into `repair_patch_validation.rs`.
  `turn.rs` now maps the typed module result back into `CheapCheckOutcome`.
- Added module tests for:
  - reading a safe workspace target snapshot;
  - rejecting unsafe and oversized targets;
  - rejecting intent paths that do not match the selected target;
  - deferring whitespace-sensitive fallback when no safe cheap verifier exists.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 13 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2992 tests when rerun outside sandbox

Assessment:

- The patch validation boundary now owns pure checks, target IO admission, and
  cheap candidate-content checking.
- `turn.rs` still owns candidate application and weakening detector dispatch.
  Those are the remaining major repair-validation responsibilities in the
  actor loop.

### 2026-05-27 Slice 7

Applied:

- Moved in-memory repair candidate application into
  `repair_patch_validation.rs`:
  - exact-once edit application;
  - whitespace-normalized fallback;
  - bounded `replace_all`;
  - exact-edit error rendering with bounded old-string excerpt.
- Removed the now-unused application result type and replace-all cap constant
  from `turn.rs`.
- Added module tests for:
  - exact plus replace-all application;
  - exact-edit failure reporting with old-string excerpt.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 15 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2994 tests when rerun outside sandbox

Assessment:

- `turn.rs` no longer owns repair target IO, cheap content checks, duplicate
  binding guard, or candidate application internals.
- The remaining repair-validation responsibility in `turn.rs` is mainly
  weakening detector dispatch plus the high-level assembly of the validated
  edit result.

### 2026-05-27 Slice 8

Applied:

- Moved weakening rejection message/metadata construction into
  `repair_patch_validation.rs`.
- `turn.rs` still runs the detector branches, but now delegates the
  non-empty weakening-pattern rejection shape to the patch-validation module.
- Added a module test proving weakening rejection preserves:
  - rejection kind;
  - first weakening pattern;
  - legacy-compatible message text.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 16 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2995 tests when rerun outside sandbox

Assessment:

- This is a conservative step toward moving weakening validation out of
  `turn.rs` without duplicating Python diagnostic helpers.
- The remaining extraction should either move shared Python diagnostic helpers
  to a small helper module first, or keep detector dispatch in `turn.rs` and
  only move the final validated-edit assembly.

### 2026-05-27 Slice 9

Applied:

- Moved `ValidatedVerifierRepairEdit` into `repair_patch_validation.rs`.
- Added a constructor that computes preimage/postimage hashes inside the
  patch-validation boundary.
- `turn.rs` now imports the validated edit type instead of defining it
  locally.
- Added a module test that pins constructor hashing and field preservation.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 17 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2996 tests when rerun outside sandbox

Assessment:

- The patch-validation module now owns the validated edit carrier and its
  hashes. `turn.rs` still applies the validated edit to disk and orchestrates
  weakening detector dispatch.
- The next meaningful cleanup is to decide whether the Python diagnostic
  helpers should be extracted into a shared module before moving weakening
  detector dispatch.

### 2026-05-27 Slice 10

Applied:

- Added `repair_patch_executor.rs` as the execution boundary for already
  validated verifier-repair edits.
- Moved disk apply logic out of `turn.rs`:
  - read current target;
  - verify preimage hash has not changed;
  - write validated contents.
- Kept target selection, authority, path validation, and edit construction out
  of the executor. The executor only applies a previously validated edit.
- Added executor module tests for:
  - successful apply when preimage matches;
  - rejection when preimage changed;
  - read failure reporting.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_executor --lib -q`: pass, 3 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo test verifier_repair_apply_rejects_changed_preimage --lib -q`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2999 tests when rerun outside sandbox

Assessment:

- `turn.rs` no longer owns patch apply mechanics; it orchestrates when the
  executor is called.
- The remaining `turn.rs` repair-validation responsibility is primarily the
  weakening detector dispatch and the high-level sequence inside
  `validate_verifier_repair_intents_inner`.

### 2026-05-27 Slice 11

Applied:

- Moved repair intent fingerprint generation into
  `repair_patch_validation.rs`.
- Production validation now builds normalized edit payloads once and reuses
  them for:
  - duplicate-intent fingerprinting;
  - in-memory candidate application.
- Kept old `turn.rs` fingerprint helper as a test-only compatibility wrapper.
- Added a module test proving fingerprint stability and distinction between
  exact-once and replace-all modes.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 18 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3000 tests when rerun outside sandbox

Assessment:

- Duplicate-intent detection now belongs to the patch-validation boundary,
  alongside edit payload normalization and application.
- `turn.rs` still owns the orchestration sequence and weakening detector
  dispatch.

### 2026-05-27 Slice 12

Applied:

- Moved the final post-apply no-op candidate check into
  `repair_patch_validation.rs`.
- Added a typed `RepairCandidateNoopError` so `turn.rs` only maps the pure
  validation result into the legacy `ValidationFailure` carrier.
- Added a module test anchoring the no-op rejection message and accepting real
  content changes.

Verification:

- `cargo fmt --check`: pass after formatting
- `cargo test repair_patch_validation --lib -q`: pass, 19 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3001 tests when rerun outside sandbox

Assessment:

- Patch validation now owns another pure candidate-admission rule.
- `turn.rs` still owns the high-level validation sequence, semantic test-edit
  gate, and weakening detector dispatch.

### 2026-05-27 Slice 13

Applied:

- Moved duplicate repair-intent replay detection into
  `repair_patch_validation.rs`.
- Kept fingerprint construction in the validation boundary and changed the
  replay check to accept the existing applied-intent history as a read-only
  slice rather than requiring a specific collection type.
- Added a module test for replay rejection and non-replayed acceptance.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 20 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3002 tests when rerun outside sandbox

Assessment:

- Duplicate-intent admission is now validation-owned end to end.
- `turn.rs` still maps the typed duplicate error into the existing
  `RepairRejectionSignal::Duplicate` carrier.

### 2026-05-27 Slice 14

Applied:

- Moved the parsed repair patch intent carrier `VerifierRepairIntent` from
  `turn.rs` into `repair_patch_validation.rs`.
- Kept fields crate-internal to the loop-run module boundary so parsing,
  validation, and existing tests can still use the same shape without exposing
  it as a public API.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 20 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3002 tests when rerun outside sandbox

Assessment:

- Patch-candidate shape ownership is now closer to the patch-validation
  boundary.
- `turn.rs` still owns patch proposal parsing and high-level validation
  orchestration.

### 2026-05-27 Slice 15

Applied:

- Moved patch proposal parsing caps, patch-proposal error mapping, and
  `PatchProposal -> VerifierRepairIntent` conversion into
  `repair_patch_validation.rs`.
- Left `turn.rs` with thin wrappers that only pass the configured verifier
  repair limits.
- Preserved existing repair-intent reason compaction behavior, including
  secret masking, whitespace collapse, and UTF-8-safe ellipsis truncation.
- Added module tests for bounded conversion and output-cap rejection.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 22 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3004 tests when rerun outside sandbox

Assessment:

- Patch proposal shaping logic is now owned by the patch-validation boundary.
- `turn.rs` still owns the high-level validation wrapper, semantic test-edit
  gate, and weakening detector dispatch.

### 2026-05-27 Slice 16

Applied:

- Updated the production verifier repair pass to call
  `repair_patch_validation::{parse_verifier_repair_patch_proposal_reply,
  patch_proposal_to_verifier_repair_intents}` directly.
- Kept the old `turn.rs` parse wrappers as `#[cfg(test)]` compatibility
  helpers for existing tests only.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 22 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3004 tests when rerun outside sandbox

Assessment:

- Production patch proposal shaping no longer flows through `turn.rs`
  wrappers.
- `turn.rs` still supplies configured limits and orchestrates the accepted
  proposal through shadow validation and repair-intent validation.

### 2026-05-27 Slice 17

Applied:

- Moved the pure test-edit `SemanticRepairPlan` gate into
  `repair_patch_validation.rs`.
- Kept the validation module independent of `RepairJob` internals by passing
  only:
  - whether the target is a test file;
  - whether an accepted repair plan is already present;
  - the optional repair hypothesis string.
- Left Python import-contract checks in `turn.rs`; those still depend on
  shared Python helper functions and should be extracted separately.
- Added module tests for missing plan, empty hypothesis, accepted plan bypass,
  non-test bypass, and valid hypothesis.

Verification:

- `cargo fmt --check`: pass after formatting
- `cargo test repair_patch_validation --lib -q`: pass, 23 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3005 tests when rerun outside sandbox

Assessment:

- The semantic gate is now a typed patch-admission rule rather than inline
  actor-loop logic.
- `turn.rs` still owns Python import-contract checks and weakening detector
  dispatch.

### 2026-05-27 Slice 18

Applied:

- Moved test import-contract evidence admission into
  `repair_patch_validation.rs`.
- `turn.rs` still gathers Python-specific evidence through existing helper
  functions, but no longer owns the reject priority or message construction
  for:
  - missing local modules;
  - missing local import symbols;
  - attribute access assumptions on imported scalar local symbols.
- Added module tests for rejection priority, each message shape, and the empty
  evidence success case.

Verification:

- `cargo fmt --check`: pass after formatting
- `cargo test repair_patch_validation --lib -q`: pass, 26 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3008 tests when rerun outside sandbox

Assessment:

- Another test-edit admission rule is validation-owned.
- Python evidence collection remains in `turn.rs` until the Python diagnostic
  helpers are moved to a dedicated module.

### 2026-05-27 Slice 19

Applied:

- Moved per-intent target-path validation, text-payload validation, edit-byte
  accounting, and edit-payload construction into
  `repair_patch_validation.rs`.
- Added `RepairIntentPayloadValidationError` so `turn.rs` can preserve the
  existing malformed/no-op signal mapping without owning the loop.
- Marked the old `turn.rs` payload builder as test-only compatibility for
  fingerprint helper tests.
- Added module tests for successful payload construction and preservation of
  input error kind.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 28 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3010 tests when rerun outside sandbox

Assessment:

- Phase-1 repair-intent validation is now validation-owned.
- `turn.rs` still owns the high-level sequence, Python evidence gathering,
  and weakening detector dispatch.

### 2026-05-27 Slice 20

Applied:

- Moved repair-intent list bounds validation into
  `repair_patch_validation.rs`.
- `turn.rs` now maps the typed bounds error into the legacy
  `ValidationFailure` carrier instead of owning the empty/too-many branches.
- Added a module test for empty, too-many, and accepted bounds.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 29 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3011 tests when rerun outside sandbox

Assessment:

- The remaining `validate_verifier_repair_intents_inner` validation logic in
  `turn.rs` is mostly orchestration, Python evidence gathering, and weakening
  detector dispatch.

### 2026-05-27 Slice 21

Applied:

- Moved test/implementation weakening detector dispatch into
  `repair_patch_validation.rs`.
- `turn.rs` still applies the test-specific observed-assert-update filter
  because that filter depends on `RepairJob` semantic context and Python
  helper functions.
- Added module tests for test-path dispatch and non-code path no-op behavior.

Verification:

- `cargo fmt --check`: pass after formatting
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3013 tests when rerun outside sandbox

Assessment:

- File-kind dispatch for weakening detection is now validation-owned.
- The remaining actor-loop responsibility is the semantic test weakening
  filter and mapping typed weakening metadata into the legacy
  `ValidationFailure` carrier.

### 2026-05-27 Slice 22

Applied:

- Moved verifier repair validation error carriers into
  `repair_patch_validation.rs`:
  - `CheapCheckOutcome`
  - `ValidationWeakening`
  - `RepairRejectionSignal`
  - `ValidationFailure`
- `turn.rs` now imports those typed carriers and keeps only the ledger/report
  conversion logic.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3013 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Patch validation now owns the validation result
  shape as well as most typed admission checks.
- The remaining actor-loop responsibility is still the high-level orchestration,
  Python evidence gathering, semantic test weakening filter, and conversion
  from validation result to repair attempt ledger/reporting.

### 2026-05-27 Slice 23

Applied:

- Moved the pure `build_verifier_repair_pass_ledger_outcome` conversion into
  `repair_patch_validation.rs`.
- `turn.rs` still decides when to record the outcome, but no longer owns the
  validation-signal-to-ledger-outcome mapping.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo test phase3_repair_pass_clears_stale_unsafe_outcome_between_attempts --lib -q`: pass, 1 test
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3013 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Validation-owned signals now convert to
  validation-owned ledger outcomes before `turn.rs` records them.
- The remaining actor-loop responsibility is narrower: orchestration, Python
  evidence gathering, semantic test weakening filter, repair lifecycle event
  recording, and final report wiring.

### 2026-05-27 Slice 24

Applied:

- Moved repair-attempt outcome to `RejectedAttemptReason` projection into
  `repair_job.rs`.
- Added a focused unit test covering every current `RepairAttemptOutcomeKind`
  branch.

Verification:

- `cargo fmt --check`: pass
- `cargo test rejected_reason_projection_matches_repair_attempt_outcomes --lib -q`: pass, 1 test
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3014 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` no longer owns this repair lifecycle
  projection; it still records lifecycle events and handles string-based
  fallback error classification.

### 2026-05-27 Slice 25

Applied:

- Moved string-only repair error to lifecycle event classification into
  `repair_job.rs`.
- Moved the two focused tests for unknown invalid patch errors and provider
  timeout errors from `turn.rs` to `repair_job.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test unknown_invalid_patch_error_is_still_budgeted --lib -q`: pass, 1 test
- `cargo test timeout_invalid_patch_error_is_budgeted_as_provider_timeout --lib -q`: pass, 1 test
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3014 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` no longer owns repair lifecycle event
  classification for invalid patch errors; it still decides when to apply the
  returned event to the active job.

### 2026-05-27 Slice 26

Applied:

- Moved `ValidationFailure` telemetry reason-label projection into
  `repair_patch_validation.rs`.
- Removed the remaining `turn.rs` helper that reinterpreted validation failure
  fields for logging.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3014 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Validation-owned error data now also owns its
  stable telemetry label.

### 2026-05-27 Slice 27

Applied:

- Moved repair-intent typed error to `ValidationFailure` conversion into
  `repair_patch_validation.rs`.
- Added a validation-owned wrapper for accepted repair-plan target
  authorization that returns `ValidationFailure` directly.
- Removed the corresponding conversion wrappers from `turn.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3014 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` now receives `ValidationFailure`
  directly from validation-owned typed error conversions.

### 2026-05-27 Slice 28

Applied:

- Moved remaining typed validation error conversions into
  `repair_patch_validation.rs`:
  - list bounds
  - target read errors
  - duplicate/no-op candidate errors
  - test edit plan and import-contract errors
  - weakening errors
  - duplicate binding errors
  - candidate content cheap-check projection
- `turn.rs` now wires those conversions instead of constructing
  `ValidationFailure` inline for each branch.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 31 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3014 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` no longer owns typed validation error
  shaping; its remaining validation responsibilities are orchestration,
  Python evidence gathering, and semantic generated-test weakening filtering.

### 2026-05-27 Slice 29

Applied:

- Added `repair_assertion_analysis.rs` for pure assertion/output parsing used
  by verifier repair diagnostics and semantic generated-test filtering.
- Moved assertion delta helpers, observed assert-pair parsing, and the pytest
  shared-state-leak signal into that module.
- Added focused unit tests for assertion delta handling and observed pytest
  output parsing.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_assertion_analysis --lib -q`: pass, 3 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3017 tests when rerun outside sandbox

Assessment:

- No behavior change intended. This does not move the semantic weakening
  decision itself yet; it removes pure assertion parsing from `turn.rs` first
  so the eventual semantic filter extraction has a smaller dependency surface.

### 2026-05-27 Slice 30

Applied:

- Added `repair_python_import_evidence.rs` for bounded Python local
  import-contract evidence used by verifier repair validation.
- Moved missing local module, missing imported symbol, and imported scalar
  attribute-assumption evidence collection out of `turn.rs`.
- Kept filesystem probing confined to `work_root`, with caller-supplied file
  size limits, and added focused unit tests for the evidence collector.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_python_import_evidence --lib -q`: pass, 3 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3020 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` still sequences verifier repair
  validation, but Python import-contract probing is no longer embedded in the
  dispatcher.

### 2026-05-27 Slice 31

Applied:

- Added `repair_python_test_analysis.rs` for Python pytest/test-fragment
  analysis shared by verifier framework diagnostics and semantic test-repair
  validation.
- Moved local mutable fixture detection, disconnected fixture assertion
  detection, and Python identifier-boundary line matching out of `turn.rs`.
- Updated framework diagnostic and semantic weakening-filter call sites to use
  the shared module.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_python_test_analysis --lib -q`: pass, 3 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3023 tests when rerun outside sandbox

Assessment:

- No behavior change intended. This reduces duplicated Python test analysis in
  the actor loop and narrows the remaining semantic weakening filter
  dependency surface.

### 2026-05-27 Slice 32

Applied:

- Added `repair_test_weakening_filter.rs` for semantic generated-test
  weakening admission.
- Moved expected-literal, disconnected-fixture-observation, and
  test-only-missing-import-symbol allowance checks out of `turn.rs`.
- Left `turn.rs` responsible only for invoking the filter after the generic
  weakening detector has produced patterns.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_test_weakening_filter --lib -q`: pass, 2 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3025 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Semantic test-repair authority checks now live
  behind a dedicated module boundary instead of being embedded in verifier
  repair validation orchestration.

### 2026-05-27 Slice 33

Applied:

- Added `repair_framework_findings.rs` for objective verifier framework /
  test-runner findings used by the diagnostic prompt and post-parse override.
- Moved pytest lifecycle, pytest setup/import/state-isolation, and cargo
  integration-test crate-import finding generation out of `turn.rs`.
- Kept parsed assessment override application in `turn.rs` for this slice so
  the diagnostic schema and high-level orchestration boundary remain stable.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_framework_findings --lib -q`: pass, 2 tests
- `cargo test framework_finding --lib -q`: pass, 11 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Framework finding generation is now a bounded
  evidence module; `turn.rs` still decides how that evidence affects the
  parsed verifier assessment.

### 2026-05-27 Slice 34

Applied:

- Added `verifier_assessment_parser.rs` for diagnostic LLM assessment JSON
  extraction, schema-shape tolerance, legacy repair-target parsing, and
  diagnostic failure-kind to verifier failure-type mapping.
- Moved `ParsedVerifierRepairAssessment` / `ParsedVerifierRepairTarget` out
  of `turn.rs`.
- Reused the extracted JSON boundary for semantic failure report parsing so
  legacy and semantic diagnostic paths continue to share one extraction rule.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. `turn.rs` now receives a parsed diagnostic
  assessment and remains responsible for workspace admission and repair-state
  orchestration, while parser-specific schema drift handling is isolated.

### 2026-05-27 Slice 35

Applied:

- Moved framework-finding post-parse override logic into
  `verifier_assessment_parser.rs`.
- Kept framework finding generation in `repair_framework_findings.rs` and
  prompt/file-excerpt assembly in `turn.rs`.
- Left `turn.rs` with a single call that applies bounded framework evidence
  to the parsed diagnostic assessment before workspace admission.

Verification:

- `cargo fmt --check`: pass
- `cargo test framework_finding --lib -q`: pass, 11 tests
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Parsed assessment normalization and evidence
  override now live with the diagnostic parser, reducing another
  non-dispatch branch from `turn.rs`.

### 2026-05-27 Slice 36

Applied:

- Moved semantic failure report parsing from diagnostic LLM replies into
  `verifier_assessment_parser.rs`.
- Kept semantic fallback synthesis and `SemanticRepairPlan` construction in
  `turn.rs` because they still combine admitted targets, active request
  authority, and RepairJob generation state.

Verification:

- `cargo fmt --check`: pass
- `cargo test semantic_failure --lib -q`: pass, 40 tests
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Both legacy assessment parsing and semantic
  report parsing now share the same diagnostic JSON extraction boundary
  outside the actor loop.

### 2026-05-27 Slice 37

Applied:

- Added `semantic_repair_planning.rs`.
- Moved legacy assessment to `SemanticFailureReport` synthesis, admitted
  assessment fallback synthesis, spec-authority input construction, and
  `SemanticRepairPlan` construction out of `turn.rs`.
- Kept `turn.rs` responsible for diagnostic pass sequencing, workspace
  admission, report enrichment, and RepairJob state writes.

Verification:

- `cargo fmt --check`: pass
- `cargo test semantic_failure --lib -q`: pass, 40 tests
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Semantic repair planning is now a dedicated
  bridge module; the actor loop no longer owns the value-construction details
  for `SemanticFailureReport` / `SemanticRepairPlan`.

### 2026-05-27 Slice 38

Applied:

- Added `verifier_repair_shadow.rs`.
- Moved verifier-repair shadow telemetry payload construction,
  `FailurePacket` projection for shadow comparison, repair-action payload
  projection, and legacy diagnostic brief projection out of `turn.rs`.
- Kept `turn.rs` responsible only for choosing when to emit the shadow event.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo test semantic_failure --lib -q`: pass, 40 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Observational payload shaping is no longer
  embedded in the actor loop dispatcher.

### 2026-05-27 Slice 39

Applied:

- Moved legacy target merge and admitted-target role-priority sorting into
  `semantic_repair_planning.rs`.
- Kept `enrich_failure_clusters_with_admitted_targets` in `turn.rs` because
  it still intentionally verifies the admission SSOT call path.
- Hardened the existing grep-style structure test so its function-body slice
  stops at the closest next function declaration, not only the next
  `pub(super)` declaration.

Verification:

- `cargo fmt --check`: pass
- `cargo test cb017_ --lib -q`: pass, 40 tests
- `cargo test semantic_failure --lib -q`: pass, 40 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Pure semantic planning helpers are outside the
  actor loop; the admission-enrichment function remains in `turn.rs` until
  the SSOT structure test is replaced by a less source-layout-sensitive seam.

### 2026-05-27 Slice 40

Applied:

- Moved diagnostic target confidence gating into
  `semantic_repair_planning.rs`.
- Moved role/failure-kind compatibility selection for diagnostic targets into
  `semantic_repair_planning.rs`.
- Kept `model_assessment_to_verifier_repair_assessment` in `turn.rs` because
  it still coordinates workspace admission, context-derived fallbacks, and
  legacy assessment construction.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo test semantic_failure --lib -q`: pass, 40 tests
- `cargo test model_assessment --lib -q`: pass, 6 tests
- `cargo test validate_verifier_repair_intents_weakening_reject_compounds_evidence_gate --lib -q`: pass, 1 test
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Diagnostic target scoring and role compatibility
  are semantic planning policy now; `turn.rs` still owns the stateful bridge
  that turns an admitted diagnostic assessment into a legacy repair assessment.

### 2026-05-27 Slice 41

Applied:

- Added `verifier_repair_targeting.rs`.
- Moved diagnostic path-input safety checks out of `turn.rs`.
- Moved Python missing external dependency parsing and local module workspace
  path projection out of `turn.rs`.
- Kept admission execution and RecoveryTargetHint construction in `turn.rs`
  because they still depend on workspace scope, artifact ownership, and active
  RepairJob context.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_diagnostic_ --lib -q`: pass, 21 tests
- `cargo test cb017_security_unsafe_paths_rejected_by_enrich --lib -q`: pass, 1 test
- `cargo test missing_local_module --lib -q`: pass, 4 tests
- `cargo test dependency --lib -q`: pass, 25 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3027 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Target/path parsing helpers are now isolated;
  `turn.rs` still owns the stateful admission boundary and the policy decision
  to promote a parsed candidate into an actual repair target.

### 2026-05-27 Slice 42

Applied:

- Added module-local tests for `verifier_repair_targeting.rs`.
- Covered unsafe diagnostic path rejection, ignored state paths, external
  dependency detection that ignores local modules, and package-parent checks
  for missing local Python module paths.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 3 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3030 tests when rerun outside sandbox

Assessment:

- No production behavior change. The extracted targeting boundary now has
  local characterization tests, reducing dependence on large `turn.rs`
  integration-style tests for path/module parsing invariants.

### 2026-05-27 Slice 43

Applied:

- Added `repair_target_admission.rs`.
- Moved `RepairTargetAdmissionContext` and `admit_repair_target_hint` out of
  `turn.rs`.
- Kept `recovery_target_hint_for_diagnostic_path` and related hint-promotion
  sequencing in `turn.rs` because they still combine filesystem resolution,
  failure-kind gating, missing setup handling, and active RepairJob context.

Verification:

- `cargo fmt --check`: pass
- `cargo test admission --lib -q`: pass, 26 tests when rerun outside sandbox
  because one filtered test starts a local mockito server
- `cargo test cb017_ --lib -q`: pass, 40 tests
- `cargo test verifier_repair_targeting --lib -q`: pass, 3 tests
- `cargo test model_assessment --lib -q`: pass, 6 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3030 tests when rerun outside sandbox

Assessment:

- No behavior change intended. The owned target admission SSOT is now outside
  the actor loop; `turn.rs` still owns when to invoke that admission gate and
  how to thread admitted hints into the legacy verifier repair assessment.

### 2026-05-27 Slice 44

Applied:

- Added module-local tests for `repair_target_admission.rs`.
- Covered admission of in-scope edited hints, rejection of candidate-only
  hints, and rejection of edited hints outside ambiguous parent scope.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_target_admission --lib -q`: pass, 3 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3033 tests when rerun outside sandbox

Assessment:

- No production behavior change. The admission SSOT now has direct local tests,
  reducing reliance on large actor-loop tests for ownership-gate invariants.

### 2026-05-27 Slice 45

Applied:

- Added `verifier_failure_signature.rs`.
- Moved verifier failure error-kind extraction, failure signature construction,
  and compact failure text masking/truncation out of `turn.rs`.
- Kept target-candidate extraction and RepairJob construction in `turn.rs` for
  now because those still combine filesystem probing, changed-file hints, and
  repair context initialization.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_failure_signature --lib -q`: pass, 2 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3033 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Verifier failure fingerprint text shaping is now
  isolated from the actor loop; `turn.rs` still decides when that signature is
  attached to a RepairJob.

### 2026-05-27 Slice 46

Applied:

- Moved verifier failure signature characterization tests from `turn.rs` into
  `verifier_failure_signature.rs`.
- Kept the same assertions for failed-test-name stability and exception-type
  distinction while removing actor-loop test coupling.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_failure_signature --lib -q`: pass, 2 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3033 tests when rerun outside sandbox

Assessment:

- No production behavior change. The failure-signature module now owns its
  characterization tests directly.

### 2026-05-27 Slice 47

Applied:

- Added `verifier_diagnostic_attempt.rs`.
- Moved diagnostic LLM attempt limit, timeout constants, attempt spec type,
  and scheduling function out of `turn.rs`.
- Updated `repair_job.rs` tests to reference the new diagnostic attempt module
  instead of depending on `turn.rs` for the attempt limit.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_diagnostic_attempt_spec --lib -q`: pass, 1 test
- `cargo test repair_job --lib -q`: pass, 143 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3033 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Diagnostic retry scheduling is now a dedicated
  value module; `turn.rs` still owns the actual LLM call and diagnostic pass
  state updates.

### 2026-05-27 Slice 48

Applied:

- Removed the duplicate `artifact_role_from_repo_edit_category` helper from
  `turn.rs`.
- Replaced its call sites with the existing
  `task_contract::role_from_repo_edit` SSOT.

Verification:

- `cargo fmt --check`: pass
- `cargo test task_contract --lib -q`: pass, 85 tests when rerun outside
  sandbox because two filtered tests start a local mockito server
- `cargo test completion_evidence --lib -q`: pass, 26 tests
- `cargo test verifier_repair_targeting --lib -q`: pass, 3 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3033 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Repo-edit-category to artifact-role mapping now
  has one owner instead of a duplicated table in `turn.rs`.

### 2026-05-27 Slice 49

Applied:

- Moved `recovery_target_hint_for_existing_path` from `turn.rs` to
  `verifier_repair_targeting.rs`.
- Added local characterization tests for safe existing implementation/test
  path classification and unsafe/ignored path rejection.
- Left diagnostic admission sequencing and RepairJob state writes in
  `turn.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 5 tests
- `cargo test recovery_target_hint --lib -q`: pass, 2 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3035 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Existing-path to repair-target hint conversion
  is now owned by the verifier repair targeting module; `turn.rs` still owns
  when that hint is promoted through admission.

### 2026-05-27 Slice 50

Applied:

- Moved `recovery_target_hint_for_diagnostic_path` and
  `recovery_target_hint_for_missing_setup_path` from `turn.rs` to
  `verifier_repair_targeting.rs`.
- Kept the same admission SSOT:
  diagnostic paths still pass through syntactic path safety, setup-target
  gating, and owned-target admission before entering the repair plan.
- Added module-local tests for owned existing target admission, unowned target
  rejection, and missing setup manifest gating.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 8 tests
- `cargo test recovery_target_hint --lib -q`: pass, 5 tests
- `cargo test diagnostic_target --lib -q`: pass, 10 tests when rerun outside
  sandbox because two filtered tests start local mockito servers
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3038 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Diagnostic target promotion now has one module
  boundary. `turn.rs` still owns the stateful timing of diagnostic pass output,
  semantic plan construction, and RepairJob state updates.

### 2026-05-27 Slice 51

Applied:

- Moved `verifier_diagnostic_missing_setup_candidates` and
  `python_verifier_output_missing_external_dependency` from `turn.rs` to
  `verifier_repair_targeting.rs`.
- Kept the behavior unchanged: only pytest-like verifier output with a missing
  third-party dependency can synthesize the missing `pyproject.toml` setup
  candidate.
- Added tests for setup candidate synthesis, non-pytest rejection, and local
  module rejection.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 10 tests
- `cargo test missing_setup --lib -q`: pass, 6 tests
- `cargo test recovery_target_hint --lib -q`: pass, 5 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3040 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Missing setup candidate generation is now a
  read-only verifier-targeting responsibility; `turn.rs` only consumes the
  generated hints while assembling the diagnostic prompt context.

### 2026-05-27 Slice 52

Applied:

- Moved missing local Python module provider targeting from `turn.rs` to
  `verifier_repair_targeting.rs`:
  - `recovery_target_hint_for_missing_local_module_path`
  - `verifier_repair_missing_local_module_provider`
- Kept behavior unchanged: prospective provider files are only synthesized
  when implementation code imports the missing local module.
- Added module-local tests for prospective provider targeting and test-only
  import rejection.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 12 tests
- `cargo test missing_local_module --lib -q`: pass, 6 tests
- `cargo test verifier_repair_missing_local_module --lib -q`: pass, 4 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3042 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Local-module provider target selection is now a
  verifier-targeting responsibility; `turn.rs` only uses the selected hint
  while building the legacy/semantic repair assessment bridge.

### 2026-05-27 Slice 53

Applied:

- Moved two remaining read-only repair target choice helpers from `turn.rs` to
  `verifier_repair_targeting.rs`:
  - `verifier_repair_preferred_local_import_source`
  - `verifier_repair_stale_assertion_test_target`
- Preserved existing admission semantics; both helpers still require owned
  target admission before returning a hint.
- Added module-local tests for local import provider promotion and stale
  assertion test retargeting.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_targeting --lib -q`: pass, 14 tests
- `cargo test local_import_source --lib -q`: pass, 2 tests
- `cargo test stale_assertion --lib -q`: pass, 4 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3044 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Target-choice policy for import-contract and
  unresolved assertion repair is no longer embedded in `turn.rs`; the actor
  loop still owns only when to apply those choices to the assessment bridge.

### 2026-05-27 Slice 54

Applied:

- Moved `enrich_failure_clusters_with_admitted_targets` from `turn.rs` to
  `semantic_repair_planning.rs`.
- Kept its dependencies explicit:
  - target admission still flows through
    `verifier_repair_targeting::recovery_target_hint_for_diagnostic_path`
  - role ordering still uses
    `sort_admitted_by_authority_role_priority`
- Updated structural tests to inspect the new owner file instead of
  `turn.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test cb017_enrich --lib -q`: pass, 2 tests
- `cargo test cb017 --lib -q`: pass, 40 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3044 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Semantic failure report enrichment now lives
  with semantic repair planning. `turn.rs` still owns diagnostic-pass
  sequencing and RepairJob state writes.

### 2026-05-27 Slice 55

Applied:

- Moved verifier-output target candidate parsing from `turn.rs` to
  `verifier_repair_targeting.rs`:
  - `VerifierRepairTargetCandidate`
  - path-like token extraction
  - verifier-output target selection
  - changed-file hint generation
- `turn.rs` now consumes the targeting module for both failure-output targets
  and changed-file hints.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_target --lib -q`: pass, 20 tests
- `cargo test verifier_repair_context --lib -q`: pass, 7 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3044 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Verifier output parsing and changed-file target
  hinting are now verifier-targeting responsibilities; `turn.rs` still owns
  RepairJob construction and state carry-over.

### 2026-05-27 Slice 56

Applied:

- Moved generic verifier failure-count extraction from `turn.rs` to
  `verifier_failure_signature.rs`.
- Kept `turn.rs` responsible for using the count while building
  `RepairJob`; only the pure output parser moved.
- Added module-local coverage for failure summary parsing.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_failure_signature --lib -q`: pass, 3 tests
- `cargo test verifier_failure_count --lib -q`: pass, 2 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Failure signature/error/count extraction now
  shares the same module; `turn.rs` keeps only RepairJob construction logic.

### 2026-05-27 Slice 57

Applied:

- Moved verifier rerun outcome classification from `turn.rs` to
  `repair_job.rs`.
- `turn.rs` now calls `repair_job::verifier_repair_rerun_outcome` while
  constructing the next RepairJob.
- Kept cluster-aware refinement (`rerun_outcome_with_cluster`) unchanged in
  `repair_job.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_context_classifies_rerun_result --lib -q`: pass
- `cargo test rerun_outcome --lib -q`: pass, 3 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Rerun outcome classification now lives next to
  the RepairJob state vocabulary; `turn.rs` still constructs and stores the
  job.

### 2026-05-27 Slice 58

Applied:

- Moved verifier failure to `RepairJob` context construction from `turn.rs` to
  `repair_job.rs`.
- Moved parser-scope failure classification stub with the context builder so
  `failure_type` initialization is owned by the RepairJob state module.
- `turn.rs` now only invokes `verifier_repair_context_from_failure` when the
  verifier observes a failure; it no longer assembles carry-over state,
  signatures, changed-file hints, rerun outcome, or sanitized excerpts inline.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_repair_context --lib -q`: pass, 7 tests
- `cargo test rerun_outcome --lib -q`: pass, 3 tests
- `cargo test verifier_failure_count --lib -q`: pass, 2 tests
- `cargo test repair_job --lib -q`: pass, 143 tests
- `cargo test verifier_repair_target --lib -q`: pass, 20 tests
- `cargo test cb017 --lib -q`: pass, 40 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. RepairJob lifecycle state construction now has
  one owner in `repair_job.rs`; `turn.rs` keeps verifier observation and
  orchestration only.

### 2026-05-27 Slice 59

Applied:

- Moved effective verifier repair target selection from `turn.rs` to
  `repair_job.rs`.
- Removed one reverse dependency where `repair_job.rs` called back into
  `turn.rs` to determine the active repair target.
- `turn.rs` now imports `verifier_repair_effective_target_hint` as a
  RepairJob-owned projection.

Verification:

- `cargo fmt --check`: pass
- `cargo test target_hint --lib -q`: pass, 13 tests
- `cargo test verifier_repair_context --lib -q`: pass, 7 tests
- `cargo test repair_job --lib -q`: pass, 143 tests

Assessment:

- No behavior change intended. The active repair target is now derived in the
  same module that owns semantic-plan exhaustion and RepairJob state.

### 2026-05-27 Slice 60

Applied:

- Moved verifier repair state projection for task-contract recovery into
  `repair_job.rs` as `task_contract_repair_state_from_job`.
- Reduced `Agent::task_contract_repair_state` to a thin adapter over the
  RepairJob-owned projection.
- Kept the task-contract-facing `VerifierRepairState` type owned by
  `repair_job.rs` and re-exported from `task_contract.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_job --lib -q`: pass, 143 tests
- `cargo test task_contract --lib -q`: pass, 85 tests when rerun outside
  sandbox

Assessment:

- No behavior change intended. Task-contract recovery now receives verifier
  repair state from the RepairJob module instead of duplicating projection
  logic inside the actor loop.

### 2026-05-27 Slice 61

Applied:

- Moved verifier changed-file aggregation from `turn.rs` to
  `verifier_repair_targeting.rs`.
- `turn.rs` now consumes a targeting-owned helper before constructing or
  updating repair jobs.

Verification:

- `cargo fmt --check`: pass
- `cargo test changed_files --lib -q`: pass, 5 tests
- `cargo test verifier_repair_target --lib -q`: pass, 20 tests

Assessment:

- No behavior change intended. Changed-file normalization is now colocated
  with verifier repair target parsing and hint generation.

### 2026-05-27 Slice 62

Applied:

- Added `tool_policy.rs` and moved `EffectiveToolPolicy`,
  `EffectiveToolPolicyReason`, `FocusedEditPolicy`, and
  `ArtifactDirectedPolicy` out of `turn.rs`.
- Updated `active_job_arbiter.rs` to depend on the standalone policy module
  instead of importing policy types from the actor loop.
- Updated policy-focused tests and PAM advisory tests to import from
  `tool_policy.rs`.

Verification:

- `cargo fmt --check`: pass
- `cargo test active_job_arbiter --lib -q`: pass, 46 tests
- `cargo test effective_tool_policy --lib -q`: pass, 8 tests when rerun
  outside sandbox

Assessment:

- No behavior change intended. Tool policy is now a neutral projection module;
  active-job arbitration no longer depends on `turn.rs` for policy types.

### 2026-05-27 Slice 63

Applied:

- Added `tool_history.rs` for conversation/tool-call evidence projection.
- Moved focused-edit read evidence, latest successful read lookup, verifier
  repair note index lookup, target-path matching, and preferred read/edit
  target classification out of `turn.rs`.
- Updated `repair_job.rs` test decision bridge to consume `tool_history.rs`
  directly instead of calling back into `turn.rs`.
- Removed the duplicate test-only verifier repair target-path helper from
  `turn.rs`; the RepairJob module now owns the local test bridge.

Verification:

- `cargo fmt --check`: pass
- `cargo test focused_edit_target_already_read --lib -q`: pass, 1 test
- `cargo test verifier_repair_decision --lib -q`: pass, 2 tests
- `cargo test repair_job --lib -q`: pass, 143 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Tool-history evidence is now a neutral support
  module, reducing reverse dependencies between the RepairJob state machine
  and the actor loop dispatcher.

### 2026-05-27 Slice 64

Applied:

- Moved tool-policy enforcement from `turn.rs` to `tool_policy.rs`.
- `tool_policy.rs` now owns focused-edit policy errors, artifact-directed
  path matching, MissingVerifierJob scope checks, setup-bootstrap Bash command
  filtering, and batch truncation/rejection decisions.
- Moved `workspace_relative_path_for_tool_arg` into the policy module so
  path-target comparisons and scope checks share one policy-owned helper.
- `turn.rs` now imports policy decisions instead of defining them inline.

Verification:

- `cargo fmt --check`: pass
- `cargo test focused_edit_target_already_read --lib -q`: pass, 1 test
- `cargo test effective_tool_policy --lib -q`: pass, 8 tests when rerun
  outside sandbox
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Tool-policy gatekeeping is now colocated with
  the policy data model, further reducing actor-loop responsibility and
  making active-job policy projection easier to audit.

### 2026-05-27 Slice 65

Applied:

- Moved verifier prompt `behavior_contract` payload shaping from `turn.rs` to
  `required_behavior.rs`.
- The required-behavior module now owns the serialized size cap, excerpt
  truncation, low-priority field drop order, and `truncated=true` metadata for
  diagnostic/repair prompt payloads.
- `turn.rs` now imports the shaped JSON value and only attaches it to verifier
  diagnostic/repair prompt payloads.

Verification:

- `cargo fmt --check`: pass
- `cargo test behavior_contract --lib -q`: pass, 25 tests
- `cargo test verifier_repair_pass_messages --lib -q`: pass, 1 test
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Behavior-contract trust narrowing and payload
  size control now live with the behavior contract schema instead of inside
  actor-loop prompt assembly.

### 2026-05-27 Slice 66

Applied:

- Moved `agent.active_job.selected` payload construction from `turn.rs` to
  `active_job_arbiter.rs`.
- The active-job arbiter now owns the sanitized projection of selected job,
  rejected jobs, projected policy, and budget counters.
- `turn.rs` still owns the emission timing/dedup path, but no longer owns the
  selected-job payload schema.

Verification:

- `cargo fmt --check`: pass
- `cargo test active_job_arbiter --lib -q`: pass, 46 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Active-job structured output is now colocated
  with active-job selection/projection, reducing another actor-loop schema
  responsibility.

### 2026-05-27 Slice 67

Applied:

- Moved `agent.verifier.invoked` and
  `agent.verifier.external_import_rejected` payload construction from
  `turn.rs` to `auto_test.rs`.
- `auto_test.rs` now owns the verifier invocation snapshot and the sanitized
  telemetry schema derived from that snapshot.
- `turn.rs` keeps only emission timing, masking, and dedup ownership.

Verification:

- `cargo fmt --check`: pass
- `cargo test verifier_invoked --lib -q`: pass, 17 tests
- `cargo test external_import_rejected --lib -q`: pass, 4 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 3045 tests when rerun outside sandbox

Assessment:

- No behavior change intended. Verifier execution telemetry is now colocated
  with verifier execution/snapshot logic instead of remaining as actor-loop
  schema construction.
