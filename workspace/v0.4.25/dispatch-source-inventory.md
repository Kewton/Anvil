# Dispatch Source Inventory

## Purpose

This inventory identifies production paths that can decide the next action or
finish the actor loop. The cleanup target is one active-job dispatch source,
with legacy/fallback paths unable to override it.

## Active Dispatch Sources

| Source | Current Owner | Current Role | Target State |
| --- | --- | --- | --- |
| `determine_loop_control_action` | `active_job_arbiter` | Selects pre-model controller action: repair, missing verifier, verifier run, or model turn | Primary pre-model dispatch source |
| `RecoveryOwner::from_control_action` | `active_job_arbiter` | Gates generic retry, focused edit recovery, deterministic fallback | Primary owner projection |
| `RepairJob::next_action` | `repair_job` | Decides diagnostic, patch, rerun verifier, verified done, or safe stop | Sole verifier repair state-machine source |
| `MissingVerifierJob::next_action` | `repair_job` | Decides setup edit, rerun verifier, or safe stop | Sole missing-verifier source |
| `TaskContract::evaluate` / recovery action | `task_contract` | Determines missing artifact roles and verifier readiness | Artifact completion input, not repair owner |
| `effective_tool_policy` | `turn.rs` shell + `active_job_arbiter` | Projects current active job into allowed tools | Thin projection only |

## Production Exit Sources

| Exit | Current Path | Required Owner |
| --- | --- | --- |
| `done` | verifier pass / success verifier | done gate + project verifier |
| `missing_repo_edits` | generic retry/artifact exhaustion | active job or no-owner retry budget |
| `verifier_failed` | verifier failure after repair budget | `RepairJob` or verifier safe stop mapper |
| `repair_exhausted` | repeated rejected/no-progress repair attempts | `RepairJob` |
| `repair_safe_stop` | terminal repair reason | `RepairJob` |
| `safe_stop_verifier_missing` | missing/unbound verifier | project verifier / missing verifier job |
| `safe_stop_verifier_weak` | weak verifier | project verifier |

## Known High-Risk Bypass Classes

- Deterministic fallback materialization after a controller owner exists.
- Focused edit recovery after verifier repair starts.
- Generic repo-change retry after verifier repair starts.
- Legacy repair assessment target fallback filling semantic repair targets
  without explicit admission.
- Safe stop report emit paths that do not include actionable next action.

## Current Fix Applied In v0.4.25

- Moved `LoopControlAction`, `LoopControlInputs`, `RecoveryOwner`, and their
  pure selection helpers from `turn.rs` into `active_job_arbiter.rs`.
- `turn.rs` now imports the dispatch vocabulary rather than defining it.
- This reduces local dispatch vocabulary drift and makes the arbiter module the
  natural home for pre-model control decisions.
- Moved verifier repair plan admission out of `turn.rs` into
  `repair_plan_admission.rs`.
- Admission now owns the conversion from legacy diagnostic brief input to
  `AcceptedRepairPlan`, including authority evidence validation and admission
  rejection-to-`RepairJobEvent` mapping.
- Added owner-gate tests that pin which lower-level recovery paths each
  `RecoveryOwner` may invoke.
- Added `repair_patch_validation.rs` as the first patch-validation extraction.
  It owns accepted-plan target authorization and exposes typed rejection
  reasons; `turn.rs` only maps those reasons to the legacy validation failure
  carrier.
- Added `RecoveryDispatchGate` as the production-facing gate for lower-level
  recovery/fallback permissions. `RecoveryOwner` remains the state label;
  production recovery branches now consult the gate rather than calling
  owner permission methods directly.
- Moved pure repair-intent text/path payload checks into
  `repair_patch_validation.rs`:
  - safe workspace-relative path input;
  - empty/no-op edit detection;
  - edit byte cap;
  - tool/markdown markup rejection;
  - secret introduction rejection;
  - suspicious shell payload rejection outside shell-controlled files.
- Moved the pure duplicate-binding repair guard into
  `repair_patch_validation.rs`; `turn.rs` now only maps the typed rejection to
  the existing duplicate rejection signal.
- Moved filesystem-bound target snapshot validation and cheap candidate-content
  validation into `repair_patch_validation.rs`; `turn.rs` now maps those typed
  errors back into existing validation carriers.
- Moved in-memory repair candidate application into
  `repair_patch_validation.rs`; `turn.rs` now passes normalized edit payloads
  and receives the updated contents plus whitespace-fallback flag.
- Moved weakening rejection message/metadata construction into
  `repair_patch_validation.rs`; detector dispatch remains in `turn.rs` until
  shared Python diagnostic helpers are separated.
- Moved the validated repair edit carrier and hash construction into
  `repair_patch_validation.rs`.
- Added `repair_patch_executor.rs` for the only workspace-mutating step in
  verifier repair patch application. It applies a validated edit after
  checking the target preimage hash still matches.
- Moved repair intent fingerprint generation into `repair_patch_validation.rs`
  so duplicate-intent detection uses the same normalized edit payload shape as
  candidate application.

## Remaining Work

- Replace direct `turn.rs` recovery-owner checks with a smaller
  `ActiveJobDispatch` wrapper where possible.
- Assert in tests that every verifier repair terminal path flows through
  `RepairJob::next_action`.
- Continue deleting legacy bypass paths after they are covered by tests.
- Decide whether to move shared Python diagnostic helpers first, or leave
  weakening detector dispatch in `turn.rs` and extract only the final
  high-level validated-edit assembly wrapper.
- Keep disk write/apply orchestration in `turn.rs` unless a separate
  executor boundary is introduced; patch validation should not silently mutate
  the workspace.
- Current status: an executor boundary exists. `turn.rs` still decides when to
  invoke it; the executor owns the write mechanics and preimage guard.
- Current status: duplicate-intent fingerprinting is now validation-owned;
  `turn.rs` no longer constructs the fingerprint directly in production.
- Current status: post-apply no-op candidate validation is now
  validation-owned; `turn.rs` maps the typed no-op error into the existing
  repair rejection signal.
- Current status: duplicate repair-intent replay detection is now
  validation-owned; `turn.rs` passes the read-only applied-intent history and
  maps the typed duplicate error into the existing repair rejection signal.
