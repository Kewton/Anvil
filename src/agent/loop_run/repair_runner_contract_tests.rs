//! Repair-runner contract tests extracted from `turn.rs` (parent #680).
//!
//! Hosts two `#[cfg(test)]` mods originally embedded near the top of
//! `turn.rs`:
//!
//! - `repair_lifecycle_event_tests` — pure-fn timeout / shadow-validation
//!   assertions on `repair_driver` + `verifier_orchestration` types.
//! - `v0421_repair_runner_contract_tests` — source-string grep assertions
//!   that pin production invariants in `turn.rs` /
//!   `verifier_orchestration.rs` / `actor_loop_flow.rs`.
//!
//! The contract-test mod uses `include_str!("turn.rs")` etc; sibling
//! placement keeps that resolution intact (path is resolved relative to
//! the source file containing the macro). Both mods stay `#[cfg(test)]`
//! so they are excluded from the production binary. No facade re-export
//! (DR3-001).

#[cfg(test)]
mod repair_lifecycle_event_tests {
    use std::time::Duration;

    use super::super::repair_driver::{
        VERIFIER_REPAIR_PASS_TIMEOUT_SECS, VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS,
        verifier_repair_pass_attempt_timeout_secs,
    };
    use super::super::verifier_orchestration::PatchProposalShadowValidation;

    #[test]
    fn repair_pass_attempt_timeout_is_capped_by_attempt_timeout() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(0)),
            Some(VERIFIER_REPAIR_PASS_TIMEOUT_SECS)
        );
    }

    #[test]
    fn repair_pass_attempt_timeout_uses_remaining_wall_clock_budget() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(
                VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS - 10
            )),
            Some(10)
        );
    }

    #[test]
    fn repair_pass_attempt_timeout_expires_at_wall_clock_limit() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(
                VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS
            )),
            None
        );
    }

    #[test]
    fn patch_shadow_old_string_not_found_defers_to_production_validator() {
        let validation = PatchProposalShadowValidation {
            status: "rejected",
            reason: "old_string_not_found",
        };

        assert!(!validation.is_decisive());
    }

    #[test]
    fn patch_shadow_target_mismatch_remains_decisive() {
        let validation = PatchProposalShadowValidation {
            status: "rejected",
            reason: "target_mismatch",
        };

        assert!(validation.is_decisive());
    }
}

#[cfg(test)]
mod v0421_repair_runner_contract_tests {
    fn function_body<'a>(src: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
        let start = src.rfind(start_marker).expect("start marker must exist");
        let end = src[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .expect("end marker must exist after start marker");
        &src[start..end]
    }

    #[test]
    fn repair_job_run_verifier_dispatch_uses_job_preserving_path() {
        // After parent #680 vertical-slice extraction, the dispatch lives
        // in `repair_job_dispatch.rs` as a free fn. The job-preserving
        // path is `drive_repair_job_verifier(agent, args)`; the legacy
        // re-entry path through verifier_orchestration's session-rebuild
        // must stay absent.
        let src = include_str!("repair_job_dispatch.rs");
        let dispatch = function_body(
            src,
            "\npub(super) fn dispatch_repair_job_step(",
            "\nfn drive_repair_job_verifier(",
        );

        assert!(dispatch.contains("RepairStep::RunVerifier"));
        assert!(dispatch.contains("drive_repair_job_verifier(agent, args)"));
        assert!(
            !dispatch.contains(
                "super::verifier_orchestration::drive_task_contract_verifier(agent, args)"
            ),
            "repair job verifier rerun must not re-enter the job-rebuilding verifier flow"
        );
    }

    #[test]
    fn repair_patch_provider_requires_committed_target_hint() {
        let src = include_str!("verifier_orchestration.rs");
        let body = function_body(
            src,
            "\npub(super) fn run_verifier_repair_pass_and_apply(",
            "\npub(super) fn record_controller_verifier_repair_invalid(",
        );

        assert!(body.contains("target_hint:"));
        assert!(body.contains("&RecoveryTargetHint"));
        assert!(
            !body.contains("verifier_repair_effective_target_hint(&context).cloned()"),
            "patch provider must consume the committed RepairStep target, not recalculate it"
        );
    }

