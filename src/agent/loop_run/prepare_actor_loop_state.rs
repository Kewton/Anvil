//! Per-actor-loop-turn state initializer extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts `prepare_actor_loop_turn_state` (pub(super)) — clears the
//! per-turn caps / counters / dedup carriers that the actor loop must
//! observe in a clean state, then computes the initial `TaskContract`
//! and seeds the artifact-recovery target when the contract's pre-loop
//! evaluation already returns `Continue`. Called once from
//! `actor_loop_flow::run_actor_loop`.
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&mut Agent`, matching `actor_loop_flow` / `reply_retry` /
//! `handle_user_message` pattern. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::completion_evidence::EvidenceSet;
use super::task_contract::{CompletionDecision, RequestCarryoverKey, TaskContract};
use crate::modes::plan_act::ExecutionMode;

pub(super) fn prepare_actor_loop_turn_state(agent: &mut Agent) -> Option<TaskContract> {
    // Issue #455 / D4 / CB-001: clear the in-snapshot turn-scoped flag
    // so first-eligible-failure-wins starts fresh on this turn. The
    // flag lives on `SessionSnapshot` itself (`#[serde(skip)]`), is set
    // by every `record_feedback`/`record_feedback_if_unset` that writes
    // an eligible-kind frame, and is consulted by
    // `record_feedback_if_unset` to decide skip-vs-overwrite.
    agent.session.reset_eligible_feedback_recorded_this_turn();
    // Issue #456 / DR2-003: reset the AnvilScore turn-local runtime
    // fields. Inline assignment (no dedicated method) keeps SRP small.
    // `consecutive_no_progress_turns` is session-cumulative and is
    // intentionally NOT reset here.
    agent.session.unsafe_blocks_this_turn = 0;
    agent.session.repo_edit_succeeded_this_turn = false;
    agent.session.touched_files_at_turn_start = agent.session.working_memory.touched_files.clone();
    // Issue #462: reset the per-turn CaseRecord extraction cap.
    agent.session.case_record_extracted_this_turn = false;
    // Issue #579: reset the per-turn FeedbackKind second-pass cap. Mirror
    // of `work_mode_confirm_called_this_turn` semantics — the flag flips
    // to `true` only when the orchestrator actually dispatches to the
    // sidecar (model.is_some()), so skipped / sidecar-unavailable paths
    // never starve subsequent turns of a confirmation attempt.
    agent.feedback_kind_confirm_called_this_turn = false;
    // Issue #580: reset the per-turn Quality-gate second-pass cap AND the
    // per-turn memoization cache. See the field doc for why this adapter
    // is the only one that carries an in-turn cache (5 callsites vs.
    // 1-2 for #576/#579).
    agent.quality_confirm_called_this_turn = false;
    agent.last_quality_confirm_result = None;
    // Issue #463: reset the per-turn case_retrieval cap.
    agent.session.case_retrieval_invoked_this_turn = false;
    // Issue #471: reset the per-turn eval log case retrieval summary.
    agent.last_case_retrieval_summary = None;
    // Issue #464: reset the per-turn anti-pattern caps.
    agent.session.anti_pattern_extracted_this_turn = false;
    agent.session.anti_pattern_retrieval_invoked_this_turn = false;
    // Issue #558: reset photon eval summary (consumed by build_eval_record).
    agent.last_photon_eval_summary = None;
    // Issue #604 Task 5.2: reset the per-turn auto-promote cap flag and
    // outcome cache. Mirror of `case_record_extracted_this_turn` semantics.
    agent.session.auto_promote_called_this_turn = false;
    agent.last_auto_promote_outcome = None;
    // Issue #651 Phase 6.1: reset the per-turn SafeStop telemetry cap
    // so the next user turn can emit `agent.verifier.weak` /
    // `agent.verifier.missing` again if the failure mode repeats.
    agent.session.verifier_safe_stop_emitted_this_turn = false;
    // Issue #664 iteration-2 (CB-001) / iteration-3 (CB2-001) /
    // iteration-4 (CB3-001): consume the cross-turn carryover bound to
    // the current request key, then clear the per-turn flag.
    let promoted = match (
        agent.owned_test_verifier_missing_observed_carryover.take(),
        super::workspace_access::active_request_text(agent),
    ) {
        (Some(stored), Some(current)) => {
            let current_key = RequestCarryoverKey::from_request(&current);
            stored == current_key
        }
        // Missing stored key OR missing current request text → fail
        // closed and do not promote (`active_request_text()` is
        // `None` only when the session has no user-driving
        // message, which can never match a key produced from a
        // real request).
        _ => false,
    };
    agent.owned_test_verifier_missing_observed_this_turn = promoted;
    // Issue #606 (T-1.8): reset the per-turn completion-evidence set so
    // observations never bleed across turns. Push-only `EvidenceSet`
    // populated by the Bash / Edit / Write hooks below; consumed by
    // `success.rs::run_post_loop_success_verifier` via
    // `ProtocolKind::evidence_set_satisfies`.
    agent.evidence_set_this_turn.clear();
    agent.task_contract_evidence_set_this_turn.clear();
    // Issue #636: drop per-turn behavior-coverage excerpts so the
    // current turn never observes a previous turn's edits.
    agent.task_contract_excerpts.clear();
    agent.current_artifact_recovery_target = None;
    // Issue #652: per-turn reset of the artifact completion job state
    // (DR3-003 / per-turn cap pattern). A job is only reconstructed
    // through `set_artifact_recovery_target_from_hint`, so dropping it
    // here cannot leak prior-turn budget into the new turn.
    agent.artifact_completion_job = None;
    agent.task_contract_verifier_repair_pending = false;
    // Issue #647 (SF1 V3.2): mirror reset for the verifier-passed
    // hint that backs `SpecAuthorityInput.has_verified_public_interface`.
    // Lives at the same per-turn reset boundary as the
    // `task_contract_verifier_repair_pending` flag so a previous turn's
    // verifier success cannot leak into the current turn's
    // SpecAuthority resolution.
    agent.task_contract_verifier_passed_this_actor_loop = false;
    agent.repair_job = None;
    // Issue #637 (CB-001): reset the artifact-recovery retry counter at
    // the same per-turn boundary as `repair_job` so a previous turn's
    // `RepairArtifact` increments do not bleed into this turn and prematurely
    // trip `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT`. The legacy
    // `verifier_repair_retries` was a `run_turn`-local `usize` and always
    // started at 0; this restores that semantics for the Agent-field
    // counter.
    agent.repair_job_artifact_attempts = 0;
    // Issue #638 (Task 1.4): clear the turn-local failure snapshot at the
    // same boundary as `repair_job` (design policy §5, A-only).
    agent.repair_failure_snapshot = None;
    // Issue #917: read the per-turn classification authority. This fn returns
    // an owned `Option<TaskContract>`, so clone out of the `Rc` (classification
    // is still computed exactly once — only the result is cloned).
    let task_contract =
        super::task_classification::task_contract_authority(agent).map(|rc| (*rc).clone());
    if agent.session.mode_state.mode != ExecutionMode::Plan
        && let Some(contract) = task_contract.as_ref()
    {
        let initial_decision = contract.evaluate(&EvidenceSet::new());
        if matches!(initial_decision, CompletionDecision::Continue { .. }) {
            super::set_artifact_recovery_target::set_artifact_recovery_target_for_decision(
                agent,
                &initial_decision,
                0,
            );
        }
    }
    task_contract
}
