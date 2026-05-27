# v0.4.25 Complete Remaining Work Plan

## Purpose

This document turns the remaining cleanup into an executable plan. The goal is
not to make one FastAPI CRUD case pass. The goal is to finish the structural
cleanup so Anvil has a small, auditable, local-LLM-oriented control loop.

The desired end state is:

- `turn.rs` orchestrates one turn but does not own repair policy, patch
  validation, diagnostic parsing, focused-edit policy, progress formatting, or
  legacy fallback decisions.
- verifier repair dispatch has one production source:
  `ActiveJobArbiter` selects the active job and `RepairJob::next_action()`
  selects the verifier repair action.
- legacy and deterministic paths cannot override an active job and cannot
  mark a task as complete.
- unsafe, ambiguous, or non-converging repair ends as actionable safe stop,
  not as generic retry or false-positive `done`.
- generic evaluation covers multiple project shapes, not only Python/FastAPI.

## Current State Snapshot

Measured code size:

| File | Approx lines | Current concern |
| --- | ---: | --- |
| `src/agent/loop_run/turn.rs` | 37,953 | still too many responsibilities |
| `src/agent/loop_run/repair_job.rs` | 7,674 | conceptual center but internally broad |
| `src/agent/loop_run/active_job_arbiter.rs` | 1,620 | dispatch owner exists but legacy parity remains |
| `src/agent/loop_run/tool_policy.rs` | 603 | policy extraction in progress |
| `src/agent/loop_run/tool_history.rs` | 360 | history extraction in progress |
| `src/agent/loop_run/progress_text.rs` | 181 | low-level progress helpers extracted |

Known unrelated working-tree noise must remain out of this cleanup:

- `.claude/scheduled_tasks.lock`
- `.agents/skills/source-command-*`
- `scripts/photon_weekly_observe.sh`

## Completion Definition

This effort is complete only when all checks below are true.

- A verifier repair action cannot be dispatched unless the active owner is
  `RepairJob`.
- A verifier repair patch cannot be requested or applied without an accepted
  semantic repair plan.
- Focused edit recovery, generic retry, deterministic fallback, and scaffold
  fallback cannot run while `RepairJob` or `MissingVerifierJob` owns the turn.
- Legacy assessment compatibility is either telemetry/assist/test-only, or it
  is explicitly admitted into a semantic plan before production patch repair.
- `turn.rs` contains no main patch validator and no main verifier repair
  decision bridge.
- `done` requires request-aligned artifacts and project-unit-aligned verifier
  evidence.
- Safe stop always includes blocker class, authority status, affected evidence,
  and next user action.
- Generic regression evaluation has zero false-positive `done`.

## Non-Goals

- Do not add task-specific templates for FastAPI, ToDo, CRUD, CSV, Rust, or
  Node.
- Do not weaken tests merely to get green verification.
- Do not introduce a broad provider abstraction.
- Do not delete every old path blindly before the replacement path is tested.
- Do not rely on manual 20+20 evaluation before targeted state-machine tests
  explain the behavior.

## Workstream A: Baseline, Guard Rails, And No-More-Patchwork Gates

### Tasks

- [ ] Record current `git status --short` before each slice.
- [ ] Keep unrelated dirty files out of staging.
- [ ] Add source-level regression tests that forbid direct production calls to:
  - `verifier_repair_decision` from patch-provider or repair dispatch paths;
  - deterministic repair candidate application from production verifier repair;
  - focused edit recovery while repair or missing-verifier owner is active.
- [ ] Add a small "dispatch invariant" test group:
  - active repair suppresses artifact completion;
  - active repair suppresses focused edit;
  - active repair suppresses deterministic fallback;
  - active missing verifier suppresses generic retry;
  - stale flags without an active job do not dispatch repair.
- [ ] Maintain `workspace/v0.4.25/dispatch-source-inventory.md` as code moves.

### Acceptance

- The invariant tests fail if a new bypass branch is introduced.
- Every later slice starts from a known clean baseline, excluding unrelated
  workspace noise.

## Workstream B: Dispatch Source Finalization

### Problem

`ActiveJobArbiter` and `RepairJob` exist, but `turn.rs` still contains
decision-adjacent helpers such as:

- `verifier_repair_decision`
- `verifier_repair_policy_for_decision`
- `verifier_repair_policy_for_target_hint`
- `malformed_repair_attempt_outcome_for_active_target`
- `verifier_repair_invalid_can_continue`
- task-contract repair state adapters

### Tasks

- [ ] Move production verifier repair decision state into `RepairJob`.
- [ ] Keep only thin compatibility adapters in `turn.rs` until call sites are
  gone.
- [ ] Replace `VerifierRepairDecision` production usage with typed
  `RepairJobAction` / `LoopControlAction` where practical.
