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
- Moved weakening rejection message/metadata construction and file-kind
  detector dispatch into `repair_patch_validation.rs`; the remaining
  `turn.rs` responsibility is the semantic generated-test expectation filter.
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
- Continue moving the shared Python diagnostic/test-weakening helper cluster
  to dedicated modules before moving the remaining semantic generated-test
  weakening filter out of `turn.rs`.
- Consider whether extracting the final high-level validated-edit assembly
  wrapper is worth the coupling cost. The remaining logic is now primarily
  orchestration and legacy-carrier mapping.
- Keep disk write/apply orchestration out of patch validation. The executor
  boundary owns mutation mechanics; `turn.rs` decides when to invoke it.
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
- Current status: parsed repair patch intent data is now owned by
  `repair_patch_validation.rs`; `turn.rs` still parses provider replies and
  orchestrates validation.
- Current status: patch proposal parsing caps, malformed-reply error mapping,
  and proposal-to-intent conversion are now validation-owned; `turn.rs` only
  supplies configured limits through thin wrappers.
- Current status: production patch proposal shaping now calls the validation
  module directly; the `turn.rs` wrappers are test-only compatibility helpers.
- Current status: the pure test-edit `SemanticRepairPlan` gate is now
  validation-owned; `turn.rs` only supplies the test-file classification,
  accepted-plan presence, and optional repair hypothesis.
- Current status: test import-contract evidence admission is now
  validation-owned; `turn.rs` still gathers Python-specific evidence.
- Current status: per-intent path/text validation and edit-payload construction
  are now validation-owned; `turn.rs` only maps typed validation errors back
  into legacy repair rejection carriers.
- Current status: repair-intent list bounds are now validation-owned; the
  actor loop no longer owns empty/too-many intent branches.
- Current status: test/implementation weakening detector dispatch is now
  validation-owned; `turn.rs` still applies the semantic test weakening
  filter before mapping typed metadata into `ValidationFailure`.
- Current status: verifier repair validation result carriers are now
  validation-owned; `turn.rs` imports them for repair pass control and report
  wiring.
- Current status: validation-signal-to-ledger-outcome conversion is now
  validation-owned; `turn.rs` still owns when to record lifecycle events.
- Current status: repair-attempt outcome to lifecycle rejection reason
  projection is now `RepairJob`-owned.
- Current status: string-only invalid patch error to lifecycle event
  classification is now `RepairJob`-owned; `turn.rs` still applies the event
  to the active job.
- Current status: validation failure to telemetry reason-label projection is
  now validation-owned.
- Current status: typed repair-intent validation errors now convert to
  `ValidationFailure` inside `repair_patch_validation.rs`; `turn.rs` no longer
  owns those mapping wrappers.
- Current status: remaining typed validation errors now convert to
  validation-result carriers inside `repair_patch_validation.rs`; `turn.rs`
  still sequences the checks and gathers Python evidence.
- Current status: pure assertion/output parsing is now isolated in
  `repair_assertion_analysis.rs`; `turn.rs` still owns the semantic
  generated-test weakening decision and Python evidence gathering.
- Current status: Python import-contract evidence gathering is now isolated in
  `repair_python_import_evidence.rs`; `turn.rs` still owns the semantic
  generated-test weakening decision and some Python diagnostic helper logic.
- Current status: Python pytest/test-fragment fixture analysis is now isolated
  in `repair_python_test_analysis.rs`; `turn.rs` still owns semantic repair
  authority decisions and the high-level weakening filter.
- Current status: semantic generated-test weakening admission is now isolated
  in `repair_test_weakening_filter.rs`; `turn.rs` only invokes it after the
  generic weakening detector reports patterns.
- Current status: verifier framework/test-runner finding generation is now
  isolated in `repair_framework_findings.rs`; `turn.rs` still owns diagnostic
  prompt assembly and parsed-assessment override application.
- Current status: diagnostic LLM assessment parsing is now isolated in
  `verifier_assessment_parser.rs`; `turn.rs` still owns workspace admission,
  semantic plan construction, and RepairJob state transitions.
- Current status: parsed-assessment framework evidence override is now
  isolated in `verifier_assessment_parser.rs`; `turn.rs` still owns
  diagnostic prompt/file-excerpt assembly.
- Current status: semantic failure report parsing is now isolated in
  `verifier_assessment_parser.rs`; `turn.rs` still owns fallback synthesis,
  repair-plan construction, and RepairJob state updates.
- Current status: semantic fallback synthesis and `SemanticRepairPlan`
  construction are now isolated in `semantic_repair_planning.rs`; `turn.rs`
  still owns diagnostic pass sequencing, admitted-target enrichment, and
  RepairJob state updates.
- Current status: verifier-repair shadow telemetry and legacy brief
  projection are now isolated in `verifier_repair_shadow.rs`; `turn.rs` still
  owns when to emit telemetry.
