# Dispatch Source Inventory

## Desired Ownership

| Phase | Owner | Output |
| --- | --- | --- |
| Artifact collection | `TaskContract` + `ArtifactLedger` | missing role / target hint / ready to verify |
| Workspace readiness | `project_probe` / project verifier discovery | verifier readiness facts |
| Verifier execution | `drive_task_contract_verifier` / `run_task_contract_verifier_once` | verifier outcome |
| Verifier repair | `RepairJob::next_action()` | diagnostic / patch / rerun / safe stop / done |
| Tool restriction | `EffectiveToolPolicy` via active job arbiter | allowed tools and target scope |
| Pre-model dispatch | `LoopControlAction` | repair step / verifier run / model turn |

## Current Main Loop Order

1. Interrupt check.
2. Compute `LoopControlAction` from mode, verifier-pending state, active
   `RepairJob`, active `MissingVerifierJob`, and task-contract action.
3. Project `LoopControlAction` into `RecoveryOwner`.
4. Dispatch repair job / missing-verifier job / verifier run before any model
   turn when those owners are active.
5. Only when the controller returns `RequestModelTurn`, request an assistant
   reply.
6. Apply `EffectiveToolPolicy` to the batch.
7. Execute tools and observe evidence.
8. Re-run task contract recovery checks after tool execution.

## Recovery Dispatch Audit

| Branch family | Phase owner | Current gate | Notes |
| --- | --- | --- | --- |
| `dispatch_repair_job_step()` | `RepairJob` | `LoopControlAction::ContinueRepairJob` | The only verifier repair dispatcher. |
| `dispatch_missing_verifier_job_step()` | `MissingVerifierJob` | `LoopControlAction::ContinueMissingVerifierJob` | Owns verifier bootstrap rerun / safe stop. |
| `drive_task_contract_verifier()` | verifier execution | `LoopControlAction::RunVerifier` | Runs only when no active repair / missing-verifier job wins. |
| task-contract `Continue` / `RepairArtifact` | artifact completion | task-contract action match | Pre-verifier bounded recovery. It may emit artifact-specific `missing_repo_edits`; it is not verifier repair. |
| task-contract scaffold fallback | artifact bootstrap | task-contract `Continue` branch | Allowed only as pre-verifier bootstrap. It must not be semantic authority for verifier repair. |
| pre-model mode / game / polish deterministic fallback | ordinary protocol recovery | `RecoveryOwner::allows_deterministic_fallback()` | Blocked for `RepairJob`, `MissingVerifierJob`, and artifact completion. |
| post-tool deterministic polish / quality fallback | ordinary protocol recovery | `RecoveryOwner::allows_deterministic_fallback()` | Blocked for verifier-owned states. |
| format-error deterministic edit fallback | ordinary protocol recovery | `RecoveryOwner::allows_deterministic_fallback()` | Blocked for verifier-owned states before returning a synthetic reply. |
| format-error finish-after-edit | ordinary protocol recovery | `RecoveryOwner::allows_generic_repo_change_recovery()` | Blocked for verifier-owned and artifact-owned states. |
| generic empty / prose-only repo-change retry | ordinary protocol recovery | `RecoveryOwner::allows_generic_repo_change_recovery()` | Blocked for verifier-owned states; artifact completion has its own earlier branch. |
| local LLM small-edit fallback | ordinary / focused recovery | generic branches require `RecoveryOwner::None`; focused reject fallback requires `allows_focused_edit_recovery()` | Blocked for `RepairJob` and `MissingVerifierJob`. |
| focused edit recovery note | focused / artifact recovery | `allows_generic_repo_change_recovery()` around generic branches; artifact branch remains explicit | No verifier-owned focused retry can become a generic missing-repo-edits path. |
| partial-progress retry | ordinary protocol recovery | `RecoveryOwner::allows_generic_repo_change_recovery()` | Blocked for verifier-owned states. |

No production branch in this table is owner-unknown. The intentionally retained
exception is task-contract scaffold fallback: it is classified as artifact
bootstrap, not verifier repair.

## Problem Spots

### Fixed In This Pass

The loop had both:

- the new `dispatch_repair_job_step()` at the top of the iteration
- older hand-written verifier repair branches later in the same iteration

This made the code harder to reason about and kept duplicate dispatch authorities in production code. The older branches were removed.

The active-job verifier repair branch also projected through `VerifierRepairDecision`. That branch now reads `LoopControlAction`, which reads `RepairJob::next_action()` directly when a concrete `RepairJob` exists:

```text
LoopControlAction::ContinueRepairJob
  -> RepairNextAction
  -> EffectiveToolPolicy
  -> tool execution
```

The top-level pre-model loop also uses the same controller boundary:

```text
determine_loop_control_action()
  -> ContinueRepairJob | RunVerifier | ContinueMissingVerifierJob | RequestModelTurn
```

### Still Open

The compatibility bridge is now test-only:

```text
RepairJob::next_action()
  -> VerifierRepairDecision
```

These helpers should be deleted after the new transition-table tests fully cover the legacy assertions.

`MissingVerifierJob` is not represented as a `RepairJob`. It should either become a separate pre-repair state under the same controller or be folded into a verifier bootstrap state.

Artifact completion still has its own retry and budget model. That is correct for pre-verifier work, but the boundary must stay strict: artifact completion must not run while a concrete `RepairJob` is active.

Synthetic E2E coverage is still incomplete. The pure controller tests now prove
owner projection, but the next step is to add end-to-end transition tests that
drive a verifier failure through `FailurePacket -> RepairJob -> next action ->
done/safe stop` without a local LLM.
