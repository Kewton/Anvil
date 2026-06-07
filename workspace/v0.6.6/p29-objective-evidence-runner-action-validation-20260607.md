# P29 Objective Evidence Runner Action Validation

Date: 2026-06-07

## Objective

Move missing-evidence action selection from `ObjectiveEvidenceStage` into `ObjectiveEvidenceRunner`, and verify that controller-owned artifact targets are installed before model requests so objective contracts are not overridden by local small-edit heuristics.

## Changes

- `ObjectiveEvidenceRunner::missing_recovery_action` now decides whether missing evidence should invoke the legacy command verifier.
- `ArtifactAcceptance` and `NotRequired` runners do not invoke command verification.
- Pre-model actor-loop control now syncs `ArtifactRecoveryAction::Continue` / `RepairArtifact` into `current_artifact_recovery_target` and `artifact_completion_job` before the next model request.
- Evidence-gated explicit existing artifacts are admitted as deliverable-present candidates, so an explicit existing `src/lib.rs` can move TDD planning to the missing `tests/password_strength.rs` while still requiring verifier evidence for completion.

## Unit Validation

Passed:

- `cargo test --offline --lib objective_evidence`
- `cargo test --offline --lib objective_lifecycle_stage`
- `cargo test --offline --lib objective_evidence_runner_action_only_commands_invoke_legacy_verifier`
- `cargo test --offline --lib explicit_evidence_gated_existing_candidate_counts_as_deliverable`
- `cargo test --offline --lib pre_model_contract_sync_installs_missing_test_target_before_policy_selection`
- `cargo test --offline --lib controller_state_packet_evidence_command_runs_after_deliverables_exist`
- `cargo test --offline --lib`

Full lib result: `3824 passed; 0 failed`.

## Local LLM Validation

Model: `qwen3.6:27b-coding-mxfp8` through local Ollama.

Cases:

- Coding scaffold: `/private/tmp/anvil-p29-runner-action-coding-state`
  - Result: `done`
  - `classified_task_kind=coding`
  - `completion_reason=verifier_evidence_satisfied`
  - `verify_commands=["cargo test --manifest-path Cargo.toml"]`
  - Observed artifact targets: `src/lib.rs`, then `Cargo.toml`, then verifier.

- Feature improvement: `/private/tmp/anvil-p29-runner-action-feature-state`
  - Result: `done`
  - `classified_task_kind=coding`
  - `completion_reason=verifier_evidence_satisfied`
  - Existing failing tests were repaired and `cargo test --manifest-path Cargo.toml` passed.

- TDD hard case before this fix: `/private/tmp/anvil-p29-runner-action-tdd-state`
  - Result: `tool_call_format_error`
  - Root cause: `local_llm_small_edit_after_read` took over after reading `Cargo.toml`, despite the objective requiring `tests/password_strength.rs`.

- TDD hard case after this fix: `/private/tmp/anvil-p29-runner-action-tdd2-state`
  - Result: `repair_exhausted`
  - Improvement observed: `agent.artifact_recovery_target.selected` was `tests/password_strength.rs`, and active job selected `ArtifactRecovery` with `recovery_job_kind=MissingDeliverableJob`.
  - Remaining issue: after verifier failure, `contract_arbitration.report` identified implementation authority, but the repair pass did not safely switch to `src/lib.rs`; it exhausted on the generated test import error.

- Docs artifact: `/private/tmp/anvil-p29-runner-action-docs-state`
  - Result: `done`
  - `classified_task_kind=docs`
  - `completion_reason=artifact_obligations_satisfied`
  - `verify_commands=[]`

- Data artifact: `/private/tmp/anvil-p29-runner-action-data-state`
  - Result: `done`
  - `classified_task_kind=data`
  - `completion_reason=artifact_obligations_satisfied`
  - `verify_commands=[]`

## Insight

The change closes one concrete architecture gap: ObjectiveContract-owned deliverable targets now reach tool policy before the model request, so work-mode and local small-edit fallback cannot steal the turn from a missing deliverable.

The TDD validation also exposed the next bottleneck: EvidenceFailedJob can know the authoritative repair role but still fail to project that role into a safe repair target. That is the next architecture step; this patch intentionally keeps that separate.