- Current status: semantic legacy target merge and admitted-target priority
  sorting are now isolated in `semantic_repair_planning.rs`; `turn.rs` still
  owns admission enrichment because it verifies the admission SSOT path.
- Current status: diagnostic target confidence gating and role/failure-kind
  compatible target selection are now isolated in `semantic_repair_planning.rs`;
  `turn.rs` still owns the legacy assessment bridge that calls admission and
  writes RepairJob-facing decisions.
- Current status: verifier repair target/path parsing helpers are now isolated
  in `verifier_repair_targeting.rs`; `turn.rs` still owns workspace admission,
  hint promotion, and stateful repair-target selection.
- Current status: verifier repair target ownership admission is now isolated
  in `repair_target_admission.rs`; `turn.rs` still owns when to invoke the
  admission gate while building diagnostic repair targets.
- Current status: verifier failure signature / compact failure text shaping is
  now isolated in `verifier_failure_signature.rs`; `turn.rs` still owns
  RepairJob context assembly and state writes.
- Current status: diagnostic LLM attempt scheduling is now isolated in
  `verifier_diagnostic_attempt.rs`; `turn.rs` still owns the provider call and
  diagnostic pass state updates.
- Current status: repo-edit-category to artifact-role mapping is now consumed
  through `task_contract::role_from_repo_edit`; the duplicate table in
  `turn.rs` has been removed.
- Current status: existing-file path to `RecoveryTargetHint` conversion is now
  isolated in `verifier_repair_targeting.rs`; `turn.rs` still owns diagnostic
  admission sequencing and RepairJob state updates.
- Current status: diagnostic path and missing setup target promotion are now
  isolated in `verifier_repair_targeting.rs`; `turn.rs` still owns semantic
  repair-plan construction and RepairJob state writes after admission.
- Current status: pytest missing-dependency to missing setup candidate
  generation is now isolated in `verifier_repair_targeting.rs`; `turn.rs`
  consumes the generated hints only for diagnostic prompt context assembly.
- Current status: missing local Python module provider target selection is now
  isolated in `verifier_repair_targeting.rs`; `turn.rs` still owns the
  legacy/semantic assessment bridge that applies the selected target.
- Current status: local import provider preference and stale assertion
  test-retarget selection are now isolated in `verifier_repair_targeting.rs`;
  `turn.rs` still owns assessment bridge sequencing and state writes.
- Current status: semantic failure cluster enrichment is now isolated in
  `semantic_repair_planning.rs`; `turn.rs` still owns diagnostic-pass
  sequencing and RepairJob state writes.
- Current status: verifier-output target candidate parsing and changed-file
  hint generation are now isolated in `verifier_repair_targeting.rs`;
  `repair_job.rs` now owns RepairJob construction and previous-state
  carry-over.
- Current status: verifier failure count parsing is now isolated in
  `verifier_failure_signature.rs`; `repair_job.rs` consumes the count while
  constructing RepairJob state.
- Current status: verifier rerun outcome classification is now isolated in
  `repair_job.rs`; the RepairJob context builder consumes the result while
  constructing the next RepairJob.
- Current status: verifier failure to `RepairJob` context construction is now
  isolated in `repair_job.rs`; `turn.rs` only invokes the builder after
  verifier observation and no longer owns signature/count/carry-over assembly.
- Current status: effective verifier repair target selection is now isolated
  in `repair_job.rs`; `turn.rs` imports it as a projection and one
  `repair_job.rs` to `turn.rs` reverse dependency has been removed.
- Current status: task-contract-facing verifier repair state projection is
  now isolated in `repair_job.rs`; `turn.rs` keeps only a thin adapter that
  passes the Agent-owned pending flag and per-turn edit counters.
- Current status: verifier changed-file aggregation is now isolated in
  `verifier_repair_targeting.rs`; `turn.rs` only decides when to capture repo
  snapshots and passes the normalized list into verifier repair state.
- Current status: effective tool policy types are now isolated in
  `tool_policy.rs`; `active_job_arbiter.rs` no longer depends on `turn.rs`
  for the selected-job policy projection.
- Current status: tool-call history evidence projection is now isolated in
  `tool_history.rs`; `turn.rs` and `repair_job.rs` consume the same helper,
  and the verifier repair decision bridge no longer calls back into `turn.rs`.
- Current status: effective tool-policy enforcement is now isolated in
  `tool_policy.rs`; `turn.rs` invokes policy checks but no longer owns
  focused-edit/artifact-directed/MissingVerifierJob rejection logic.
- Current status: verifier prompt `behavior_contract` payload shaping is now
  isolated in `required_behavior.rs`; `turn.rs` attaches the shaped data but
  no longer owns cap/drop/truncation rules for that untrusted metadata.