- [ ] Ensure `effective_tool_policy` is a projection of active owner, not an
  alternate dispatcher.
- [ ] Move task-contract-facing repair state projection into a small module or
  `repair_job` projection API.
- [ ] Add tests that enumerate each `LoopControlAction` and assert which
  lower-level recovery gates are allowed.

### Acceptance

- `turn.rs` asks the arbiter/job what to do; it does not compute repair next
  action itself.
- `verifier_repair_decision` is test-only, compatibility-only, or deleted.

## Workstream C: Verifier Repair Pipeline Boundaries

### Desired Pipeline

```text
FailurePacket
  -> DiagnosticPrompt
  -> SemanticFailureReport
  -> AcceptedRepairPlan
  -> PatchProposal
  -> ValidatedRepairEdit
  -> PatchExecutor
  -> VerifierDelta
  -> RepairJob::next_action()
```

### Tasks

- [ ] Extract diagnostic prompt assembly from `turn.rs`:
  - `verifier_diagnostic_messages`
  - `verifier_diagnostic_file_excerpts`
  - `safe_verifier_diagnostic_file_excerpt`
- [ ] Extract repair-pass prompt assembly from `turn.rs`:
  - `verifier_repair_pass_messages`
  - `verifier_repair_pass_retry_message`
  - `safe_verifier_repair_file_excerpt`
- [ ] Move or delete remaining thin wrappers around:
  - repair intent parsing;
  - patch proposal parsing;
  - proposal-to-intent conversion;
  - intent fingerprinting.
- [ ] Ensure patch application only accepts `ValidatedRepairEdit`.
- [ ] Ensure preimage hash check remains in `repair_patch_executor`.
- [ ] Add tests for changed preimage, unsafe path, symlink escape, duplicate
  intent, no-op edit, secret introduction, and test weakening.

### Acceptance

- `turn.rs` may call the pipeline, but does not own parsing, validation, or
  mutation safety rules.
- All unsafe/malformed patch outcomes are typed before they reach `turn.rs`.

## Workstream D: RepairJob Internal Decomposition

### Problem

`repair_job.rs` has become the center but is large enough to become the next
maintenance bottleneck.

### Target Modules

- `repair_job/state.rs`: state, counters, budgets, active cluster/target.
- `repair_job/driver.rs`: `next_action`, transition policy, event application.
- `repair_job/delta.rs`: verifier delta and failure comparison.
- `repair_job/report.rs`: terminal repair report and safe-stop projection.
- `repair_job/legacy.rs`: temporary compatibility only, with expiry notes.

### Tasks

- [ ] Move pure state transition tests next to `driver`.
- [ ] Move budget accounting tests next to `state`.
- [ ] Move failure delta tests next to `delta`.
- [ ] Move safe-stop projection tests next to `report`.
- [ ] Keep legacy compatibility in a clearly named boundary; do not let it
  leak into new production APIs.
- [ ] Avoid splitting into pass-through files; every module must own a
  decision, data type, or validation boundary.

### Acceptance

- `RepairJob` remains the semantic owner, but no single file owns every repair
  concern.
- New repair behavior can be unit-tested without constructing a full `Agent`.

## Workstream E: VerifierDelta As The Repair Transition Input

### Problem

Repair convergence is weak if the loop only tracks attempts. It needs to know
whether the latest patch improved, changed, worsened, or did not affect the
verifier failure.

### Tasks

- [ ] Define `VerifierDelta` with:
  - previous failure fingerprint;
  - current failure fingerprint;
  - fixed failures;
  - unchanged failures;
  - new failures;
  - verifier command;
  - project unit.
- [ ] Feed `VerifierDelta` into `RepairJob`.
- [ ] Add transitions:
  - improvement: continue or rerun verifier;
  - unchanged after valid patch: lower target confidence or re-diagnose;
  - repeated invalid proposal: consume job-level budget then target switch,
    re-diagnostic, or safe stop;
  - new unrelated failure: re-diagnostic;
  - ambiguous assertion authority: actionable safe stop.
- [ ] Add synthetic tests for compile/import/runtime failure, assertion
  mismatch, test bug, missing dependency/setup, and multi-failure cases.

### Acceptance

- Repair decisions are based on verifier delta plus authority, not retry count
  alone.
- Repeated non-improving repair cannot loop until `max_iterations`.

## Workstream F: Legacy, Deterministic, Scaffold, And Focused-Edit Reduction

### Tasks

- [ ] Classify every remaining `legacy`, `deterministic`, `fallback`, and
  `scaffold` production call site as:
  - production active path;
  - compatibility wrapper;
  - validator assist;
  - telemetry only;
  - test only;
  - deletion candidate.
