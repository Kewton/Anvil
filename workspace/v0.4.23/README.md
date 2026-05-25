# v0.4.23 State Control Consolidation

## Purpose

This workspace tracks the effort to simplify Anvil's state control so one controller decides the next action for each phase.

The immediate target is verifier repair. When a verifier failure has created a `RepairJob`, production dispatch must go through `RepairJob::next_action()` and `RepairJob::begin_next_repair_step()` only. Focused edit recovery, generic retry, and legacy deterministic repair must not independently decide repair progress.

The full unification plan is maintained in `full-unification-work-plan.md`.
The remaining completion work is tracked in `completion-work-plan.md`.
The execution plan for reaching the final completion criteria is maintained in `complete-achievement-execution-plan.md`.

## Current Dispatch Sources

### Production path to keep

- `task_contract::plan_artifact_recovery`
  - Owns pre-verifier artifact completion decisions.
  - Should decide whether required artifact roles are still missing, whether verifier can run, or whether the task can safe stop before verification.
- `project_probe::probe_completion`
  - Owns generic workspace readiness probing.
  - Should return facts/decision for verifier readiness, not repair policy.
- `repair_job::RepairJob::next_action`
  - Owns verifier repair phase decisions after a verifier failure.
  - Should produce diagnostic, patch, verifier rerun, safe stop, or done.
- `active_job_arbiter::select_active_job`
  - Owns tool policy projection for one write-owner job.
  - Should not decide semantic progress.

### Transitional / fallback path to restrict

- `MissingVerifierJob`
  - Transitional missing-verifier bootstrap. It may remain until project verifier discovery is generalized.
- focused edit recovery
  - Useful for non-repair protocol recovery, but must not control verifier repair when `RepairJob` exists.
- local-LLM small edit fallback
  - Should remain a protocol fallback, not a completion or repair authority.
- deterministic repair/scaffold paths
  - Should be telemetry, validator assist, or explicit fallback only. They should not override the main state machine.

## Change Made In This Pass

### Phase 1: remove duplicate verifier repair dispatch

`turn.rs` had a new top-level `dispatch_repair_job_step()` path, but old verifier repair branches still existed below it:

- direct `DiagnosticUnavailable` handling via `scope_safeguarded_verifier_repair_decision`
- direct diagnostic pass dispatch
- direct controller-applied patch dispatch

Those branches duplicated the same responsibilities already handled by `dispatch_repair_job_step()`.

This pass removes the duplicate lower branches. With this change, when all of the following are true:

- mode is not Plan
- `task_contract_verifier_repair_pending == true`
- `repair_job.is_some()`

then the loop enters `dispatch_repair_job_step()` at the start of the iteration and the selected repair step is derived from `RepairJob::begin_next_repair_step()`.

### Phase 2: project active repair policy from `RepairJob::next_action()`

The active-job policy branch still projected verifier repair through the legacy `VerifierRepairDecision` compatibility shape. That kept one more semantic dispatch source in production code.

This pass adds a direct mapping:

```text
RepairJob::next_action()
  -> RepairNextAction
  -> EffectiveToolPolicy
  -> DesiredAction::VerifierRepair
```

When an active `RepairJob` exists, `build_arbiter_candidates()` now reads `RepairJob::next_action()` directly. For `RequestPatch`, the target hint is converted into a least-privilege read/edit/write policy for that specific workspace-relative path. For diagnostic, replan, rerun verifier, safe stop, and done states, the LLM tool surface is closed because those steps are controller-owned.

### Phase 3: introduce `LoopControlAction`

This pass adds a small pre-model controller decision:

```text
LoopControlInputs
  -> determine_loop_control_action()
  -> LoopControlAction
```

The top-level verifier dispatch now goes through this boundary:

```text
ContinueRepairJob -> dispatch_repair_job_step()
RunVerifier       -> drive_task_contract_verifier()
```

`build_arbiter_candidates()` also projects verifier repair / missing verifier ownership through `LoopControlAction`. A stale `task_contract_verifier_repair_pending` flag without either `RepairJob` or `MissingVerifierJob` no longer creates a verifier-repair active job.

### Phase 4: demote legacy verifier repair projection

`VerifierRepairDecision` and `repair_job::verifier_repair_decision()` are now test-only compatibility helpers. Production recovery notes and verifier repair policy messages read `RepairJob::next_action()` directly.

## Remaining Structural Problems