    #[test]
    fn deterministic_repair_candidate_is_not_called_by_patch_provider_main_path() {
        let src = include_str!("verifier_orchestration.rs");
        let body = function_body(
            src,
            "\npub(super) fn run_verifier_repair_pass_and_apply(",
            "\npub(super) fn record_controller_verifier_repair_invalid(",
        );

        assert!(
            !body.contains("controller_repair_candidate_for_job("),
            "patch provider must not call the legacy deterministic repair helper"
        );
        assert!(
            !body.contains("agent.verifier_repair_candidate.assist"),
            "legacy deterministic repair assist must stay out of the production patch provider"
        );
        assert!(
            !body.contains("agent.verifier_repair_candidate.applied"),
            "deterministic repair candidates must not apply patches from the verifier repair main path"
        );
        assert!(
            !body.contains("agent.verifier_repair_candidate.apply_failed"),
            "deterministic repair candidates are telemetry/validator assist only"
        );
    }

    #[test]
    fn repair_patch_provider_uses_json_mode_and_no_think() {
        let src = include_str!("verifier_orchestration.rs");
        let body = function_body(
            src,
            "\npub(super) fn run_verifier_repair_pass_and_apply(",
            "\npub(super) fn record_controller_verifier_repair_invalid(",
        );
        let orchestration = include_str!("verifier_orchestration.rs");
        let prompt = function_body(
            orchestration,
            "\npub(super) fn verifier_repair_pass_messages(",
            "\npub(super) fn safe_verifier_repair_file_excerpt(",
        );

        assert!(
            body.contains("chat_text_json_control"),
            "patch provider must use Ollama JSON mode so repair replies stay machine-readable"
        );
        assert!(
            prompt.contains("/no_think\\nYou are a short-lived verifier repair editor"),
            "patch provider system prompt must disable model thinking chatter"
        );
    }

    #[test]
    fn plan_admission_without_assessment_forces_re_diagnostic() {
        let event = super::super::repair_plan_admission::admission_error_event(
            &super::super::repair_plan_admission::RepairPlanAdmissionError::MissingDiagnosticAssessment,
        );
        assert!(matches!(
            event,
            super::super::repair_job::RepairJobEvent::DiagnosticMalformed
        ));

        let mut job = super::super::repair_job::RepairJob::new_for_test();
        job.apply_event(event);

        assert_eq!(
            job.next_action(),
            super::super::repair_job::RepairNextAction::RequestDiagnostic
        );
    }

    #[test]
    fn production_repair_dispatch_does_not_call_legacy_decision_bridge() {
        let actor_loop_flow_src = include_str!("actor_loop_flow.rs");
        // After parent #680: `build_arbiter_candidates` lives in
        // `effective_tool_policy_flow.rs` as a `pub(super)` free fn.
        let arbiter_src = include_str!("effective_tool_policy_flow.rs");
        let run_actor_loop = function_body(
            actor_loop_flow_src,
            "\npub(super) fn run_actor_loop(",
            "\n    let mut last_iter = 0usize;\n",
        );
        let arbiter_candidates = function_body(
            arbiter_src,
            "\npub(super) fn build_arbiter_candidates(",
            "\nfn priority_one_arbiter_candidates(",
        );

        for (label, body) in [
            ("run_actor_loop", run_actor_loop),
            ("build_arbiter_candidates", arbiter_candidates),
        ] {
            assert!(
                !body.contains("VerifierRepairDecision"),
                "{label} must not project production repair dispatch through VerifierRepairDecision"
            );
            assert!(
                !body.contains("verifier_repair_decision("),
                "{label} must not call the legacy verifier_repair_decision bridge"
            );
        }
    }
}