- [ ] Remove production verifier repair access to deterministic candidates.
- [ ] Keep deterministic scaffold only as bootstrap; never completion evidence
  by itself.
- [ ] Move scaffold snapshot/diff helpers out of `turn.rs`.
- [ ] Move framework fallback helpers out of `turn.rs` or demote/delete them.
- [ ] Move focused-edit notes/history helpers out of `turn.rs` into
  `tool_policy` / `tool_history` / a dedicated focused-edit module.
- [ ] Add source-level tests that production repair code cannot call legacy
  deterministic apply paths.

### Acceptance

- No legacy or deterministic path can override active job ownership.
- Every retained fallback has an owner, an allowed scope, and an expiry plan.

## Workstream G: Artifact Evidence And Done Gate Hardening

### Tasks

- [ ] Audit artifact evidence sources:
  - implementation;
  - tests;
  - usage docs;
  - setup/config;
  - verifier pass.
- [ ] Ensure `.anvil-state`, evaluation outputs, logs, and transient files are
  ignored as artifact evidence.
- [ ] Require verifier evidence to match project unit and task scope.
- [ ] Preserve multi-directory project support by using project-unit evidence,
  not single-file heuristics.
- [ ] Add tests for:
  - empty workspace;
  - existing workspace with unrelated old artifacts;
  - multi-directory app;
  - docs-only task;
  - missing verifier setup;
  - wrong-stack verifier command.

### Acceptance

- Existing old files in sibling directories cannot satisfy a fresh-session
  request.
- Wrong-stack verifier pass cannot lead to `done`.
- Placeholder docs/tests cannot satisfy required roles.

## Workstream H: Safe Stop Quality

### Tasks

- [ ] Ensure every terminal non-success repair path returns a structured
  report with:
  - blocker class;
  - verifier command;
  - project unit;
  - affected files;
  - observed failure summary;
  - authority status;
  - attempted actions;
  - why repair was not safe;
  - next user action.
- [ ] Render concise user-facing text from the structured report.
- [ ] Add tests for:
  - ambiguous generated assertion;
  - repeated invalid patch;
  - unsafe test weakening;
  - missing setup/verifier;
  - diagnostic unavailable;
  - wrong-stack verifier.

### Acceptance

- Safe stop is treated as a correct terminal only when actionable.
- `missing_repo_edits` is not used for active verifier repair exhaustion.

## Workstream I: Progress, Plan, And UI Responsibility Extraction

### Tasks

- [ ] Move `progress_path_display`, `tool_display`,
  `format_progress_line`, `format_blocked_progress_line`, and
  `build_recent_tool_summary` out of `turn.rs`.
- [ ] Move plan read/write summary helpers out of `turn.rs`.
- [ ] Keep `progress_text.rs` for primitive string/style helpers and create a
  separate display module for higher-level progress-line formatting.
- [ ] Move related tests with the helpers.

### Acceptance

- Display code cannot affect dispatch or repair state.
- `turn.rs` shrinks without changing control behavior.

## Workstream J: Diagnostic Robustness And PAM Boundary

### Tasks

- [ ] Keep diagnostic LLM output as advisory until parsed, bounded, and
  admitted.
- [ ] Add malformed diagnostic retry budget tests.
- [ ] Add tests showing fallback diagnostic model cannot bypass plan admission.
- [ ] Keep PAM as context/advisory only:
  - no authority assignment from PAM alone;
  - no patch target admission from PAM alone;
  - no done/safe-stop decision from PAM alone.
- [ ] Evaluate no-PAM first, then PAM, to avoid hiding base control regressions.

### Acceptance

- PAM can improve hints but cannot change safety authority.
- Malformed diagnostic output consumes typed budget and reaches re-diagnostic
  or safe stop predictably.

## Workstream K: Generic Evaluation Harness

### Tasks

- [ ] Implement or document a repeatable runner that creates isolated
  directories under `anvilwork/NNN`.
- [ ] Track for each run:
  - prompt;
  - terminal status;
  - verifier command;
  - edited files;
  - project unit;
  - false-positive done;
  - wrong-stack verifier;
  - repair loop exhaustion;
  - safe-stop actionability;
  - iteration count.
- [ ] Start with a no-PAM 5-case gate:
  - Python CLI/API;
  - Rust library;
  - Node CLI/package;
  - docs-only or existing-project change;
  - missing verifier or ambiguous assertion.
- [ ] Run PAM 5-case gate only after no-PAM has no critical failures.
- [ ] Run 20+20 only after targeted tests and 5+5 smoke are stable.

### Acceptance

- Evaluation exposes structural failure categories instead of only reporting
  success/failure count.
- No case is considered success by false-positive `done`.