1. `VerifierRepairDecision` still exists in test-only compatibility code.
   - It is no longer used as a production dispatch source.
   - Remaining work is to delete or isolate old compatibility tests once the new transition-table tests fully cover the same behavior.

2. `MissingVerifierJob` is still outside `RepairJob`.
   - It is now projected through `LoopControlAction`, but the state struct remains separate.
   - A future pass should expose a dedicated bootstrap next-action enum and remove any remaining missing-verifier special casing.

3. Artifact completion can still fail before verifier repair starts.
   - This is separate from verifier repair. It needs an artifact-completion state machine or clearer project-unit probing.

4. `project_probe` is still conservative.
   - It detects safe verifier readiness but does not yet provide a full project-unit model for every stack.

5. Legacy fallback code still exists.
   - The main verifier repair path is narrower now, but deterministic scaffold/repair paths still need a production-path audit.

## Quality Plan

Run in this order:

1. `cargo fmt --check`
2. targeted verifier repair tests
3. `cargo test task_contract --lib`
4. `cargo test repair_job --lib`
5. `cargo test --lib`
6. `cargo clippy --all-targets -- -D warnings`
7. `cargo build --release`

E2E evaluation should wait until the state-transition tests pass.

## Verification Result

Completed in this pass:

- `cargo fmt --check`: pass
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test synthetic_verifier_e2e --lib`: pass, 4 tests
- `cargo test production_repair_dispatch_does_not_call_legacy_decision_bridge --lib`: pass, 1 test
- `cargo test missing_verifier_job --lib`: pass, 6 tests
- `cargo test repair_job --lib`: pass, 133 tests
- `cargo test project_probe --lib`: pass, 5 tests
- `cargo test task_contract --lib`: pass, 80 tests
  - First sandboxed run failed because mock server creation was blocked by the sandbox.
  - Re-run outside the sandbox passed.
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib`: pass, 2925 tests
- `cargo build --release`: pass

Latest smoke evaluation, 2026-05-25:

| Group | Result | Notes |
|---|---:|---|
| FastAPI CRUD no-PAM | 4/5 `done` | 1 run ended `verifier_failed`; failure showed `.anvil-state/verifier-python/site` leaking into runtime import behavior (`State.items` missing). |
| FastAPI CRUD PAM live | 4/5 `done` | 1 run ended `verifier_failed`; delete endpoint expectation mismatch remained after repair attempts. |
| Generality smoke: Rust CLI | 0/1 `done` | Ended `transport_error` because auto test command timed out after 300s. |
| Generality smoke: Python data script | 0/1 `done` | Ended with verifier repair safe stop: `patch_rejected_repeatedly`. |
| Generality smoke: Node utility | 0/1 `done` | Ended with verifier repair safe stop: `patch_rejected_repeatedly`. |

Smoke logs are under `workspace/v0.4.23/smoke-20260525/`.

Interpretation:

- The main verifier-control regression is improved: observed verifier-owned runs did not fall back to generic `missing_repo_edits`.
- FastAPI CRUD success is much better than the earlier baseline, but not stable enough to call complete.
- Generality is not complete. The controller no longer looks FastAPI-specific, but verifier discovery / verifier execution / repair proposal quality still do not generalize reliably across Rust, Python data, and Node utility tasks.
- Remaining work should focus on verifier environment isolation and patch proposal convergence, not on adding more framework-specific repair branches.

## Implementation Notes

Production verifier repair dispatch is now narrower:

```text
task_contract_verifier_repair_pending && repair_job.is_some()
  -> dispatch_repair_job_step()
  -> RepairJob::begin_next_repair_step()
  -> RunDiagnostic | RunPatchProvider | RunVerifier | SafeStop | Done
```

The removed lower branches were not the desired authority anymore. Their responsibilities are now owned by `dispatch_repair_job_step()`.

The active-job policy projection is now:

```text
LoopControlAction::ContinueRepairJob
  -> verifier_repair_policy_for_next_action()
  -> EffectiveToolPolicy
```

New regression tests cover:

- direct target-hint policy mapping
- active-job branch reading `RepairJob::next_action()`
- active repair job winning over verifier run, artifact flow, and missing-verifier state
- stale pending flag without an owner not creating legacy repair dispatch

Test-only compatibility remains for `verifier_weak` safe-stop emission because the existing safe-stop E2E tests still use the seam.