## Workstream L: Test Relocation And Coverage Cleanup

### Tasks

- [ ] Move tests from `turn.rs` to the modules that own the logic:
  - patch validation tests to `repair_patch_validation`;
  - repair transition tests to `repair_job`;
  - progress display tests to progress display module;
  - focused edit tests to policy/history modules;
  - scaffold fallback tests to scaffold/fallback module;
  - artifact evidence tests to task contract/artifact ledger modules.
- [ ] Keep only full actor-loop integration tests in `turn.rs`.
- [ ] Avoid source-string tests unless they guard an architectural invariant
  that cannot be asserted through public behavior.

### Acceptance

- Tests document module ownership.
- `turn.rs` test surface no longer hides coupling.

## Workstream M: Final Verification And Commit Hygiene

### Commands

Run after each meaningful slice:

```bash
cargo fmt --check
cargo test <targeted-pattern> --lib -q
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Run before final acceptance:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib -q
cargo build --release
```

### Tasks

- [ ] Split commits by concern:
  - dispatch source;
  - repair pipeline boundaries;
  - RepairJob split;
  - verifier delta;
  - legacy deletion/demotion;
  - done gate;
  - safe stop;
  - evaluation harness/docs.
- [ ] Do not stage unrelated working-tree noise.
- [ ] Update `workspace/v0.4.25/evaluation-results.md` after evaluation.
- [ ] Update `workspace/v0.4.25/dispatch-source-inventory.md` after deleting
  or moving each dispatch path.

### Acceptance

- Final `git status --short` has only known unrelated noise or is clean.
- Final report states completed work, residual risk, and next actions.

## Execution Order

Use this order to avoid another patchwork cycle:

1. Workstream A: guard rails and inventory update.
2. Workstream B: dispatch source finalization.
3. Workstream C: verifier repair pipeline boundaries.
4. Workstream E: verifier delta as transition input.
5. Workstream H: actionable safe stop.
6. Workstream F: legacy/deterministic/focused-edit reduction.
7. Workstream G: artifact evidence and done gate hardening.
8. Workstream D: internal RepairJob split.
9. Workstream I: progress/plan/UI extraction.
10. Workstream L: test relocation.
11. Workstream K: generic evaluation harness and smoke runs.
12. Workstream M: final verification and commit hygiene.

The order intentionally delays cosmetic extraction until dispatch and repair
safety are pinned. Moving display helpers is useful, but it does not solve the
core control risk.

## Parallelization Plan

The work can be split safely only by ownership boundary:

- Worker 1: dispatch source and arbiter invariants.
- Worker 2: repair patch validation / executor boundary.
- Worker 3: verifier delta and RepairJob transition tests.
- Worker 4: artifact evidence / done gate.
- Worker 5: progress and plan display extraction.
- Worker 6: evaluation harness and documentation.

Do not let two workers edit the same module family at the same time:

- `turn.rs` and `active_job_arbiter.rs` are one write domain.
- `repair_job*` modules are one write domain.
- `repair_patch_validation.rs` and `repair_patch_executor.rs` are one write
  domain.
- `task_contract`, `artifact_ledger`, and completion evidence are one write
  domain.

## Risk Review

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Over-tightening repair causes too many safe stops | Lower completion rate | verifier delta continues objective repairs when authority is clear |
| Legacy deletion regresses useful behavior | Hidden flow loss | characterize first, delete only after typed replacement passes |
| More modules hide complexity | Maintenance worsens | each module must own a real decision or data boundary |
| Evaluation overfits to FastAPI | false confidence | mixed case matrix; no-PAM first |
| PAM masks base control issues | unstable quality | PAM cannot provide authority; no-PAM gate comes first |
| Safe stop becomes vague | user cannot act | structured `SafeStopReport` acceptance tests |
| Source-string tests become brittle | refactor pain | use behavior tests except for architecture guard rails |

## Final Acceptance Checklist

- [ ] `turn.rs` no longer owns main repair validation.
- [ ] `turn.rs` no longer owns production verifier repair next-action logic.
- [ ] `turn.rs` no longer owns high-level progress display formatting.
- [ ] `RepairJob` has typed delta-driven transitions.
- [ ] Legacy repair bridge cannot apply patches directly.
- [ ] Deterministic fallback cannot run during active job ownership.
- [ ] Focused edit recovery cannot steal verifier repair turns.
- [ ] Done gate rejects wrong stack, wrong project unit, placeholder evidence,
  and old unrelated artifacts.
- [ ] Safe stop is actionable in all terminal repair failures.
- [ ] Generic no-PAM smoke passes critical gates.
- [ ] PAM smoke does not reduce safety.
- [ ] Full Rust verification passes.

