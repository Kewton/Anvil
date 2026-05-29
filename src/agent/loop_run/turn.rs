use super::active_job_arbiter::{
    LoopControlAction, LoopControlInputs, RecoveryDispatchGate, RecoveryOwner,
    build_active_job_selected_payload, determine_loop_control_action,
};
use super::actor_loop_flow::{
    TaskContractVerifierFlowOutcome, build_feedback_for_deterministic_content_fallback,
    format_iteration_status, repair_job_done_outcome, run_actor_loop,
};
use super::auto_test::{
    AutoTestKind, AutoTestRunner, build_agent_verifier_external_import_rejected_payload,
    build_agent_verifier_invoked_payload,
};
use super::completion_evidence::is_repo_edit_no_op;
use super::feedback_kind_confirm::{
    self, FEEDBACK_KIND_CONFIRM_TIMEOUT_SECS, FeedbackKindConfirmInputs,
    FeedbackKindConfirmOutcome, run_feedback_kind_confirm_with_strategy,
};
use super::interrupt::{InterruptEnv, InterruptFlag, InterruptMonitor};
use super::model_request::{build_assistant_request_plan, request_non_streaming_assistant_reply};
use super::repair_driver::{
    VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS, VerifierRepairPassOutcome,
    verifier_repair_pass_timeout_error,
};
use super::repair_job::verifier_repair_context_from_failure;
use super::repair_patch_validation::{
    CheapCheckOutcome, RepairRejectionSignal, ValidationFailure,
    build_verifier_repair_pass_ledger_outcome, validate_accepted_repair_plan_authorizes_target,
};

use super::answer_only_mode::{
    answer_only_script_command_allowed, answer_only_script_execution_fallback_response,
};
use super::case_record_extract::{
    case_record_auto_test_active, case_record_extraction_succeeded, case_record_initial_feedback,
    derive_language_stack,
};
use super::confirmation_flow::{
    effective_turn_index_for_stage, log_feedback_kind_confirm_outcome, log_quality_confirm_outcome,
    log_work_mode_confirm_outcome, override_feedback_kind_from_outcome,
    preflight_feedback_kind_skip_reason, preflight_quality_confirm_skip_reason,
    preflight_work_mode_skip_reason, quality_confirm_cached_result, should_writeback_first_pass,
};
use super::feedback_builders::{
    build_feedback_for_bash, build_feedback_for_edit_failure,
    build_feedback_for_unsafe_block_reason, extract_current_request_paths,
};
use super::file_excerpt::{
    current_file_hash_for_relative_path, open_excerpt_file_nofollow, truncate_on_char_boundary,
    utf8_prefix_respecting_cap,
};
use super::focused_edit_recovery::{
    focused_edit_compact_anchor_note, focused_edit_compact_recovery_anchor,
    focused_edit_exact_anchor_history, focused_edit_exact_recovery_anchor,
    focused_edit_first_slice_note, focused_edit_guidance_note_for_policy, focused_edit_history,
    focused_edit_second_slice_note,
};
use super::path_helpers::normalize_memory_path;
use super::photon_feedback_derive::{
    build_rerun_prompt_hint_if_eligible, request_explicitly_requests_script_execution,
};
use super::plan_mode_helpers::{assistant_model_for_mode, plan_file_alias};
use super::precaution_relevance::select_precautions_for_prompt;
use super::safe_stop_payload::{build_safe_stop_payload, collect_recent_action_labels};
#[cfg(debug_assertions)]
use super::small_helpers::masked_path_hash_bounded_list;
use super::small_helpers::{
    anti_pattern_failed_action_summary, latest_tool_result_since_last_user, raw_mode_safe_text,
    rfc3339_now_utc, user_interrupt_result,
};
use super::spinner::{Spinner, SpinnerStopSignal};
use super::summary::{ExitReason, LoopResult};
use super::tester;
use super::tool_display::progress_path_display;
use super::tool_execution::{
    failed_outcome_for_call, rejected_outcome_for_call, success_outcome_for_call,
};
use super::tool_history::{
    build_recent_tool_summary, focused_edit_target_already_read, focused_read_target_for_directory,
    has_successful_non_plan_repo_edit,
    has_successful_non_plan_repo_edit_after_latest_truncated_tool_call,
    is_preferred_read_edit_target, latest_successful_read_existing_path, latest_user_turn_slice,
    latest_verifier_repair_note_index, recent_truncated_tool_call_attempt,
    successful_non_plan_repo_edit_count,
};
use super::tool_policy::{
    EffectiveToolPolicy, EffectiveToolPolicyReason, FocusedEditPolicy,
    effective_tool_policy_error_for_call_with_scope, focused_edit_policy_violation_feedback_note,
    workspace_relative_path_for_tool_arg,
};
use super::verifier_diagnostic_attempt::{
    VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT, verifier_diagnostic_attempt_spec,
};
use super::verifier_driver::TaskContractVerifierOutcome;
use super::verifier_orchestration::{
    JobInstallOutcome, PreparedVerifierDiagnosticPass, PreparedVerifierRepairPass,
    TaskContractVerifierFlowArgs, VerifierDiagnosticPassOutcome, VerifierRepairAttemptProgress,
    build_task_contract_verifier_exit_zero_evidence,
    build_task_contract_verifier_exit_zero_evidence_bound, build_verifier_exit_zero_evidence,
    emit_patch_proposal_legacy_validation_comparison_event,
    emit_patch_proposal_shadow_validation_event, emit_repair_progress_classified_event,
    repair_terminal_exit_reason, synthesized_missing_implementation_target_path_for_request,
    synthesized_missing_test_target_path_for_request, task_contract_needs_verification,
    task_contract_no_verifier_note, task_contract_verifier_failure_attempt_limit,
    task_contract_verifier_repair_note, task_contract_verifier_targeted_edit_required_note,
    test_target_path_compatible_with_request, validate_verifier_repair_intents_with_accepted_plan,
    verifier_diagnostic_messages, verifier_repair_context_diagnostics,
    verifier_repair_diagnostic_pending_note, verifier_repair_intent_limits,
    verifier_repair_pass_messages, verifier_repair_pass_request_error_message,
    verifier_repair_policy_for_target_hint, verifier_repair_safe_stop_message,
    verifier_repair_target_display, verifier_repair_transition_message,
    verifier_repair_unsafe_target_message, verifier_setup_policy_message,
};
use super::verifier_repair_shadow::legacy_repair_brief_input_from_assessment;
use super::verifier_repair_targeting::changed_files_for_verifier;
use super::work_mode_confirm::{
    self, WORK_MODE_CONFIRM_TIMEOUT_SECS, WorkModeConfirmInputs, WorkModeConfirmOutcome,
    run_work_mode_confirm_with_strategy,
};
use super::workspace_candidates::existing_workspace_candidate_for_role_in_scope;
use super::workspace_walk::workspace_appears_empty;
use super::*;
use crate::agent::orchestration::verify_repo_progress;
use crate::logging::{log_llm_event, stable_path_hash};
use crate::model_capabilities::model_capabilities;
use crate::modes::plan_act::{ModeClassification, WorkMode, classify_work_mode_json};
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::session::feedback::{FeedbackFrame, FeedbackKind};
use crate::session::precaution::PrecautionStatus;
use crate::session::store::ScaffoldArtifactFileSnapshot;
use crate::tools::registry::{BashErrorClass, ToolSpec};
use crate::util::workspace_paths::is_ignored_workspace_display_path;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::deterministic;
use super::quality::{
    first_existing_impl_target, quality_first_pass_observation, repo_change_request_text,
    request_allows_fast_polish_fallback, request_explicitly_requires_tests,
    request_mentions_unsupported_ui_framework, request_needs_playable_ui_quality_gate,
    workspace_has_unsupported_ui_framework,
};
use super::quality_confirm::{
    self, QUALITY_CONFIRM_TIMEOUT_SECS, QualityConfirmInputs, QualityConfirmOutcome,
    QualityConfirmation, QualityConfirmationSource, run_quality_confirm_with_strategy,
};

/// Maximum number of characters of tool-call arguments retained in trace logs.
pub(super) const LOG_ARGS_MAX_CHARS: usize = 200;

// Issue #634: SSOT for specialized-fallback ログ event 名。emit 側 / test 側の
// 双方が参照し、typo による検証無効化を防ぐ。文字列値そのものは既存テスト互換の
// ため不変。`EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK` は本 Issue で新規追加。
pub(super) const PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD: usize = 2;
pub(super) const TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT: usize = 3;
pub(super) const TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT: usize = 6;
const VERIFIER_DIAGNOSTIC_MAX_PREDICT: usize = 2_048;
pub(super) const USER_INTERRUPT_ERROR: &str = "__anvil_user_interrupt__";

#[derive(Debug, Clone, Copy)]
struct AssistantReplyRetryState {
    downgraded_native_tools: bool,
    retries_remaining: usize,
    tool_call_format_retries_remaining: usize,
    extra_transport_retries: usize,
    transport_retry_count: usize,
    focused_edit_timeout_retry_count: usize,
    tool_call_format_retry_count: usize,
}

enum AssistantReplyRetryDecision {
    Retry,
    ReturnReply(AssistantReply),
    Fail(String),
}

impl AssistantReplyRetryState {
    fn new(chat_retries: usize, message_count: usize) -> Self {
        Self {
            downgraded_native_tools: false,
            retries_remaining: chat_retries,
            tool_call_format_retries_remaining: 2,
            extra_transport_retries: if message_count >= 12 { 4 } else { 2 },
            transport_retry_count: 0,
            focused_edit_timeout_retry_count: 0,
            tool_call_format_retry_count: 0,
        }
    }
}

pub(super) fn extract_filename_with_suffix(text: &str, suffix: &str) -> Option<String> {
    text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '、'
                    | '。'
                    | '，'
                    | '：'
                    | ':'
                    | ';'
            )
    })
    .map(|token| token.trim_matches([',', '.', '。', '、']))
    .find(|token| {
        token.ends_with(suffix)
            && token.len() <= 80
            && !token.contains('/')
            && !token.contains('\\')
            && !token.starts_with('.')
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    })
    .map(ToString::to_string)
}

pub(super) fn write_stdout_rendered(text: &str, trailing_newline: bool) {
    let mut out = io::stdout().lock();
    let rendered = raw_mode_safe_text(text);
    let _ = out.write_all(rendered.as_bytes());
    if trailing_newline {
        let _ = out.write_all(b"\r\n");
    }
    let _ = out.flush();
}

pub(super) fn tool_result_failed(result: &str) -> bool {
    result.starts_with("Error:") || result.contains("\ninterrupted=true\n")
}

/// Issue #555: carries a retrieval message and the IDs of the selected
/// records so the photon mapper can include them without re-parsing the
/// rendered prompt text.
pub(super) struct RetrievalInjection {
    pub message: ConversationMessage,
    pub selected_ids: Vec<String>,
}

pub(super) type WrittenScaffoldArtifacts = (Vec<PathBuf>, Vec<ScaffoldArtifactFileSnapshot>);

/// Issue #580: SSoT memoization key for the Quality-gate second-pass adapter.
/// Hashes `(request, full_content)` with `DefaultHasher` (per design judgement
/// #5: full_content avoids stale reuse when only the middle of a large file
/// changes — the LLM still sees only the head+tail excerpt).
pub(super) fn quality_confirm_cache_key(request: &str, content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    request.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

impl Agent {
    /// Issue #576: SSoT wrapper that classifies user input with
    /// `classify_work_mode_json`, emits the existing
    /// `agent.work_mode.classified` event (now with `turn_index`), then drives
    /// the LLM second-pass confirmation via `maybe_invoke_work_mode_confirm`.
    /// Returns the first-pass classification — the final (possibly LLM-
    /// corrected) work_mode is written into `self.session.mode_state.work_mode`
    /// by the wrapper before this function returns, so the caller can read
    /// `self.session.mode_state.work_mode` immediately afterwards.
    ///
    /// CB-001 (Issue #576 follow-up): when the per-turn confirmation cap has
    /// already been consumed for this user input (e.g. `auto_plan_precheck`
    /// invoked the second-pass first), do NOT overwrite the previously-resolved
    /// `session.mode_state.work_mode` with the new first-pass result.
    /// `maybe_invoke_work_mode_confirm` will then early-return as
    /// `Skipped(PerTurnCapConsumed)` and the confirmed value survives. The
    /// `agent.work_mode.classified` event is still emitted so downstream
    /// observers can see the second classification attempt.
    ///
    /// CB-004 (Issue #576 follow-up): `auto_plan_precheck` runs in
    /// `process_line` BEFORE `handle_user_message` increments
    /// `current_turn_index`, so logging the raw counter would emit a stale value
    /// for the precheck event. The wrapper compensates by logging
    /// `current_turn_index + 1` for that specific stage so the precheck event
    /// shares the same `(session_id, turn_index)` join key as the matching
    /// `turn_start` event and the post-loop AnvilScore event.
    pub(super) fn classify_with_confirmation(
        &mut self,
        input: &str,
        stage_label: &'static str,
    ) -> ModeClassification {
        let classification = classify_work_mode_json(input);
        // CB-001: only write back the first-pass result when the per-turn cap
        // has NOT yet been consumed. Otherwise the previous call already
        // resolved the final mode and we must keep it.
        if should_writeback_first_pass(self.work_mode_confirm_called_this_turn) {
            self.session.mode_state.work_mode = classification.work_mode;
        }
        // CB-004: align `turn_index` with the upcoming `handle_user_message`
        // turn for the pre-`handle_user_message` precheck event.
        let event_turn_index = effective_turn_index_for_stage(stage_label, self.current_turn_index);
        log_llm_event(
            "agent.work_mode.classified",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "turn_index": event_turn_index,
                "input": input,
                "stage": stage_label,
                "work_mode": classification.work_mode.as_str(),
                "intent": classification.intent,
                "confidence": classification.confidence,
                "ambiguity": classification.ambiguity,
                "alternative_gap": classification.alternative_gap,
                "allows_file_edits": classification.allows_file_edits,
                "requires_tests": classification.requires_tests,
                "reason": classification.reason,
                "evidence": &classification.evidence,
                "alternatives": &classification.alternatives,
            }),
        );
        self.maybe_invoke_work_mode_confirm(&classification, input, event_turn_index);
        classification
    }

    /// Issue #576: gate + dispatch the WorkMode second-pass confirmation. Skip
    /// order (DR2-004):
    ///   1. `work_mode_confirm_called_this_turn` (per-turn cap)
    ///   2. Plan mode (caller-decided)
    ///   3. `ANVIL_NO_MODE_CONFIRM` env
    ///   4. `first_pass_has_explicit_no_edit_signal` — handled by the
    ///      orchestrator as `Skipped(ExplicitReadOnly)`.
    ///   5. `should_request_confirmation == false` — handled by the
    ///      orchestrator as `Skipped(HighConfidence)`.
    ///
    /// Sidecar unavailable / timeout / transport / malformed responses map to
    /// `Fallback` (consumes per-turn cap; first-pass work_mode kept).
    ///
    /// CB-004 (Issue #576 follow-up): `turn_index` is passed in by the caller
    /// rather than read from `self.current_turn_index`, so events emitted by
    /// the pre-`handle_user_message` `auto_plan_precheck` stage share the
    /// upcoming-turn join key with the matching `turn_start` events.
    pub(super) fn maybe_invoke_work_mode_confirm(
        &mut self,
        first_pass: &ModeClassification,
        raw_input: &str,
        turn_index: usize,
    ) {
        let session_id = self.session_store.session_id().to_string();
        let sidecar_model = self.models.sidecar.clone();
        let env_disabled =
            work_mode_confirm::work_mode_confirm_disabled(|k: &str| std::env::var(k));
        if let Some(reason) = preflight_work_mode_skip_reason(
            self.work_mode_confirm_called_this_turn,
            self.session.mode_state.mode,
            env_disabled,
        ) {
            let outcome = WorkModeConfirmOutcome::Skipped { reason };
            log_work_mode_confirm_outcome(
                &outcome,
                &session_id,
                sidecar_model.as_deref(),
                turn_index,
                first_pass,
                None,
            );
            return;
        }

        // Build inputs + invoke orchestrator. The orchestrator handles the
        // remaining skip / fallback branches.
        let inputs = WorkModeConfirmInputs {
            first_pass,
            raw_input,
            session_id: &session_id,
            turn_index,
            model: sidecar_model.as_deref(),
        };

        let attempt_started = Instant::now();
        let outcome = self.run_work_mode_confirm_attempt(inputs, &sidecar_model);
        let latency_ms = attempt_started.elapsed().as_millis() as u64;

        if let WorkModeConfirmOutcome::Confirmed(c) = &outcome {
            self.session.mode_state.work_mode = c.mode;
        }

        log_work_mode_confirm_outcome(
            &outcome,
            &session_id,
            sidecar_model.as_deref(),
            turn_index,
            first_pass,
            Some(latency_ms),
        );
    }

    fn run_work_mode_confirm_attempt(
        &mut self,
        inputs: WorkModeConfirmInputs<'_>,
        sidecar_model: &Option<String>,
    ) -> WorkModeConfirmOutcome {
        if let Some(sidecar_name) = sidecar_model.as_ref() {
            // We're about to dispatch — consume the per-turn cap regardless of
            // success/failure (DR4-004) so timeout/malformed/oversized cannot
            // re-trigger another dispatch in the same user-input.
            self.work_mode_confirm_called_this_turn = true;
            let confirm_client = self
                .client
                .clone_with_overrides(WORK_MODE_CONFIRM_TIMEOUT_SECS, 384)
                .ok();
            run_work_mode_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
                Some(c) => c
                    .chat_text(
                        sidecar_name,
                        &[ConversationMessage::user(prompt.to_string())],
                    )
                    .map(|reply| reply.content),
                None => Err("client clone_with_overrides failed".to_string()),
            })
        } else {
            // sidecar_model is None — orchestrator returns Fallback(SidecarUnavailable)
            // without invoking the closure. We do not consume the per-turn cap
            // because the user might transition into a state where the sidecar
            // becomes available later in this same turn (defensive design).
            run_work_mode_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
        }
    }

    /// Issue #579: FeedbackKind second-pass confirmation wrapper. Called from
    /// `success.rs` facade immediately before `record_feedback_if_unset(fb)`
    /// when `VerifierOutcome::AutoTestRan { feedback: Some(_), .. }` is in
    /// hand. Returns `Some(corrected_kind)` only when the orchestrator
    /// resolved `Confirmed(SecondPassOverridden)` AND the LLM-chosen kind
    /// actually differs from the first-pass kind; otherwise returns `None`
    /// and the caller keeps `fb.kind` unchanged.
    ///
    /// Gate evaluation order (DR1-003 / DR2-005):
    ///   1. Plan mode                              → Skip(PlanMode), cap intact
    ///   2. `ANVIL_NO_FEEDBACK_KIND_CONFIRM` env   → Skip(EnvDisabled), cap intact
    ///   3. per-turn cap already consumed          → Skip(PerTurnCapConsumed), cap intact
    ///   4. Otherwise → orchestrator
    ///
    /// Per-turn cap (`feedback_kind_confirm_called_this_turn`) is consumed
    /// here — not inside the orchestrator — because the orchestrator is a
    /// pure function that does not hold `&mut Agent`. The cap is set only
    /// when `model.is_some()` so `Fallback(SidecarUnavailable)` (model None)
    /// remains retryable on a later turn.
    pub(super) fn classify_with_feedback_confirm(
        &mut self,
        first_pass: &FeedbackKind,
        combined_output: &str,
    ) -> Option<FeedbackKind> {
        let session_id = self.session_store.session_id().to_string();
        let sidecar_model = self.models.sidecar.clone();
        let turn_index = self.current_turn_index;
        let combined_bytes = combined_output.len();
        let env_disabled =
            feedback_kind_confirm::feedback_kind_confirm_disabled(|k: &str| std::env::var(k));
        if let Some(reason) = preflight_feedback_kind_skip_reason(
            self.feedback_kind_confirm_called_this_turn,
            self.session.mode_state.mode,
            env_disabled,
        ) {
            let outcome = FeedbackKindConfirmOutcome::Skipped { reason };
            log_feedback_kind_confirm_outcome(
                &outcome,
                &session_id,
                turn_index,
                first_pass,
                sidecar_model.as_deref(),
                combined_bytes,
                None,
            );
            return None;
        }
        let inputs = FeedbackKindConfirmInputs {
            first_pass,
            combined_output,
            session_id: &session_id,
            turn_index,
            model: sidecar_model.as_deref(),
        };
        let attempt_started = Instant::now();
        let outcome = self.run_feedback_kind_confirm_attempt(inputs, &sidecar_model);
        let latency_ms = attempt_started.elapsed().as_millis() as u64;
        let override_kind = override_feedback_kind_from_outcome(&outcome, first_pass);
        log_feedback_kind_confirm_outcome(
            &outcome,
            &session_id,
            turn_index,
            first_pass,
            sidecar_model.as_deref(),
            combined_bytes,
            Some(latency_ms),
        );
        override_kind
    }

    fn run_feedback_kind_confirm_attempt(
        &mut self,
        inputs: FeedbackKindConfirmInputs<'_>,
        sidecar_model: &Option<String>,
    ) -> FeedbackKindConfirmOutcome {
        if let Some(sidecar_name) = sidecar_model.as_ref() {
            self.feedback_kind_confirm_called_this_turn = true;
            let confirm_client = self
                .client
                .clone_with_overrides(FEEDBACK_KIND_CONFIRM_TIMEOUT_SECS, 384)
                .ok();
            run_feedback_kind_confirm_with_strategy(inputs, |prompt| {
                match confirm_client.as_ref() {
                    Some(c) => c
                        .chat_text(
                            sidecar_name,
                            &[ConversationMessage::user(prompt.to_string())],
                        )
                        .map(|reply| reply.content),
                    None => Err("client clone_with_overrides failed".to_string()),
                }
            })
        } else {
            run_feedback_kind_confirm_with_strategy(inputs, |_| {
                Err("sidecar unavailable".to_string())
            })
        }
    }

    /// Issue #580: Quality-gate second-pass confirmation wrapper. Replaces
    /// direct `implementation_quality_issue_for_request(request, content)`
    /// calls in `accepted_repo_change_quality_issue` /
    /// `accepted_repo_change_polish_target`. Signature mirrors the SSoT
    /// wrapper so callsites stay one-line drop-in replacements.
    ///
    /// Gate evaluation order (Skip → Fallback → Confirmed):
    ///   1. Plan mode (caller-host check + defensive 2nd check here)
    ///   2. `ANVIL_NO_QUALITY_CONFIRM` env disabled
    ///   3. per-turn cap consumed AND no cache hit
    ///   4. per-turn cap consumed AND cache hit → return cached `issue`
    ///   5. early fail / all_zero / all_strong / no sidecar / etc. handled
    ///      by `run_quality_confirm_with_strategy` (orchestrator)
    ///
    /// Returns the final `issue` (None = pass, Some = quality gate fail).
    pub(super) fn implementation_quality_issue_with_confirm(
        &mut self,
        request: &str,
        content: &str,
    ) -> Option<String> {
        // SSoT first-pass observation. The wrapper signature `Option<String>`
        // is preserved for the deterministic / non-confirmable paths.
        let observation = quality_first_pass_observation(request, content);

        let session_id = self.session_store.session_id().to_string();
        let sidecar_model = self.models.sidecar.clone();
        let turn_index = self.current_turn_index;
        let content_hash = quality_confirm_cache_key(request, content);
        if let Some(skip_reason) = preflight_quality_confirm_skip_reason(
            self.session.mode_state.mode,
            quality_confirm::quality_confirm_disabled(|k: &str| std::env::var(k)),
            self.quality_confirm_called_this_turn,
        ) {
            if skip_reason == quality_confirm::QualityConfirmSkipReason::PerTurnCapConsumed
                && let Some(cached) = quality_confirm_cached_result(
                    self.last_quality_confirm_result.as_ref(),
                    content_hash,
                )
            {
                let outcome = QualityConfirmOutcome::Confirmed(cached.clone());
                log_quality_confirm_outcome(
                    &outcome,
                    &session_id,
                    sidecar_model.as_deref(),
                    turn_index,
                    &observation,
                    None,
                );
                return cached.issue;
            }
            let outcome = QualityConfirmOutcome::Skipped {
                reason: skip_reason,
            };
            log_quality_confirm_outcome(
                &outcome,
                &session_id,
                sidecar_model.as_deref(),
                turn_index,
                &observation,
                None,
            );
            return observation.issue;
        }

        let will_dispatch =
            sidecar_model.is_some() && should_request_quality_confirmation(&observation);
        let inputs = QualityConfirmInputs {
            observation: &observation,
            request,
            content,
            session_id: &session_id,
            turn_index,
            model: sidecar_model.as_deref(),
        };
        let attempt_started = Instant::now();
        let outcome =
            self.run_quality_confirm_attempt(inputs, sidecar_model.as_deref(), will_dispatch);
        let latency_ms = attempt_started.elapsed().as_millis() as u64;
        let final_issue = self.resolve_quality_confirm_issue(&outcome, content_hash, &observation);
        log_quality_confirm_outcome(
            &outcome,
            &session_id,
            sidecar_model.as_deref(),
            turn_index,
            &observation,
            Some(latency_ms),
        );

        final_issue
    }

    fn run_quality_confirm_attempt(
        &mut self,
        inputs: QualityConfirmInputs<'_>,
        sidecar_model: Option<&str>,
        will_dispatch: bool,
    ) -> QualityConfirmOutcome {
        if let Some(sidecar_name) = sidecar_model {
            if will_dispatch {
                self.quality_confirm_called_this_turn = true;
            }
            let confirm_client = self
                .client
                .clone_with_overrides(QUALITY_CONFIRM_TIMEOUT_SECS, 384)
                .ok();
            run_quality_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
                Some(c) => c
                    .chat_text(
                        sidecar_name,
                        &[ConversationMessage::user(prompt.to_string())],
                    )
                    .and_then(|reply| {
                        if !reply.tool_calls.is_empty() {
                            Err("sidecar reply contained unexpected tool_calls".to_string())
                        } else {
                            Ok(reply.content)
                        }
                    }),
                None => Err("client clone_with_overrides failed".to_string()),
            })
        } else {
            run_quality_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
        }
    }

    fn resolve_quality_confirm_issue(
        &mut self,
        outcome: &QualityConfirmOutcome,
        content_hash: u64,
        observation: &super::quality::QualityFirstPassObservation,
    ) -> Option<String> {
        match outcome {
            QualityConfirmOutcome::Confirmed(c) => {
                self.last_quality_confirm_result = Some((content_hash, c.clone()));
                c.issue.clone()
            }
            QualityConfirmOutcome::Skipped { .. } | QualityConfirmOutcome::Fallback { .. } => {
                if self.quality_confirm_called_this_turn {
                    self.last_quality_confirm_result = Some((
                        content_hash,
                        QualityConfirmation {
                            issue: observation.issue.clone(),
                            reason: None,
                            source: QualityConfirmationSource::FirstPass,
                        },
                    ));
                }
                observation.issue.clone()
            }
        }
    }

    pub(super) fn handle_user_message(&mut self, input: &str, stream_output: bool) -> LoopResult {
        // Start the ESC interrupt monitor for the duration of this turn only —
        // rustyline owns raw mode during the REPL line-edit, so the monitor
        // must live strictly inside `handle_user_message`. Drop at function
        // exit disables raw mode deterministically (AC-2 / AC-3 / R1 / R2).
        let env = InterruptEnv::detect();
        let mut monitor = InterruptMonitor::start(&env);
        // Issue #452: Reminder Sidecar per-turn cap counter. "Turn" is one
        // user message — reset here so a fresh handle_user_message can fire
        // the Reminder once even if the previous turn already did.
        self.reminder_called_this_turn = false;
        // Issue #646 (C1): per-turn ownership reset. Each user turn starts a
        // fresh task — prior-turn edits do NOT auto-confer ownership on the
        // new task. Clearing here also wipes the MissingVerifierJob so
        // verifier retry budgets restart per task, and the
        // `turn_pre_tool_file_hashes` capture cache so stale baselines from
        // a prior turn cannot mask the next turn's first write.
        self.turn_edited_relative_paths.clear();
        self.turn_pre_tool_file_hashes.clear();
        self.missing_verifier_job = None;
        // Issue #654: per-turn dedup marker reset (DR1-006 / DR2-005). The
        // `agent.safe_stop.report` event is emitted at most once per
        // StopReason per turn; clearing here lets a new user turn re-emit
        // the same StopReason if the stop condition recurs.
        self.safe_stop_report_emitted.clear();
        // Issue #660 (Phase C / DD-4 / DR1-007): per-turn diff-based dedup
        // state for the `agent.active_job.selected` event. Reset adjacent to
        // `safe_stop_report_emitted.clear()` so all per-turn dedup state
        // restarts together at turn boundary (locality eases review when
        // adding new per-turn caps). Forces the first selection of the new
        // turn to emit (None → Some triggers emit), so each turn starts the
        // observation series fresh.
        self.last_active_job_selection = None;
        // Issue #666: per-turn fire-once dedup for the four new
        // `agent.{artifact_completion,verification,repair,memory}.report`
        // events. Reset adjacent to `safe_stop_report_emitted.clear()` and
        // `last_active_job_selection = None` so all per-turn dedup state
        // restarts together at turn boundary (locality, CLAUDE.md per-turn
        // rule). NOT serialized.
        self.job_report_dedup_keys.clear();
        // Issue #665 (Phase 6 / S5-006 / S7-002): per-turn diff-based dedup
        // state for `agent.behavior_contract.projected` event. Reset adjacent
        // to `last_active_job_selection = None` so all per-turn dedup state
        // restarts together at turn boundary. Forces the first consumed
        // projection in the new turn to emit.
        self.last_behavior_contract_projection_event = None;
        // Issue #667 (DR1-004 / per-turn rule): clear the PAM advisory
        // decision carrier. `is_some()` is the "decided this turn" predicate;
        // the 2 production chokepoints set this exactly once (DR1-005).
        self.last_pam_decision_this_turn = None;
        // Issue #661 Task 2.6 (DR1-004 / DR1-010): per-turn dedup state for
        // `agent.verifier.invoked` (digest of canonical-JSON payload) and
        // per-turn cap for `agent.verifier.external_import_rejected`. Reset
        // adjacent to `last_active_job_selection = None` so the per-turn
        // reset group stays co-located. Producers land in iteration-3.
        self.last_verifier_invoked_payload_digest = None;
        self.external_import_rejected_emitted_this_turn = false;
        // Issue #459: Tester Skill per-turn cap counter (DR1-004). Mirror of
        // the reminder cap above; reset so a fresh user turn can fire the
        // Tester once even if the previous turn already did.
        self.tester_called_this_turn = false;
        // Issue #652 CB-002: per-turn `ArtifactCompletionJob` exhaustion flag.
        // Reset here so a fresh turn can re-arm the budget once the previous
        // turn's job was either completed or exhausted.
        self.artifact_completion_exhausted_this_turn = false;
        // Issue #652 CB2-003: turn-local dedup flag for
        // `maybe_emit_artifact_completion_failed_diagnostic`. The previous
        // dedup looked at `working_memory.unresolved_errors`, which is
        // *session* state that handle_user_message does NOT clear — so a
        // residual `artifact_completion_failed role=<role>` from the prior
        // turn suppressed the very first emission of the current turn.
        // Resetting a turn-local bool here is the CLAUDE.md per-turn cap
        // pattern; combined with `artifact_completion_exhausted_this_turn`
        // it gives a within-turn single-emit guarantee that does NOT bleed
        // across turn boundaries.
        self.artifact_completion_failed_diagnostic_emitted_this_turn = false;
        // Issue #456: AnvilScore compute happens once per turn, post-loop.
        // The flag flips after the compute so the post-loop Reminder hook
        // sees `CurrentTurn` while the iteration-internal hook sees
        // `PreviousTurn`.
        self.anvil_score_computed_this_turn = false;
        // Issue #473: increment monotonic per-session turn counter so the
        // dataset export can join `agent.reminder.completed` with
        // `agent.anvil_score.computed` events by `(session_id, turn_index)`.
        // Saturating add defends against pathological session lengths.
        //
        // Issue #659 PR-002 (Option B): perform the increment **before** the
        // per-turn ArtifactLedger reset so the upcoming turn index is the
        // post-increment value. Decoupling the increment from the stamp
        // timing prevents an off-by-one where `event_recorded` /
        // `turn_summary` carry `N-1` while every other observability event
        // emitted during the same user turn carries `N`.
        self.current_turn_index = self.current_turn_index.saturating_add(1);
        // Issue #659 (Task 2.2 / PR-002): per-turn ArtifactLedger reset.
        // Lives at the same per-turn boundary as
        // `turn_edited_relative_paths.clear()` / `turn_pre_tool_file_hashes.clear()`
        // above so all artifact-observation state restarts together on a
        // fresh user turn (CLAUDE.md per-turn rule). The upcoming turn
        // index is passed explicitly so the ledger log context stamp
        // shares `(session_id, turn_index)` join keys with sibling
        // observability events.
        let upcoming_turn_index = u32::try_from(self.current_turn_index).unwrap_or(u32::MAX);
        self.clear_per_turn_ledger_state_for_turn(upcoming_turn_index);
        // Issue #556: clear per-turn photon context_pack response.
        self.photon_context_pack_response = None;
        // Issue #558: clear context_pack_id (turn boundary).
        self.last_context_pack_id = None;
        // Live injection: reset adopted item count.
        self.last_photon_adopted_items = 0;
        // Issue #601: reset per-turn counters consumed by Case F no-progress
        // detection. Reset HERE (handle_user_message head) — NOT in
        // `run_actor_loop` head — because the `#[serde(skip, default)]`
        // counters need to be cleared even for entry points that bypass the
        // actor loop (Plan-mode turns skip `invoke_photon_evaluate`, but a
        // subsequent Act turn must still see a clean baseline). Populated at
        // `run_actor_loop` tail; see §5.5 of design v2.
        self.session.iter_count_this_turn = 0;
        self.session.tool_calls_this_turn = 0;
        // Issue #591 (AS-01 / 設計判断 #2): reset the per-turn adopted summary
        // ids HERE — NOT at `run_actor_loop` head. `invoke_photon_evaluate`
        // runs post-loop and reads this field; resetting at the loop entry
        // would wipe the ids before the evaluate hook can consume them.
        self.last_adopted_summary_ids.clear();
        // Issue #594: clear per-turn provenance summary cache. NOTE we do NOT
        // reset `last_photon_context_pack_status` here — `/photon-why` must
        // remain able to surface the previous Act turn's lineage even after a
        // `/plan` mode change (S7-002).
        self.last_injected_seed_provenance.clear();
        // LI-2: reset the one-shot flag here (before invoke_photon_context_pack
        // in run_turn) so the flag set by path (a) is still true when path (b)
        // in build_request_messages runs. Previously this reset lived in
        // run_actor_loop which wiped it before path (b) could check it.
        self.session.context_pack_sent_this_turn = false;
        // Issue #608 Phase α-2 (AP-09): detect rerun-trigger keyword in the
        // user message and re-present the previous turn's verifier command
        // as a model prompt hint. The runnable eligibility guard
        // (`runnable_rerun_hint_for_session`) re-validates the persisted
        // command against the same DR4-002 / DR4-003 invariants that gated
        // its original observation (BuildTest class, no shell control,
        // non-empty / no control chars / no 4096-byte cap hit). Tampered or
        // ineligible commands are silently skipped — the prompt hint is
        // never emitted as a direct Bash dispatch (DR4-001).
        if let Some(hint) = build_rerun_prompt_hint_if_eligible(input, &self.session) {
            self.session
                .messages
                .push(crate::session::store::ConversationMessage::system(hint));
        }
        let result = self.run_turn(input, stream_output, &mut monitor);
        // Issue #663 (Phase B / AD2 / DR1-001): refresh the active
        // `ArtifactCompletionJob` Satisfied state from the ledger
        // projection. This is the SSOT chokepoint — no status guard
        // here, the guard lives inside
        // `ArtifactCompletionJob::record_satisfied_from_ledger`
        // (DR1-001 SSOT集約). Runs BEFORE `maybe_emit_job_reports` so
        // the job state emitted in `ArtifactCompletionReport` reflects
        // ledger-driven Satisfied transitions for the turn.
        self.refresh_artifact_completion_satisfied();
        // Issue #666: emit per-turn structured job reports just before
        // returning. Wrapping the result guarantees emit fires once per
        // turn regardless of how `run_turn` exited (Ok / Err / early
        // return inside the loop). Order: 4 Report → SafeStopReport is
        // preserved because `record_safe_stop_report` has already run by
        // the time `run_turn` returns, so the SafeStopLinkage snapshot
        // here reflects the final state of the turn (DR3-003).
        self.maybe_emit_job_reports();
        result
    }

    /// Issue #663 (AD2 / AD9 / DR1-001): SSOT chokepoint for the
    /// ledger-driven Satisfied transition. Status guard is intentionally
    /// absent here — it lives inside
    /// `ArtifactCompletionJob::record_satisfied_from_ledger`. The function
    /// computes the projection once and delegates to every active job.
    ///
    /// Today the Agent still holds a single `Option<ArtifactCompletionJob>`
    /// (the BTreeMap migration tracked in AD9 is staged for the legacy
    /// counter cleanup follow-up PR); the function therefore iterates the
    /// single-entry option but is shaped so the future BTreeMap
    /// substitution is a single-line replacement.
    pub(super) fn refresh_artifact_completion_satisfied(&mut self) {
        if self.artifact_completion_job.is_none() {
            return;
        }
        let task_contract = match self.active_request_text() {
            Some(req) => super::task_contract::TaskContract::from_request(&req),
            None => return,
        };
        let projection = self
            .artifact_ledger
            .required_artifacts_completed_projection(&task_contract);
        if let Some(job) = self.artifact_completion_job.as_mut() {
            job.record_satisfied_from_ledger(&projection);
        }
    }

    /// Issue #462: post-loop CaseRecord extraction. Pure success-condition,
    /// scrub, and persist; never calls Ollama / sidecars. Per-turn cap is
    /// `case_record_extracted_this_turn` on `SessionSnapshot` (cleared at
    /// `run_turn` head). Failures are logged via `agent.case_record.failed`
    /// and never propagate.
    ///
    /// DR3-002: gathers `verify_commands` from the agent layer (turn.rs)
    /// and passes them into `case_record::extract` via a borrowed slice.
    /// `language_stack` is derived here using `auto_test::has_*` helpers
    /// (also agent-layer) for the same reason.
    pub(super) fn maybe_extract_case_record(
        &mut self,
        stats: &crate::agent::loop_run::summary::LoopStats,
        verify_commands: &[String],
    ) -> Option<crate::session::case_record::CaseRecord> {
        use crate::session::case_record;

        // Plan-mode gate: never extract in Plan mode.
        if self.session.mode_state.mode == ExecutionMode::Plan {
            return None;
        }
        // Per-turn cap.
        if self.session.case_record_extracted_this_turn {
            return None;
        }
        // Disable env.
        if case_record::case_record_disabled(|k| std::env::var(k)) {
            log_llm_event(
                "agent.case_record.disabled",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                }),
            );
            self.session.case_record_extracted_this_turn = true;
            return None;
        }

        // Success condition (Issue #462 spec).
        let Some(score) = self.session.last_anvil_score.clone() else {
            // No AnvilScore computed for this turn (e.g., TransportError) — skip silently.
            return self.finish_case_record_extraction(None);
        };
        let auto_test_active = case_record_auto_test_active(&score);
        // CB-002 (codex review fix): the auto_test success branch now also
        // requires `unsafe_actions_blocked == 0` to prevent a turn where
        // an unsafe command was blocked in the same turn from extracting
        // a CaseRecord solely because build / tests / artifact were green.
        // This matches the verifier-less fallback and
        // `case_photon_bridge::is_eligible_for_promotion`'s full_pass check.
        if !case_record_extraction_succeeded(
            &score,
            self.session.repo_edit_succeeded_this_turn,
            self.session.unsafe_blocks_this_turn,
        ) {
            return self.finish_case_record_extraction(None);
        }

        // language_stack derivation (agent layer; reuses `auto_test::has_*`).
        let language_stack = derive_language_stack(&self.work_root);

        // Build inputs.
        let active_task = self.session.working_memory.active_task.clone();
        let active_precautions: Vec<crate::session::precaution::Precaution> =
            self.session.working_memory.active_precautions.to_vec();
        // initial_feedback: take the kind of the latest recorded feedback as a
        // single-item list (Issue Out of Scope: rich N-frame history is for
        // CBR follow-up Issue).
        let initial_feedback = case_record_initial_feedback(self.session.last_feedback.as_ref());
        let workspace_key = self.session.workspace_key.clone();

        let inputs = case_record::CaseRecordInputs {
            workspace_key: &workspace_key,
            work_root: &self.work_root,
            active_task: active_task.as_deref(),
            language_stack: &language_stack,
            initial_feedback: &initial_feedback,
            active_precautions: &active_precautions,
            changed_files: &stats.changed_files,
            verify_commands,
            anvil_score: &score,
            repo_edit_succeeded_this_turn: self.session.repo_edit_succeeded_this_turn,
            unsafe_blocks_this_turn: self.session.unsafe_blocks_this_turn,
            auto_test_active,
        };

        let started = std::time::Instant::now();
        let Some(record) = case_record::extract(&inputs) else {
            log_llm_event(
                "agent.case_record.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "extract_returned_none",
                }),
            );
            return self.finish_case_record_extraction(None);
        };

        // Dry-run gate (DR3-002 / Issue): extract still runs so log payloads
        // can confirm the would-be case_id.
        if case_record::case_record_dry_run(|k| std::env::var(k)) {
            log_llm_event(
                "agent.case_record.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "dry_run",
                    "case_id": record.case_id,
                }),
            );
            // DR2-008: Issue #604 — return the extracted record even on
            // dry_run so the post-loop auto-promote hook can still see what
            // would have been promoted (dry_run is observability-only).
            return self.finish_case_record_extraction(Some(record));
        }

        let persist_ok = self.persist_case_record(&record, started);
        self.finish_case_record_extraction(persist_ok.then_some(record))
    }

    fn finish_case_record_extraction<T>(&mut self, result: Option<T>) -> Option<T> {
        self.session.case_record_extracted_this_turn = true;
        result
    }

    fn persist_case_record(
        &self,
        record: &crate::session::case_record::CaseRecord,
        started: std::time::Instant,
    ) -> bool {
        use crate::session::case_record;

        let state_root = self.session_store.state_root().to_path_buf();
        match case_record::persist(&state_root, record) {
            Ok(bytes) => {
                let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
                log_llm_event(
                    "agent.case_record.extracted",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "case_id": record.case_id,
                        "bytes": bytes,
                        "compute_ms": compute_ms,
                    }),
                );
                true
            }
            Err(case_record::PersistError::TooLarge { bytes }) => {
                log_llm_event(
                    "agent.case_record.skipped",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "reason": "too_large",
                        "bytes": bytes,
                    }),
                );
                false
            }
            Err(err) => {
                log_llm_event(
                    "agent.case_record.failed",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "error": err.to_string(),
                    }),
                );
                false
            }
        }
    }

    /// Issue #463: build and (when applicable) inject a `Relevant Local Cases:`
    /// system message into the next prompt. Called from the per-iteration
    /// message-build path immediately after `working_memory_message`. Pure-
    /// function retrieval; never calls Ollama / sidecars. Failures are logged
    /// via `agent.case_retrieval.failed` and never propagate.
    pub(super) fn try_inject_case_retrieval_message(&mut self) -> Option<RetrievalInjection> {
        use crate::session::case_record::{
            PrecautionSnapshot, build_task_signature, capture_repo_fingerprint,
        };
        use crate::session::case_retrieval::{self, CaseRetrievalInputs, RetrievalOutcome};

        // 1. Plan mode → skipped(plan_mode), do not consume cap.
        if self.session.mode_state.mode == ExecutionMode::Plan {
            log_llm_event(
                "agent.case_retrieval.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "plan_mode",
                }),
            );
            return None;
        }
        // 2. per-turn cap consumed → skipped(per_turn_cap_consumed).
        if self.session.case_retrieval_invoked_this_turn {
            log_llm_event(
                "agent.case_retrieval.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "per_turn_cap_consumed",
                }),
            );
            return None;
        }
        // 3. Env disable → cap=true, disabled event.
        if case_retrieval::case_retrieval_disabled(|k| std::env::var(k)) {
            self.session.case_retrieval_invoked_this_turn = true;
            log_llm_event(
                "agent.case_retrieval.disabled",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                }),
            );
            return None;
        }

        // 4. Build inputs from the current SessionSnapshot view.
        let language_stack = derive_language_stack(&self.work_root);
        let workspace_key = self.session.workspace_key.clone();
        let active_task = self.session.working_memory.active_task.clone();
        let task_signature = build_task_signature(active_task.as_deref(), &self.work_root);
        let repo_fp = capture_repo_fingerprint(&workspace_key, &self.work_root, &language_stack);
        let touched_files = self.session.working_memory.touched_files.clone();
        let feedback_kind = self.session.last_feedback.as_ref().map(|f| f.kind.clone());
        let prec_snapshots: Vec<PrecautionSnapshot> = self
            .session
            .working_memory
            .active_precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .map(PrecautionSnapshot::from)
            .collect();

        let dry_run = case_retrieval::case_retrieval_dry_run(|k| std::env::var(k));

        let inputs = CaseRetrievalInputs {
            current_task_signature: &task_signature,
            current_language_stack: &language_stack,
            current_repo_fingerprint: &repo_fp,
            current_touched_files: &touched_files,
            current_feedback_kind: feedback_kind,
            current_active_precautions: &prec_snapshots,
        };

        // 5. Consume the cap before retrieve so the failure path also accounts.
        self.session.case_retrieval_invoked_this_turn = true;

        let state_root = self.session_store.state_root().to_path_buf();
        match case_retrieval::retrieve_relevant_cases(&state_root, &inputs, dry_run) {
            Ok(RetrievalOutcome::Completed {
                candidate_count,
                selected,
                skipped_corrupt_count,
                compute_ms,
            }) => {
                let top_score = selected.first().map(|s| s.breakdown.total).unwrap_or(0.0);
                let top_case_id = selected
                    .first()
                    .map(|s| s.record.case_id.clone())
                    .unwrap_or_default();
                let selected_reasons: Vec<&case_retrieval::CaseScoreBreakdown> =
                    selected.iter().map(|s| &s.breakdown).collect();
                log_llm_event(
                    "agent.case_retrieval.completed",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "candidate_count": candidate_count,
                        "selected_count": selected.len(),
                        "top_score": top_score,
                        "top_case_id": top_case_id,
                        "threshold": 0.40_f32,
                        "compute_ms": compute_ms,
                        "skipped_corrupt_count": skipped_corrupt_count,
                        "selected_reasons": selected_reasons,
                    }),
                );
                // Issue #471 / DR2-005: build CaseRetrievalSummary before
                // format_for_prompt consumes `selected`.
                self.last_case_retrieval_summary =
                    Some(crate::session::eval_log::CaseRetrievalSummary {
                        selected: selected.len(),
                        scores: selected.iter().map(|s| s.breakdown.clone()).collect(),
                    });
                // Issue #555: capture selected IDs for photon mapper before
                // format_for_prompt consumes `selected`.
                let selected_ids: Vec<String> =
                    selected.iter().map(|s| s.record.case_id.clone()).collect();
                case_retrieval::format_for_prompt(&selected)
                    .map(ConversationMessage::system)
                    .map(|message| RetrievalInjection {
                        message,
                        selected_ids,
                    })
            }
            Ok(RetrievalOutcome::Skipped {
                reason,
                candidate_count,
                skipped_corrupt_count,
                compute_ms,
            }) => {
                log_llm_event(
                    "agent.case_retrieval.skipped",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "reason": reason.as_log_str(),
                        "candidate_count": candidate_count,
                        "skipped_corrupt_count": skipped_corrupt_count,
                        "compute_ms": compute_ms,
                    }),
                );
                None
            }
            Err(error) => {
                log_llm_event(
                    "agent.case_retrieval.failed",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "error": error,
                    }),
                );
                None
            }
        }
    }

    /// Issue #464: post-loop AntiPattern extraction. Mirrors
    /// `maybe_extract_case_record` but triggers on **failure** turns instead
    /// of success. Upserts a record keyed by (workspace_key, task_signature,
    /// feedback_kind); the second + N-th occurrence increments `repeat_count`.
    /// Pure upsert / scrub / persist; no sidecar / LLM calls.
    pub(super) fn maybe_extract_anti_pattern(&mut self) {
        use crate::session::anti_pattern;

        let Some(frame) = self.anti_pattern_extraction_feedback() else {
            return;
        };
        let kind = frame.kind.clone();

        if anti_pattern::anti_pattern_dry_run(|k| std::env::var(k)) {
            self.log_anti_pattern_skip("dry_run", Some(&kind), None);
            self.finish_anti_pattern_extraction();
            return;
        }

        let summary_owned = anti_pattern_failed_action_summary(&frame);
        let language_stack = derive_language_stack(&self.work_root);
        let active_task = self.session.working_memory.active_task.clone();
        let workspace_key = self.session.workspace_key.clone();
        let touched_files = self.session.working_memory.touched_files.clone();
        let inputs = anti_pattern::AntiPatternRecordInputs {
            workspace_key: &workspace_key,
            work_root: &self.work_root,
            active_task: active_task.as_deref(),
            language_stack: &language_stack,
            touched_files: &touched_files,
            feedback_kind: kind.clone(),
            failed_action_summary: &summary_owned,
        };

        let started = std::time::Instant::now();
        let state_root = self.session_store.state_root().to_path_buf();
        self.log_anti_pattern_extract_result(
            anti_pattern::extract_or_increment(&state_root, &inputs),
            started,
        );
        self.finish_anti_pattern_extraction();
    }

    fn anti_pattern_extraction_feedback(&mut self) -> Option<FeedbackFrame> {
        use crate::session::anti_pattern;

        if self.session.mode_state.mode == ExecutionMode::Plan {
            return None;
        }
        if self.session.anti_pattern_extracted_this_turn {
            return None;
        }
        if anti_pattern::anti_pattern_disabled(|k| std::env::var(k)) {
            log_llm_event(
                "agent.anti_pattern.disabled",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                }),
            );
            self.finish_anti_pattern_extraction();
            return None;
        }
        let Some(frame) = self.session.last_feedback.clone() else {
            self.finish_anti_pattern_extraction();
            return None;
        };
        if !anti_pattern::is_repeat_eligible_kind(&frame.kind) {
            self.finish_anti_pattern_extraction();
            return None;
        }
        Some(frame)
    }

    fn finish_anti_pattern_extraction(&mut self) {
        self.session.anti_pattern_extracted_this_turn = true;
    }

    fn log_anti_pattern_skip(
        &self,
        reason: &str,
        feedback_kind: Option<&FeedbackKind>,
        bytes: Option<usize>,
    ) {
        let mut payload = serde_json::json!({
            "session_id": self.session_store.session_id(),
            "reason": reason,
        });
        if let Some(kind) = feedback_kind {
            payload["feedback_kind"] = serde_json::to_value(kind).unwrap_or_default();
        }
        if let Some(bytes) = bytes {
            payload["bytes"] = serde_json::json!(bytes);
        }
        log_llm_event("agent.anti_pattern.skipped", payload);
    }

    fn log_anti_pattern_extracted(
        &self,
        record: &crate::session::anti_pattern::AntiPatternRecord,
        outcome: &str,
        started: std::time::Instant,
    ) {
        let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
        log_llm_event(
            "agent.anti_pattern.extracted",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "anti_pattern_id": record.anti_pattern_id,
                "outcome": outcome,
                "repeat_count": record.repeat_count,
                "feedback_kind": serde_json::to_value(&record.feedback_kind).unwrap_or_default(),
                "compute_ms": compute_ms,
            }),
        );
    }

    fn log_anti_pattern_extract_result(
        &self,
        result: Result<
            crate::session::anti_pattern::ExtractOutcome,
            crate::session::anti_pattern::PersistError,
        >,
        started: std::time::Instant,
    ) {
        use crate::session::anti_pattern::{ExtractOutcome, PersistError};

        match result {
            Ok(ExtractOutcome::Created(record)) => {
                self.log_anti_pattern_extracted(&record, "created", started);
            }
            Ok(ExtractOutcome::Incremented(record)) => {
                self.log_anti_pattern_extracted(&record, "incremented", started);
            }
            Ok(ExtractOutcome::SkippedIneligibleKind) => {
                self.log_anti_pattern_skip("ineligible_kind", None, None);
            }
            Ok(ExtractOutcome::SkippedNoActiveTask) => {
                self.log_anti_pattern_skip("no_active_task", None, None);
            }
            Err(PersistError::TooLarge { bytes }) => {
                self.log_anti_pattern_skip("too_large", None, Some(bytes));
            }
            Err(err) => {
                log_llm_event(
                    "agent.anti_pattern.failed",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "error": err.to_string(),
                    }),
                );
            }
        }
    }

    /// Issue #464: build and (when applicable) inject an `Avoid Patterns:`
    /// system message into the next prompt. Mirrors
    /// `try_inject_case_retrieval_message` but pulls from
    /// `state_root/anti_patterns/`.
    pub(super) fn try_inject_anti_pattern_message(&mut self) -> Option<RetrievalInjection> {
        use crate::session::anti_pattern::{self, AntiPatternRetrievalInputs, RetrievalOutcome};
        use crate::session::case_record::{build_task_signature, capture_repo_fingerprint};

        // 1. Plan mode → skipped, do not consume cap.
        if self.session.mode_state.mode == ExecutionMode::Plan {
            log_llm_event(
                "agent.anti_pattern.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "plan_mode",
                }),
            );
            return None;
        }
        // 2. per-turn cap consumed.
        if self.session.anti_pattern_retrieval_invoked_this_turn {
            log_llm_event(
                "agent.anti_pattern.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "reason": "per_turn_cap_consumed",
                }),
            );
            return None;
        }
        // 3. Env disable.
        if anti_pattern::anti_pattern_disabled(|k| std::env::var(k)) {
            self.session.anti_pattern_retrieval_invoked_this_turn = true;
            log_llm_event(
                "agent.anti_pattern.disabled",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                }),
            );
            return None;
        }

        // 4. Build inputs.
        let language_stack = derive_language_stack(&self.work_root);
        let workspace_key = self.session.workspace_key.clone();
        let active_task = self.session.working_memory.active_task.clone();
        let task_signature = build_task_signature(active_task.as_deref(), &self.work_root);
        let repo_fp = capture_repo_fingerprint(&workspace_key, &self.work_root, &language_stack);
        let touched_files = self.session.working_memory.touched_files.clone();
        let feedback_kind = self.session.last_feedback.as_ref().map(|f| f.kind.clone());

        let dry_run = anti_pattern::anti_pattern_dry_run(|k| std::env::var(k));
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: &task_signature,
            current_language_stack: &language_stack,
            current_repo_fingerprint: &repo_fp,
            current_touched_files: &touched_files,
            current_feedback_kind: feedback_kind,
        };

        self.session.anti_pattern_retrieval_invoked_this_turn = true;

        let state_root = self.session_store.state_root().to_path_buf();
        match anti_pattern::retrieve_relevant_anti_patterns(&state_root, &inputs, dry_run) {
            Ok(RetrievalOutcome::Completed {
                candidate_count,
                selected,
                skipped_corrupt_count,
                compute_ms,
            }) => {
                let top_score = selected.first().map(|s| s.breakdown.total).unwrap_or(0.0);
                let top_id = selected
                    .first()
                    .map(|s| s.record.anti_pattern_id.clone())
                    .unwrap_or_default();
                log_llm_event(
                    "agent.anti_pattern.retrieved",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "candidate_count": candidate_count,
                        "selected_count": selected.len(),
                        "top_score": top_score,
                        "top_anti_pattern_id": top_id,
                        "threshold": anti_pattern::ANTI_PATTERN_RETRIEVAL_SCORE_THRESHOLD,
                        "compute_ms": compute_ms,
                        "skipped_corrupt_count": skipped_corrupt_count,
                    }),
                );
                // Issue #555: capture selected IDs for photon mapper before
                // format_for_prompt consumes `selected`.
                let selected_ids: Vec<String> = selected
                    .iter()
                    .map(|s| s.record.anti_pattern_id.clone())
                    .collect();
                anti_pattern::format_for_prompt(&selected)
                    .map(ConversationMessage::system)
                    .map(|message| RetrievalInjection {
                        message,
                        selected_ids,
                    })
            }
            Ok(RetrievalOutcome::Skipped {
                reason,
                candidate_count,
                skipped_corrupt_count,
                compute_ms,
            }) => {
                log_llm_event(
                    "agent.anti_pattern.skipped",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "reason": reason.as_log_str(),
                        "candidate_count": candidate_count,
                        "skipped_corrupt_count": skipped_corrupt_count,
                        "compute_ms": compute_ms,
                    }),
                );
                None
            }
            Err(error) => {
                log_llm_event(
                    "agent.anti_pattern.failed",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "error": error,
                    }),
                );
                None
            }
        }
    }

    fn run_turn(
        &mut self,
        input: &str,
        stream_output: bool,
        monitor: &mut InterruptMonitor,
    ) -> LoopResult {
        self.push_user_message(input.to_string());
        if self.session.mode_state.mode != ExecutionMode::Plan {
            // Issue #576: replace direct `classify_work_mode_json` + event
            // emit with the shared `classify_with_confirmation` wrapper. The
            // wrapper emits the existing `agent.work_mode.classified` event
            // (now with `turn_index`) and drives the LLM second-pass via
            // `maybe_invoke_work_mode_confirm`. Final (LLM-corrected when
            // applicable) work_mode lives in `self.session.mode_state.work_mode`.
            let _ = self.classify_with_confirmation(input, "turn_start");
            self.maybe_compact_session(DEFAULT_KEEP_TAIL);
        }
        let _ = self.refresh_plan_stage();

        let mut action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
        if !self.session.mode_state.policy().repo_edit_required {
            action_expectation = recovery::ActionExpectation::None;
        }
        let requires_action = action_expectation != recovery::ActionExpectation::None;

        // [Issue #556] pre-turn photon context_pack hook
        if self.session.mode_state.mode != ExecutionMode::Plan {
            super::photon_feedback_derive::invoke_photon_context_pack(self);
        } else if self.photon.is_some() {
            // Issue #594: surface plan-mode skip via /photon-why.
            self.last_photon_context_pack_status =
                crate::agent::loop_run::PhotonContextPackStatus::PlanMode;
            // CB-003 (Issue #592): Plan-mode skip path must also clear stale
            // inject tracking so a previous Act-turn's seed ids do not survive
            // into a Plan turn and become "visible" to `/photon-thumbs-*`.
            self.last_injected_summary_ids.clear();
            self.last_injected_summary_turn_index = None;
            log_llm_event(
                "agent.photon_context_pack.skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "turn_index": self.current_turn_index,
                    "reason": "plan_mode",
                }),
            );
        }

        run_actor_loop(
            self,
            action_expectation,
            requires_action,
            stream_output,
            false,
            monitor,
        )
    }

    pub(super) fn prepare_actor_loop_turn_state(
        &mut self,
    ) -> Option<super::task_contract::TaskContract> {
        // Issue #455 / D4 / CB-001: clear the in-snapshot turn-scoped flag
        // so first-eligible-failure-wins starts fresh on this turn. The
        // flag lives on `SessionSnapshot` itself (`#[serde(skip)]`), is set
        // by every `record_feedback`/`record_feedback_if_unset` that writes
        // an eligible-kind frame, and is consulted by
        // `record_feedback_if_unset` to decide skip-vs-overwrite.
        self.session.reset_eligible_feedback_recorded_this_turn();
        // Issue #456 / DR2-003: reset the AnvilScore turn-local runtime
        // fields. Inline assignment (no dedicated method) keeps SRP small.
        // `consecutive_no_progress_turns` is session-cumulative and is
        // intentionally NOT reset here.
        self.session.unsafe_blocks_this_turn = 0;
        self.session.repo_edit_succeeded_this_turn = false;
        self.session.touched_files_at_turn_start =
            self.session.working_memory.touched_files.clone();
        // Issue #462: reset the per-turn CaseRecord extraction cap.
        self.session.case_record_extracted_this_turn = false;
        // Issue #579: reset the per-turn FeedbackKind second-pass cap. Mirror
        // of `work_mode_confirm_called_this_turn` semantics — the flag flips
        // to `true` only when the orchestrator actually dispatches to the
        // sidecar (model.is_some()), so skipped / sidecar-unavailable paths
        // never starve subsequent turns of a confirmation attempt.
        self.feedback_kind_confirm_called_this_turn = false;
        // Issue #580: reset the per-turn Quality-gate second-pass cap AND the
        // per-turn memoization cache. See the field doc for why this adapter
        // is the only one that carries an in-turn cache (5 callsites vs.
        // 1-2 for #576/#579).
        self.quality_confirm_called_this_turn = false;
        self.last_quality_confirm_result = None;
        // Issue #463: reset the per-turn case_retrieval cap.
        self.session.case_retrieval_invoked_this_turn = false;
        // Issue #471: reset the per-turn eval log case retrieval summary.
        self.last_case_retrieval_summary = None;
        // Issue #464: reset the per-turn anti-pattern caps.
        self.session.anti_pattern_extracted_this_turn = false;
        self.session.anti_pattern_retrieval_invoked_this_turn = false;
        // Issue #558: reset photon eval summary (consumed by build_eval_record).
        self.last_photon_eval_summary = None;
        // Issue #604 Task 5.2: reset the per-turn auto-promote cap flag and
        // outcome cache. Mirror of `case_record_extracted_this_turn` semantics.
        self.session.auto_promote_called_this_turn = false;
        self.last_auto_promote_outcome = None;
        // Issue #651 Phase 6.1: reset the per-turn SafeStop telemetry cap
        // so the next user turn can emit `agent.verifier.weak` /
        // `agent.verifier.missing` again if the failure mode repeats.
        self.session.verifier_safe_stop_emitted_this_turn = false;
        // Issue #664 iteration-2 (CB-001): reset the per-turn Stage A
        // observation flag. The flag is set when
        // `run_task_contract_verifier_once` observes
        // `OwnedTestVerifierPlan::Missing` (single producer); read by
        // `build_arbiter_candidates` as the SetupBootstrap signal Stage A.
        //
        // Issue #664 iteration-3 (CB2-001): consume the cross-turn
        // carryover **before** the per-turn reset clears it. The
        // carryover is set on the previous turn's
        // `OwnedTestVerifierPlan::Missing` arm (which immediately
        // exits via `SafeStop`), so the **only** opportunity for the
        // SetupBootstrap arbiter to see the signal is the head of the
        // next user-message turn. Promote the carryover into
        // `_this_turn`, then clear the carryover so it never
        // accumulates across multiple SafeStop cycles.
        //
        // Issue #664 iteration-4 (CB3-001): the promotion is now
        // request-bound. Compute the current turn's
        // `RequestCarryoverKey` (16-hex digest of
        // `mask_secrets(active_request_text)`) and compare to the
        // stored key. Promote only on equality — a topic switch
        // clears the carryover without promotion so the stale
        // Stage A signal cannot grant the Bash-only
        // `setup_bootstrap` policy to an unrelated request. The
        // carryover field is always cleared so it is consumed
        // exactly once (single-shot invariant preserved from
        // iteration-3).
        let promoted = match (
            self.owned_test_verifier_missing_observed_carryover.take(),
            self.active_request_text(),
        ) {
            (Some(stored), Some(current)) => {
                let current_key = super::task_contract::RequestCarryoverKey::from_request(&current);
                stored == current_key
            }
            // Missing stored key OR missing current request text → fail
            // closed and do not promote (`active_request_text()` is
            // `None` only when the session has no user-driving
            // message, which can never match a key produced from a
            // real request).
            _ => false,
        };
        self.owned_test_verifier_missing_observed_this_turn = promoted;
        // Issue #606 (T-1.8): reset the per-turn completion-evidence set so
        // observations never bleed across turns. Push-only `EvidenceSet`
        // populated by the Bash / Edit / Write hooks below; consumed by
        // `success.rs::run_post_loop_success_verifier` via
        // `ProtocolKind::evidence_set_satisfies`.
        self.evidence_set_this_turn.clear();
        self.task_contract_evidence_set_this_turn.clear();
        // Issue #636: drop per-turn behavior-coverage excerpts so the
        // current turn never observes a previous turn's edits.
        self.task_contract_excerpts.clear();
        self.current_artifact_recovery_target = None;
        // Issue #652: per-turn reset of the artifact completion job state
        // (DR3-003 / per-turn cap pattern). A job is only reconstructed
        // through `set_artifact_recovery_target_from_hint`, so dropping it
        // here cannot leak prior-turn budget into the new turn.
        self.artifact_completion_job = None;
        self.task_contract_verifier_repair_pending = false;
        // Issue #647 (SF1 V3.2): mirror reset for the verifier-passed
        // hint that backs `SpecAuthorityInput.has_verified_public_interface`.
        // Lives at the same per-turn reset boundary as the
        // `task_contract_verifier_repair_pending` flag so a previous turn's
        // verifier success cannot leak into the current turn's
        // SpecAuthority resolution.
        self.task_contract_verifier_passed_this_actor_loop = false;
        self.repair_job = None;
        // Issue #637 (CB-001): reset the artifact-recovery retry counter at
        // the same per-turn boundary as `repair_job` so a previous turn's
        // `RepairArtifact` increments do not bleed into this turn and prematurely
        // trip `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT`. The legacy
        // `verifier_repair_retries` was a `run_turn`-local `usize` and always
        // started at 0; this restores that semantics for the Agent-field
        // counter.
        self.repair_job_artifact_attempts = 0;
        // Issue #638 (Task 1.4): clear the turn-local failure snapshot at the
        // same boundary as `repair_job` (design policy §5, A-only).
        self.repair_failure_snapshot = None;
        let task_contract = self
            .active_request_text()
            .map(|request| super::task_contract::TaskContract::from_request(&request));
        if self.session.mode_state.mode != ExecutionMode::Plan
            && let Some(contract) = task_contract.as_ref()
        {
            let initial_decision =
                contract.evaluate(&super::completion_evidence::EvidenceSet::new());
            if matches!(
                initial_decision,
                super::task_contract::CompletionDecision::Continue { .. }
            ) {
                self.set_artifact_recovery_target_for_decision(&initial_decision, 0);
            }
        }
        task_contract
    }

    pub(super) fn tool_policy_violation_exit_reason(recovery_owner: RecoveryOwner) -> ExitReason {
        if recovery_owner == RecoveryOwner::MissingVerifierJob {
            ExitReason::MissingVerification
        } else if recovery_owner.is_verifier_owned() {
            ExitReason::VerifierFailed
        } else {
            ExitReason::ToolCallFormatError
        }
    }

    pub(super) fn materialize_python_package_markers_for_external_import(
        &mut self,
        stdout: &str,
        stderr: &str,
    ) -> Vec<String> {
        let candidates = super::auto_test::python_package_marker_candidates_for_external_import(
            &self.work_root,
            stdout,
            stderr,
        );
        self.materialize_python_package_marker_candidates(candidates)
    }

    pub(super) fn materialize_python_package_markers_for_owned_test_imports(
        &mut self,
        owned_test_artifacts: &[String],
    ) -> Vec<String> {
        let candidates = super::auto_test::python_package_marker_candidates_for_owned_test_imports(
            &self.work_root,
            owned_test_artifacts,
        );
        self.materialize_python_package_marker_candidates(candidates)
    }

    fn materialize_python_package_marker_candidates(
        &mut self,
        candidates: Vec<String>,
    ) -> Vec<String> {
        let mut created = Vec::new();
        for relative_path in candidates {
            let full_path = self.work_root.join(&relative_path);
            let Some(parent) = full_path.parent() else {
                continue;
            };
            if !parent.is_dir() {
                continue;
            }
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&full_path)
                .is_err()
            {
                continue;
            }
            self.observe_evidence_from_repo_edit(&relative_path);
            log_llm_event(
                "agent.verifier.python_package_marker.created",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "turn_index": self.current_turn_index,
                    "path_hash": stable_path_hash(
                        &crate::session::feedback::mask_secrets(&relative_path)
                    ),
                }),
            );
            created.push(relative_path);
        }
        created
    }

    fn handle_repair_job_verifier_pass(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
        previous_repair_context: super::repair_job::RepairJob,
        command: String,
    ) -> TaskContractVerifierFlowOutcome {
        if task_contract_needs_verification(
            self.session.mode_state.mode,
            args.task_contract,
            &self.task_contract_evidence_set_this_turn,
        ) {
            return TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::MissingVerification,
                error_text: "verifier passed but verifier evidence could not be recorded"
                    .to_string(),
            };
        }
        let safe_command = crate::session::feedback::mask_secrets(&command).replace('`', "\\`");
        if let Some(sanitized) =
            super::verifier_skill::sanitize_verify_command_for_case_record(&command)
        {
            args.task_contract_verify_commands_collected.push(sanitized);
        }
        if let Some(job) = self.repair_job.as_mut() {
            job.apply_event(super::repair_job::RepairJobEvent::VerifierObserved {
                delta: super::repair_job::VerifierDelta::Passed,
            });
        }
        emit_repair_progress_classified_event(
            self.session_store.session_id(),
            Some(&previous_repair_context),
            self.repair_job.as_ref(),
            true,
        );
        self.missing_verifier_job = None;
        *args.verifier_repair_retries = 0;
        self.repair_job_artifact_attempts = 0;
        *args.task_contract_verifier_passed_in_loop = true;
        self.task_contract_verifier_passed_this_actor_loop = true;
        TaskContractVerifierFlowOutcome::Done {
            final_prose: format!(
                "Completed requested repository changes and verified them with `{safe_command}`."
            ),
        }
    }

    fn handle_repair_job_verifier_failure(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
        previous_repair_context: super::repair_job::RepairJob,
        changed_files: &[String],
        command: String,
        output: String,
    ) -> TaskContractVerifierFlowOutcome {
        *args.contract_verification_retries += 1;
        let mut repair_context = verifier_repair_context_from_failure(
            &self.work_root,
            &command,
            &output,
            changed_files,
            *args.contract_verification_retries,
            Some(&previous_repair_context),
        );
        let attempt_limit =
            task_contract_verifier_failure_attempt_limit(Some(&previous_repair_context));
        if *args.contract_verification_retries >= attempt_limit {
            self.repair_job = Some(repair_context);
            super::verifier_orchestration::emit_safe_stop_report_for_repair_exhausted(self);
            return TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::RepairExhausted,
                error_text: format!(
                    "verifier repair budget exhausted: {}\n{}",
                    crate::session::feedback::mask_secrets(&command),
                    crate::session::feedback::mask_secrets(&output)
                ),
            };
        }

        let applied_outcome_promotion = super::repair_job::apply_verifier_rerun_observation(
            &mut repair_context,
            Some(&previous_repair_context),
        );
        *args.contract_verifier_repair_edit_count = Some(args.repo_edit_calls_made_this_turn);
        self.task_contract_verifier_repair_pending = true;
        if let Some(outcome) = repair_context.rerun_outcome {
            log_llm_event(
                "agent.verifier_repair.rerun_classified",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "outcome": outcome.as_str(),
                    "previous_failure_signature": repair_context.previous_failure_signature.as_deref(),
                    "current_failure_signature": repair_context.failure_signature.as_str(),
                    "previous_failure_count": repair_context.previous_failure_count,
                    "current_failure_count": repair_context.failure_count,
                    "job_preserving": true,
                }),
            );
        }
        emit_repair_progress_classified_event(
            self.session_store.session_id(),
            Some(&previous_repair_context),
            Some(&repair_context),
            false,
        );
        self.repair_job = Some(repair_context);
        super::verifier_orchestration::maybe_emit_repair_exhausted_from_promotion(
            self,
            applied_outcome_promotion,
        );
        *args.repo_change_retries = 0;
        *args.verifier_repair_retries = 0;
        self.repair_job_artifact_attempts = 0;
        write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                self.config.max_iterations,
                "Verification failed",
                "Repair job observed verifier failure and updated its state.",
                self.footer.current_cols(),
            ),
            true,
        );
        self.push_system_note(task_contract_verifier_repair_note(
            &command,
            &output,
            *args.contract_verification_retries,
            attempt_limit,
            self.repair_job.as_ref(),
        ));
        TaskContractVerifierFlowOutcome::Continue
    }

    fn handle_repair_job_verifier_no_verifier(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
    ) -> TaskContractVerifierFlowOutcome {
        if let Some(job) = self.repair_job.as_mut() {
            job.apply_event(super::repair_job::RepairJobEvent::VerifierObserved {
                delta: super::repair_job::VerifierDelta::VerifierUnavailable,
            });
        }
        self.task_contract_verifier_repair_pending = true;
        *args.repo_change_retries = 0;
        *args.verifier_repair_retries = 0;
        write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                self.config.max_iterations,
                "Verification missing",
                "Repair job observed that no runnable verifier is available.",
                self.footer.current_cols(),
            ),
            true,
        );
        TaskContractVerifierFlowOutcome::Continue
    }

    fn handle_repair_job_verifier_safe_stop(
        &mut self,
        last_iter: usize,
        reason: super::task_contract::SafeStopReason,
    ) -> TaskContractVerifierFlowOutcome {
        let outcome = super::verifier_orchestration::handle_task_contract_verifier_safe_stop(
            self,
            last_iter,
            reason,
            "repair_job_verifier",
        );
        if let Some(job) = self.repair_job.as_mut() {
            job.apply_event(super::repair_job::RepairJobEvent::VerifierObserved {
                delta: super::repair_job::VerifierDelta::VerifierUnavailable,
            });
        }
        outcome
    }

    pub(super) fn dispatch_repair_job_step(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
        repo_edit_calls_made_this_turn: &mut usize,
    ) -> TaskContractVerifierFlowOutcome {
        let Some(step) = self
            .repair_job
            .as_mut()
            .map(|job| job.begin_next_repair_step())
        else {
            return TaskContractVerifierFlowOutcome::Continue;
        };

        match step {
            super::repair_job::RepairStep::RunDiagnostic => {
                self.handle_repair_job_diagnostic_step(args)
            }
            super::repair_job::RepairStep::RunPatchProvider { target_hint } => self
                .handle_repair_job_patch_provider_step(
                    args,
                    repo_edit_calls_made_this_turn,
                    target_hint,
                ),
            super::repair_job::RepairStep::RunVerifier => self.drive_repair_job_verifier(args),
            super::repair_job::RepairStep::SafeStop { reason } => {
                self.repair_job_safe_stop_outcome(reason)
            }
            super::repair_job::RepairStep::Done => repair_job_done_outcome(),
        }
    }

    fn handle_repair_job_diagnostic_step(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
    ) -> TaskContractVerifierFlowOutcome {
        self.write_repair_job_step_status(
            args.last_iter,
            "Verifier diagnostic",
            "Running short-lived diagnostic LLM pass outside the main session.",
        );
        match super::verifier_orchestration::run_verifier_diagnostic_pass(self) {
            VerifierDiagnosticPassOutcome::Accepted => {
                self.write_repair_job_step_status(
                    args.last_iter,
                    "Verifier diagnostic",
                    "Accepted validated diagnostic result; continuing verifier repair.",
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            VerifierDiagnosticPassOutcome::RetryPending { error } => {
                self.write_repair_job_step_status(
                    args.last_iter,
                    "Verifier diagnostic",
                    &format!("Diagnostic pass failed ({error}); retrying with fallback model."),
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            VerifierDiagnosticPassOutcome::Unavailable { error } => {
                self.emit_safe_stop_report_for_repair_terminal(
                    super::repair_job::RepairTerminalReason::DiagnosticUnavailable,
                );
                TaskContractVerifierFlowOutcome::Exit {
                    reason: ExitReason::VerifierFailed,
                    error_text: format!("verifier repair diagnostic_unavailable: {error}"),
                }
            }
            VerifierDiagnosticPassOutcome::Skipped => TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::VerifierFailed,
                error_text: self.repair_job_diagnostic_skipped_error(),
            },
        }
    }

    fn handle_repair_job_patch_provider_step(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
        repo_edit_calls_made_this_turn: &mut usize,
        target_hint: super::task_contract::RecoveryTargetHint,
    ) -> TaskContractVerifierFlowOutcome {
        self.write_repair_job_step_status(
            args.last_iter,
            "Verifier repair",
            "Running controller-applied repair pass for the selected target.",
        );
        match super::verifier_orchestration::run_verifier_repair_pass_and_apply(self, &target_hint)
        {
            VerifierRepairPassOutcome::Applied { relative_path } => {
                *repo_edit_calls_made_this_turn = repo_edit_calls_made_this_turn.saturating_add(1);
                *args.repo_change_retries = 0;
                *args.verifier_repair_retries = 0;
                self.write_repair_job_step_status(
                    args.last_iter,
                    "Verifier repair",
                    &format!(
                        "Applied controller repair edit to {relative_path}; verifier will rerun."
                    ),
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            VerifierRepairPassOutcome::Invalid {
                error,
                repair_attempt_outcome,
            } => {
                super::verifier_orchestration::record_controller_verifier_repair_invalid(
                    self,
                    &error,
                    repair_attempt_outcome,
                );
                self.dispatch_after_repair_patch_rejection(args.last_iter, error, Some(target_hint))
            }
            VerifierRepairPassOutcome::Unavailable { relative_path } => {
                super::verifier_orchestration::record_controller_verifier_repair_invalid(
                    self,
                    &format!(
                        "verifier repair unavailable: no safe cheap check available for {relative_path}"
                    ),
                    None,
                );
                self.write_repair_job_step_status(
                    args.last_iter,
                    "Verifier repair",
                    &format!(
                        "No safe cheap check available for {relative_path}; continuing through repair job state."
                    ),
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            VerifierRepairPassOutcome::Skipped => TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::VerifierFailed,
                error_text: "verifier repair patch provider skipped after committed dispatch"
                    .to_string(),
            },
        }
    }

    fn write_repair_job_step_status(&self, last_iter: usize, title: &str, message: &str) {
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                self.config.max_iterations,
                title,
                message,
                self.footer.current_cols(),
            ),
            true,
        );
    }

    fn repair_job_diagnostic_skipped_error(&mut self) -> String {
        self.emit_safe_stop_report_for_repair_terminal(
            super::repair_job::RepairTerminalReason::DiagnosticUnavailable,
        );
        "verifier repair diagnostic skipped after committed dispatch".to_string()
    }

    fn repair_job_safe_stop_outcome(
        &mut self,
        reason: super::repair_job::RepairTerminalReason,
    ) -> TaskContractVerifierFlowOutcome {
        self.emit_safe_stop_report_for_repair_terminal(reason);
        TaskContractVerifierFlowOutcome::Exit {
            reason: repair_terminal_exit_reason(reason),
            error_text: format!("verifier repair safe stop: {}", reason.as_str()),
        }
    }

    fn drive_repair_job_verifier(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
    ) -> TaskContractVerifierFlowOutcome {
        let Some(previous_repair_context) = self.repair_job.clone() else {
            return TaskContractVerifierFlowOutcome::Continue;
        };
        let current_verif = verify_repo_progress(args.before_snapshot, &self.work_root);
        let changed_files = changed_files_for_verifier(args.accumulated, &current_verif);
        write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                self.config.max_iterations,
                "Task contract",
                "Running verifier for repair job result.",
                self.footer.current_cols(),
            ),
            true,
        );

        match super::verifier_orchestration::run_task_contract_verifier_once(self, &changed_files) {
            TaskContractVerifierOutcome::Passed { command } => {
                self.handle_repair_job_verifier_pass(args, previous_repair_context, command)
            }
            TaskContractVerifierOutcome::Failed { command, output } => self
                .handle_repair_job_verifier_failure(
                    args,
                    previous_repair_context,
                    &changed_files,
                    command,
                    output,
                ),
            TaskContractVerifierOutcome::NoVerifier => {
                self.handle_repair_job_verifier_no_verifier(args)
            }
            TaskContractVerifierOutcome::Disabled => TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::MissingVerification,
                error_text: "task contract requires verification, but ANVIL_NO_AUTO_TEST is set"
                    .to_string(),
            },
            TaskContractVerifierOutcome::TransportError { error } => {
                TaskContractVerifierFlowOutcome::Exit {
                    reason: ExitReason::TransportError,
                    error_text: error,
                }
            }
            TaskContractVerifierOutcome::SafeStop { reason } => {
                self.handle_repair_job_verifier_safe_stop(args.last_iter, reason)
            }
        }
    }

    pub(super) fn dispatch_missing_verifier_job_step(
        &mut self,
        args: TaskContractVerifierFlowArgs<'_, '_>,
        next_action: super::repair_job::VerifierBootstrapNextAction,
    ) -> Option<TaskContractVerifierFlowOutcome> {
        match next_action {
            super::repair_job::VerifierBootstrapNextAction::RequestSetupEdit => None,
            super::repair_job::VerifierBootstrapNextAction::RerunVerifier => Some(
                super::verifier_orchestration::drive_task_contract_verifier(self, args),
            ),
            super::repair_job::VerifierBootstrapNextAction::SafeStop { reason } => {
                self.emit_safe_stop_report_for_verifier_missing();
                Some(TaskContractVerifierFlowOutcome::Exit {
                    reason: ExitReason::MissingVerification,
                    error_text: reason.to_string(),
                })
            }
        }
    }

    fn dispatch_after_repair_patch_rejection(
        &mut self,
        last_iter: usize,
        error: String,
        attempted_target_hint: Option<super::task_contract::RecoveryTargetHint>,
    ) -> TaskContractVerifierFlowOutcome {
        let Some(action) = self.repair_job.as_ref().map(|job| job.next_action()) else {
            return TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::VerifierFailed,
                error_text: error,
            };
        };
        match action {
            super::repair_job::RepairNextAction::SafeStop { reason } => {
                self.emit_safe_stop_report_for_repair_terminal(reason);
                TaskContractVerifierFlowOutcome::Exit {
                    reason: repair_terminal_exit_reason(reason),
                    error_text: format!(
                        "verifier repair safe stop after rejected patch: {error}. next_action: {}",
                        Self::repair_rejection_next_action(&error)
                    ),
                }
            }
            super::repair_job::RepairNextAction::RequestDiagnostic
            | super::repair_job::RepairNextAction::Replan => {
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Verifier repair",
                        "Rejected invalid controller repair proposal; repair job will re-run diagnostics.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
                let target_changed = attempted_target_hint
                    .as_ref()
                    .map(|attempted| attempted.path != target_hint.path)
                    .unwrap_or(false);
                let note = if target_changed {
                    "Rejected invalid controller repair proposal; repair job selected another target."
                } else {
                    "Rejected invalid controller repair proposal; repair job will retry within its budget."
                };
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Verifier repair",
                        note,
                        self.footer.current_cols(),
                    ),
                    true,
                );
                TaskContractVerifierFlowOutcome::Continue
            }
            super::repair_job::RepairNextAction::RerunVerifier => {
                TaskContractVerifierFlowOutcome::Continue
            }
            super::repair_job::RepairNextAction::VerifiedDone => {
                TaskContractVerifierFlowOutcome::Done {
                    final_prose:
                        "Completed requested repository changes and verified them with the required verifier."
                            .to_string(),
                }
            }
        }
    }

    pub(super) fn repair_rejection_next_action(error: &str) -> &'static str {
        let normalized = error.to_ascii_lowercase();
        if normalized.contains("role_mismatch") {
            return "diagnostic and patch target disagreed; narrow the requested repair target or provide the authoritative file to change";
        }
        if normalized.contains("ambiguous")
            || normalized.contains("authority")
            || normalized.contains("expectation")
        {
            return "clarify the expected behavior or provide authoritative examples before retrying";
        }
        if normalized.contains("malformed") {
            return "retry with a narrower task or simpler verifier output so the patch proposal can be structured safely";
        }
        if normalized.contains("duplicate") || normalized.contains("noop") {
            return "inspect the verifier failure and retry with a different repair target";
        }
        "inspect the verifier diagnostics and retry with a narrower repair target"
    }

    pub(super) fn record_missing_verifier_setup_failure(
        &mut self,
        last_iter: usize,
        reason: &str,
    ) -> bool {
        let Some(job) = self.missing_verifier_job.as_mut() else {
            return false;
        };
        let exhausted = job.record_invalid_setup_attempt();
        let attempt = job.setup_attempts_used as usize;
        let attempt_limit = job.retry_budget as usize;
        if exhausted {
            self.emit_safe_stop_report_for_verifier_missing();
            return true;
        }
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                self.config.max_iterations,
                "Verification missing",
                &format!("Verifier setup still needs an in-scope repository edit ({reason})."),
                self.footer.current_cols(),
            ),
            true,
        );
        self.push_system_note(task_contract_no_verifier_note(
            attempt,
            attempt_limit,
            self.active_request_text().unwrap_or_default().as_str(),
        ));
        false
    }

    fn emit_safe_stop_report_for_repair_terminal(
        &mut self,
        reason: super::repair_job::RepairTerminalReason,
    ) {
        let Some(stop_reason) = reason.safe_stop_reason() else {
            return;
        };
        self.emit_repair_safe_stop_report(stop_reason);
    }

    /// Issue #661 iteration-4 Task 5.2 / DR1-005 emit ownership: pre-spawn
    /// `agent.verifier.invoked` event emit + per-turn dedup. The payload
    /// schema matches design Section 8-1 exactly. See
    /// [`build_agent_verifier_invoked_payload`] for the pure-fn builder
    /// (testable without an Agent instance) and the field-by-field schema.
    ///
    /// Emit ownership rules (DR1-005 / DR1-004):
    /// - Caller MUST have built the snapshot at pre-spawn (before
    ///   `run_structured` spawns `Command::new`)
    /// - `mask_payload_inplace` is the final defence line — applied here
    ///   BEFORE the digest computation so the dedup key matches the
    ///   post-mask representation log consumers see
    /// - Per-turn dedup: same digest as `last_verifier_invoked_payload_digest`
    ///   suppresses re-emit; a different digest emits and replaces the
    ///   field. Reset at `handle_user_message` head clears the digest.
    ///
    /// Returns `true` if the event was emitted, `false` if suppressed by
    /// dedup (used by unit tests; production callers ignore the return
    /// value).
    pub(super) fn emit_agent_verifier_invoked_if_new(
        &mut self,
        snapshot: &super::auto_test::VerifierInvokedSnapshot,
    ) -> bool {
        let mut payload = build_agent_verifier_invoked_payload(
            self.session_store.session_id(),
            self.current_turn_index,
            self.session.iter_count_this_turn,
            snapshot,
        );
        // DR1-004 step 1: apply mask_payload_inplace BEFORE digest so the
        // dedup key matches the post-mask representation log consumers see.
        crate::logging::mask_payload_inplace(&mut payload);
        let digest = crate::logging::compute_payload_digest(&payload);
        if self.last_verifier_invoked_payload_digest == Some(digest) {
            return false;
        }
        self.last_verifier_invoked_payload_digest = Some(digest);
        // log_llm_event masks again — idempotent for already-masked
        // payloads (final defence line invariant).
        log_llm_event("agent.verifier.invoked", payload);
        true
    }

    /// Issue #661 iteration-5 Task 7.3: emit
    /// `agent.verifier.external_import_rejected` event subject to per-turn
    /// cap (`external_import_rejected_emitted_this_turn`). Caller passes the
    /// already-hashed module hashes + their static source labels so raw paths
    /// never reach the payload (DR4-005).
    ///
    /// Returns `true` if emitted, `false` if suppressed by the per-turn cap.
    /// Caller (`run_task_contract_verifier_once`) wires both pre-execution
    /// (PYTHONPATH) and post-execution (stdout/stderr) detection through
    /// this single SSOT.
    pub(super) fn emit_agent_verifier_external_import_rejected_if_first(
        &mut self,
        runner: &str,
        reason: &'static str,
        detected_hashes: &[(&str, &'static str)],
        detected_count: usize,
        detected_truncated: bool,
    ) -> bool {
        if self.external_import_rejected_emitted_this_turn {
            return false;
        }
        self.external_import_rejected_emitted_this_turn = true;
        let mut payload = build_agent_verifier_external_import_rejected_payload(
            self.session_store.session_id(),
            self.current_turn_index,
            runner,
            reason,
            detected_hashes,
            detected_count,
            detected_truncated,
        );
        crate::logging::mask_payload_inplace(&mut payload);
        log_llm_event("agent.verifier.external_import_rejected", payload);
        true
    }

    pub(super) fn record_task_contract_verifier_invocation(
        &mut self,
        command: &str,
        exit_code: Option<i32>,
    ) {
        let redacted = crate::session::feedback::redact_verifier_command_for_storage(command);
        if redacted.trim().is_empty() {
            return;
        }
        self.session.last_verifier_command = Some(redacted.clone());
        self.session.last_verifier_invocation =
            Some(crate::session::store::VerifierInvocationRecord {
                command: redacted,
                exit_code: exit_code.unwrap_or(-1),
                recorded_at: rfc3339_now_utc(),
            });
    }

    pub(super) fn observe_task_contract_verifier_exit_zero(&mut self, command: &str) {
        if let Some(evidence) = build_task_contract_verifier_exit_zero_evidence(command) {
            self.evidence_set_this_turn.push(evidence.clone());
            self.task_contract_evidence_set_this_turn.push(evidence);
            crate::logging::log_completion_evidence_observed(
                self.current_turn_index,
                0,
                "verifier_exit_zero",
                serde_json::json!({
                    "command_class": "build_test",
                    "source": "task_contract_verifier",
                }),
            );
        }
    }

    /// Issue #651 PR-001: structured-runner variant of
    /// `observe_task_contract_verifier_exit_zero`. Records a verifier
    /// success that came through `AutoTestRunner::run_structured`, i.e.
    /// the runner's argv was validated against the owned test artifact
    /// list. The recorded `bound_test_artifacts_count` is the only proof
    /// `TaskContract::evaluate_with_owned_test_artifacts` accepts to
    /// satisfy `test_execution_required = true`.
    pub(super) fn observe_task_contract_verifier_exit_zero_bound(
        &mut self,
        command: &str,
        bound_count: usize,
    ) {
        if let Some(evidence) =
            build_task_contract_verifier_exit_zero_evidence_bound(command, bound_count)
        {
            self.evidence_set_this_turn.push(evidence.clone());
            self.task_contract_evidence_set_this_turn.push(evidence);
            crate::logging::log_completion_evidence_observed(
                self.current_turn_index,
                0,
                "verifier_exit_zero",
                serde_json::json!({
                    "command_class": "build_test",
                    "source": "task_contract_verifier_structured",
                    "bound_test_artifacts_count": bound_count,
                }),
            );
        }
    }

    /// Issue #459: try to invoke the Tester Skill when `AutoTestRunner::detect`
    /// returned None. Returns `true` iff the Tester recorded a FeedbackFrame
    /// (so the caller skips the `NoVerifierAvailable` fallback). Disable
    /// gating (Plan / `ANVIL_NO_TESTER` / per-turn cap / no candidate) is
    /// evaluated here so the orchestrator (`run_tester_with_strategy`) only
    /// sees the run-body inputs.
    ///
    /// `Aborted` outcomes (LLM malformed / approval denied / harness build
    /// failure) consume the per-turn cap and **do not** record a frame —
    /// caller falls through to the `NoVerifierAvailable` fallback per design
    /// § 4-2 ("Skip / Abort の細粒度 variant は同じ branch (= 既存
    /// no_verifier) に集約し、log のみで識別する"). This is the boundary
    /// captured by the bool return.
    pub(super) fn try_invoke_tester(&mut self, changed_files: &[String]) -> bool {
        if let Some(reason) = tester::check_invocation_gate(
            self.tester_called_this_turn,
            self.session.mode_state.mode == ExecutionMode::Plan,
            tester::tester_disabled(|key| std::env::var(key).ok()),
        ) {
            self.log_tester_skip(reason.as_str());
            return false;
        }
        let candidate = match self.detect_tester_candidate(changed_files) {
            Some(candidate) => candidate,
            None => return false,
        };
        let session_dir = self
            .session_store
            .state_root()
            .join("sessions")
            .join(self.session_store.session_id());
        let tmp_tests_root = session_dir.join("tmp-tests");
        let tester_runs_root = session_dir.join("tester-runs");
        if !self.ensure_tester_runs_root(&tester_runs_root) {
            return false;
        }
        let approval_mode =
            tester::tester_approval_mode(self.config.yes_mode, io::stdin().is_terminal());
        self.tester_called_this_turn = true;

        let work_root = self.work_root.clone();
        let session_id = self.session_store.session_id().to_string();
        let run = tester::TesterRun {
            work_root: &work_root,
            tmp_tests_root: &tmp_tests_root,
            tester_runs_root: &tester_runs_root,
            approval_mode,
            plan_mode: false,
            no_tester_env: false,
            session_id: std::borrow::Cow::Owned(session_id.clone()),
        };

        let session_id_for_log = session_id.clone();
        let tester_client = self.client.clone();
        let tester_main_model = self.models.main.clone();
        let llm_call =
            move |prompt: &tester::TesterPrompt| -> Result<String, tester::TesterLlmError> {
                tester::run_tester_llm_call(
                    &tester_client,
                    &tester_main_model,
                    &session_id_for_log,
                    prompt,
                )
            };

        let offline = self.config.offline;
        let run_bash = move |cmd: &str,
                             cwd: &Path,
                             timeout: Option<std::time::Duration>|
              -> Result<crate::tools::bash::BashExecutionOutcome, String> {
            // No cancel_flag propagation: Tester's smoke run sits past the
            // main interrupt monitor scope (post-loop hook). The 30s
            // explicit_timeout still caps wall time.
            //
            // CB-003 (Issue #459): pass `BashEnvPolicy::TesterSanitized` so
            // LLM-generated smoke code cannot read parent-process secrets
            // (`OPENAI_API_KEY`, `GITHUB_TOKEN`, `AWS_*`, anything `*_TOKEN`/
            // `*_SECRET`/`*_PASSWORD`). Only the explicit allowlist in
            // `bash::TESTER_ENV_ALLOWLIST_EXACT` is forwarded.
            crate::tools::bash::run_with_outcome(
                cmd,
                cwd,
                None,
                offline,
                timeout,
                Some(crate::tools::bash::BashEnvPolicy::TesterSanitized),
            )
            .map(|(_, outcome)| outcome)
        };

        let approver = move |mode: tester::ApprovalMode,
                             command: &[String]|
              -> Result<(), tester::AbortReason> {
            // Honour the same write/run/promote 3-gate symmetry: Auto bypass /
            // Forbidden deny / Interactive y/N. Production prompt goes through
            // `prompt_for_approval(stdout, stdin)` so both ends are real TTY
            // streams; CI takes the Forbidden branch above.
            let mut stdout = std::io::stdout().lock();
            let stdin_handle = std::io::stdin();
            let mut stdin = stdin_handle.lock();
            tester::prompt_for_approval(mode, command, &mut stdout, &mut stdin)
        };

        let outcome =
            tester::run_tester_with_strategy(run, candidate, llm_call, run_bash, approver);
        self.handle_tester_outcome(outcome, &session_id)
    }

    fn log_tester_event(&self, event: &'static str, payload: serde_json::Value) {
        log_llm_event(event, payload);
    }

    fn log_tester_skip(&self, reason: &str) {
        self.log_tester_event(
            "agent.tester.skipped",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "skip_reason": reason,
            }),
        );
    }

    fn detect_tester_candidate(&self, changed_files: &[String]) -> Option<tester::TesterCandidate> {
        let candidate = tester::TesterCandidate::detect(&self.work_root, changed_files);
        if candidate.is_none() {
            self.log_tester_skip(tester::NotInvokedReason::NoCandidate.as_str());
        }
        candidate
    }

    fn ensure_tester_runs_root(&self, tester_runs_root: &Path) -> bool {
        if let Err(err) = std::fs::create_dir_all(tester_runs_root) {
            self.log_tester_event(
                "agent.tester.failed",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "failure_reason": format!("mkdir tester-runs: {err}"),
                }),
            );
            return false;
        }
        true
    }

    fn handle_tester_outcome(&mut self, outcome: tester::TesterOutcome, session_id: &str) -> bool {
        match outcome {
            tester::TesterOutcome::Recorded(frame) => {
                let kind_value =
                    serde_json::to_value(&frame.kind).unwrap_or(serde_json::Value::Null);
                self.session.record_feedback_if_unset(frame);
                self.log_tester_event(
                    "agent.tester.completed",
                    serde_json::json!({
                        "session_id": session_id,
                        "feedback_kind": kind_value,
                    }),
                );
                true
            }
            tester::TesterOutcome::NotInvoked(reason) => {
                self.log_tester_skip(reason.as_str());
                false
            }
            tester::TesterOutcome::Aborted(reason) => {
                self.log_tester_event(
                    "agent.tester.failed",
                    serde_json::json!({
                        "session_id": session_id,
                        "failure_reason": reason.as_str(),
                        "detail": reason.detail().map(|d| tester::sanitize_tester_log(d, tester::TESTER_LOG_CAP)),
                    }),
                );
                false
            }
        }
    }

    pub(super) fn request_assistant_reply_with_retry(
        &mut self,
        stream_output: bool,
        interrupt_flag: &InterruptFlag,
        recovery_dispatch_gate: RecoveryDispatchGate,
    ) -> Result<AssistantReply, String> {
        // Issue #430 Phase D: freeze the footer for the entire LLM call (the
        // thinking spinner writes to stderr, but stream chunks land on stdout
        // and would otherwise race the footer rewrite). Guard drops on
        // function exit alongside the spinner, restoring redraws.
        let _footer_freeze = self.footer.freeze_for_inference();
        // Start spinner once at function entry; retries share the same
        // animation (no flicker between attempts). Dropped automatically on
        // function exit (Ok / Err / early-return), clearing the line.
        let sp = Spinner::start(format!("thinking... ({})", self.current_assistant_model()));
        let mut retry_state =
            AssistantReplyRetryState::new(self.config.chat_retries, self.session.messages.len());
        loop {
            // Only streaming paths need first-chunk stop; oneshot blocks until
            // the whole reply is assembled so Drop is sufficient.
            let stop_signal = sp.stop_signal();
            match self.request_assistant_reply(stream_output, stop_signal, interrupt_flag) {
                Ok(reply) => return Ok(reply),
                Err(err) => match self.handle_assistant_reply_retry_error(
                    err,
                    recovery_dispatch_gate,
                    &mut retry_state,
                )? {
                    AssistantReplyRetryDecision::Retry => continue,
                    AssistantReplyRetryDecision::ReturnReply(reply) => return Ok(reply),
                    AssistantReplyRetryDecision::Fail(err) => return Err(err),
                },
            }
        }
    }

    fn handle_assistant_reply_retry_error(
        &mut self,
        err: String,
        recovery_dispatch_gate: RecoveryDispatchGate,
        retry_state: &mut AssistantReplyRetryState,
    ) -> Result<AssistantReplyRetryDecision, String> {
        if err == USER_INTERRUPT_ERROR {
            return Ok(AssistantReplyRetryDecision::Fail(err));
        }
        if self.maybe_disable_native_tools_after_request_error(&err, retry_state) {
            return Ok(AssistantReplyRetryDecision::Retry);
        }
        if let Some(decision) = self.maybe_handle_assistant_reply_format_error(
            &err,
            recovery_dispatch_gate,
            retry_state,
        )? {
            return Ok(decision);
        }
        if let Some(decision) = self.maybe_handle_assistant_reply_timeout_error(
            &err,
            recovery_dispatch_gate,
            retry_state,
        ) {
            return Ok(decision);
        }
        if let Some(decision) =
            self.maybe_handle_assistant_reply_transport_error(&err, retry_state)?
        {
            return Ok(decision);
        }
        Ok(self.finish_assistant_reply_retry(err, retry_state))
    }

    fn maybe_disable_native_tools_after_request_error(
        &mut self,
        err: &str,
        retry_state: &mut AssistantReplyRetryState,
    ) -> bool {
        if self.native_tools_enabled
            && !retry_state.downgraded_native_tools
            && (lifecycle::is_native_tool_parser_failure(err)
                || lifecycle::is_native_tool_transport_failure(err))
        {
            retry_state.downgraded_native_tools = true;
            self.disable_native_tools_for_session();
            return true;
        }
        false
    }

    fn maybe_handle_assistant_reply_format_error(
        &mut self,
        err: &str,
        recovery_dispatch_gate: RecoveryDispatchGate,
        retry_state: &mut AssistantReplyRetryState,
    ) -> Result<Option<AssistantReplyRetryDecision>, String> {
        if let Some(reply) =
            super::scaffold_pipeline::maybe_materialize_plan_after_tool_call_format_error(
                self, err,
            )?
        {
            return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
        }
        // Issue #634: Format-error 経路の制御フロー不変条件 (SSOT)
        //   (1) 評価順序固定: `maybe_apply_*` → `maybe_finish_*` の順で呼ぶ
        //       (順序を変えると edit-then-finish の意味が崩れる)。
        //   (2) flag off で apply は no-op (`Ok(None)`)。loop は次の
        //       handler (`maybe_finish_*`) にフォールスルー。
        //   (3) `maybe_finish_*` は capability gate (`finish_after_edit_format_error`)
        //       のみで動く汎用 path (experimental flag 非依存)。
        //       qwen3.5 ユーザーの format-error 後 finish は flag off
        //       でも維持される。
        if recovery_dispatch_gate.allows_deterministic_fallback()
            && let Some(reply) =
                super::scaffold_pipeline::maybe_apply_deterministic_edit_after_format_error(
                    self, err,
                )?
        {
            return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
        }
        if recovery_dispatch_gate.allows_generic_repo_change_recovery()
            && let Some(reply) = self.maybe_finish_after_edit_format_error(err)
        {
            return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
        }
        if lifecycle::is_tool_call_format_error(err)
            && retry_state.tool_call_format_retries_remaining > 0
        {
            retry_state.tool_call_format_retry_count += 1;
            retry_state.tool_call_format_retries_remaining -= 1;
            self.push_tool_call_format_retry_note(err, retry_state.tool_call_format_retry_count);
            return Ok(Some(AssistantReplyRetryDecision::Retry));
        }
        Ok(None)
    }

    fn push_tool_call_format_retry_note(&mut self, err: &str, retry_count: usize) {
        let lower_err = err.to_ascii_lowercase();
        let effective_tool_policy = self.effective_tool_policy();
        if let Some(policy) = effective_tool_policy.focused_edit_policy() {
            let target = &policy.target;
            let target_already_read = policy.target_already_read;
            let target_display = progress_path_display(
                &target.display().to_string(),
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
                120,
            );
            if !target.is_file() {
                self.push_system_note(recovery::focused_edit_missing_target_recovery_note(
                    &target_display,
                    retry_count,
                ));
                return;
            }
            if lower_err.contains("truncated tool call") {
                self.push_system_note(recovery::focused_edit_truncated_tool_call_note(
                    &target_display,
                    target_already_read,
                    retry_count,
                ));
                return;
            }
            if lower_err.contains("unterminated <anvil_tool_call> block") {
                self.push_system_note(recovery::focused_edit_unterminated_tool_call_note(
                    &target_display,
                    target_already_read,
                    retry_count,
                ));
                return;
            }
        }
        self.push_system_note(recovery::tool_call_format_recovery_note(err, retry_count));
    }

    fn maybe_handle_assistant_reply_timeout_error(
        &mut self,
        err: &str,
        recovery_dispatch_gate: RecoveryDispatchGate,
        retry_state: &mut AssistantReplyRetryState,
    ) -> Option<AssistantReplyRetryDecision> {
        if !err.to_ascii_lowercase().contains("timed out") {
            return None;
        }
        if recovery_dispatch_gate.allows_deterministic_fallback()
            && let Some(reply) =
                super::scaffold_pipeline::maybe_apply_deterministic_polish_fallback_after_timeout(
                    self, err,
                )
        {
            self.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&self.work_root),
            );
            return Some(AssistantReplyRetryDecision::ReturnReply(reply));
        }
        let timeout_focused_policy = self.effective_tool_policy().focused_edit_policy().cloned();
        if let Some(policy) = timeout_focused_policy {
            if recovery_dispatch_gate.allows_deterministic_fallback()
                && let Some(reply) =
                    super::scaffold_pipeline::maybe_apply_deterministic_quality_fallback_after_timeout(self, err)
            {
                self.session.record_feedback_if_unset(
                    build_feedback_for_deterministic_content_fallback(&self.work_root),
                );
                return Some(AssistantReplyRetryDecision::ReturnReply(reply));
            }
            retry_state.focused_edit_timeout_retry_count += 1;
            if retry_state.focused_edit_timeout_retry_count >= 2 {
                return Some(AssistantReplyRetryDecision::Fail(err.to_string()));
            }
            self.push_system_note(recovery::focused_edit_timeout_recovery_note(
                &progress_path_display(
                    &policy.target.display().to_string(),
                    &self.work_root,
                    self.session.mode_state.active_plan_path.as_deref(),
                    120,
                ),
                policy.target_already_read,
                retry_state.focused_edit_timeout_retry_count,
            ));
            return Some(AssistantReplyRetryDecision::Retry);
        }
        None
    }

    fn maybe_handle_assistant_reply_transport_error(
        &mut self,
        err: &str,
        retry_state: &mut AssistantReplyRetryState,
    ) -> Result<Option<AssistantReplyRetryDecision>, String> {
        if !lifecycle::is_transport_error(err) || retry_state.extra_transport_retries == 0 {
            return Ok(None);
        }
        if let Some(reply) =
            super::scaffold_pipeline::maybe_materialize_plan_after_timeout(self, err)?
        {
            return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
        }
        if super::scaffold_pipeline::maybe_fallback_plan_model_after_timeout(self, err) {
            return Ok(Some(AssistantReplyRetryDecision::Retry));
        }
        retry_state.transport_retry_count += 1;
        retry_state.extra_transport_retries -= 1;
        thread::sleep(Duration::from_secs(
            (retry_state.transport_retry_count as u64) * 4,
        ));
        Ok(Some(AssistantReplyRetryDecision::Retry))
    }

    fn finish_assistant_reply_retry(
        &self,
        err: String,
        retry_state: &mut AssistantReplyRetryState,
    ) -> AssistantReplyRetryDecision {
        if retry_state.retries_remaining == 0 {
            return AssistantReplyRetryDecision::Fail(err);
        }
        let sleep_secs = (self.config.chat_retries - retry_state.retries_remaining + 1) as u64 * 2;
        retry_state.retries_remaining -= 1;
        thread::sleep(Duration::from_secs(sleep_secs));
        AssistantReplyRetryDecision::Retry
    }

    fn request_assistant_reply(
        &mut self,
        stream_output: bool,
        stop_signal: Option<SpinnerStopSignal>,
        interrupt_flag: &InterruptFlag,
    ) -> Result<AssistantReply, String> {
        let protocol =
            prompting::ToolProtocol::from_native_tools_enabled(self.native_tools_enabled);
        let native_tools_enabled = protocol.native_tools_enabled();
        let effective_tool_policy = self.effective_tool_policy();
        let focused_edit_target = effective_tool_policy
            .focused_edit_policy()
            .map(|policy| policy.target.as_path());
        let messages = self.build_request_messages(protocol, &effective_tool_policy);
        let assistant_model = self.current_assistant_model();
        let request_plan = build_assistant_request_plan(
            assistant_model.as_str(),
            native_tools_enabled,
            stream_output,
            io::stdin().is_terminal(),
            &self.session.messages,
            focused_edit_target,
            &self.work_root,
        );
        let tool_specs = self.tool_specs_for_policy(&effective_tool_policy);

        if request_plan.use_streaming_transport {
            self.request_streaming_assistant_reply(
                &messages,
                &tool_specs,
                native_tools_enabled,
                stream_output,
                stop_signal,
                interrupt_flag,
            )
        } else {
            request_non_streaming_assistant_reply(
                &self.client,
                assistant_model.as_str(),
                &messages,
                &tool_specs,
                native_tools_enabled,
                request_plan.focused_edit_timeout_override,
                request_plan.focused_edit_max_predict_override,
            )
        }
    }

    fn request_streaming_assistant_reply(
        &self,
        messages: &[ConversationMessage],
        tool_specs: &[ToolSpec],
        native_tools_enabled: bool,
        stream_output: bool,
        stop_signal: Option<SpinnerStopSignal>,
        interrupt_flag: &InterruptFlag,
    ) -> Result<AssistantReply, String> {
        let assistant_model = self.current_assistant_model();
        let mut render_state = super::streaming_reply::StreamingReplyRenderState::new();
        let reply = self.client.chat_streaming_with_mode(
            assistant_model.as_str(),
            messages,
            tool_specs,
            native_tools_enabled,
            |chunk| {
                super::streaming_reply::handle_streaming_assistant_chunk(
                    &mut render_state,
                    chunk,
                    stream_output,
                    stop_signal.as_ref(),
                    interrupt_flag,
                )
            },
        )?;
        super::streaming_reply::finish_streaming_assistant_reply(&mut render_state, stream_output);
        Ok(reply)
    }

    pub(super) fn current_assistant_model(&self) -> String {
        assistant_model_for_mode(
            self.session.mode_state.mode,
            &self.models.main,
            self.plan_model_override.as_deref(),
        )
    }

    /// Issue #634: 旧名 `maybe_finish_after_qwen35_edit_format_error`。
    /// 「format error でも edit success なら finish」というモデル非依存の汎用
    /// 挙動を担う。`finish_after_edit_format_error` capability のみで gate される
    /// (experimental flag 非依存)。
    fn maybe_finish_after_edit_format_error(&self, err: &str) -> Option<AssistantReply> {
        if !lifecycle::is_tool_call_format_error(err)
            || !model_capabilities(&self.current_assistant_model()).finish_after_edit_format_error
            || self.session.mode_state.mode != ExecutionMode::Act
        {
            return None;
        }
        if self.active_python_request_requires_tests() && !self.python_test_artifact_exists() {
            return None;
        }
        let edits = successful_non_plan_repo_edit_count(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        );
        (edits > 0).then(|| AssistantReply {
            content: "Applied the focused edit; stopping after a malformed follow-up tool call."
                .to_string(),
            tool_calls: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
        })
    }

    fn build_request_messages(
        &mut self,
        protocol: prompting::ToolProtocol,
        effective_tool_policy: &EffectiveToolPolicy,
    ) -> Vec<ConversationMessage> {
        let mut messages = Vec::new();
        let focused_edit_policy = effective_tool_policy.focused_edit_policy().cloned();
        let focused_edit_target = focused_edit_policy
            .as_ref()
            .map(|policy| policy.target.clone());
        let successful_repo_edits = successful_non_plan_repo_edit_count(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        );
        let focused_edit_target_already_read = focused_edit_policy
            .as_ref()
            .is_some_and(|policy| policy.target_already_read);
        let plan_contents = if self.session.mode_state.mode == ExecutionMode::Plan {
            self.current_plan_contents().ok().flatten()
        } else {
            None
        };
        let plan_stage = plan_contents
            .as_deref()
            .map(lifecycle::current_plan_stage)
            .or_else(|| {
                (self.session.mode_state.mode == ExecutionMode::Plan)
                    .then_some(self.session.mode_state.plan_stage)
            });
        let next_sections = plan_contents
            .as_deref()
            .map(lifecycle::plan_next_stage_sections)
            .unwrap_or_default();

        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
            self.session.mode_state.task_profile,
            protocol,
            plan_stage,
            &next_sections,
            effective_tool_policy.allowed_tool_names_for_prompt(),
        )));
        if let Some(message) = self.mode_policy_message() {
            messages.push(message);
        }
        if focused_edit_target.is_none() {
            self.append_general_request_context_messages(&mut messages);
        }
        self.append_common_request_messages(&mut messages, protocol, effective_tool_policy);
        if let Some(target) = focused_edit_target {
            self.append_focused_edit_request_messages(
                &mut messages,
                effective_tool_policy,
                &target,
                focused_edit_target_already_read,
                successful_repo_edits,
            );
        } else {
            messages.extend(self.session.messages.clone());
        }
        messages
    }

    fn append_general_request_context_messages(&mut self, messages: &mut Vec<ConversationMessage>) {
        if self.active_task_expects_repo_change() && self.workspace_appears_empty() {
            if let Some(framework) =
                super::scaffold_pipeline::active_task_requested_scaffold_framework(self)
            {
                messages.push(ConversationMessage::system(
                    recovery::framework_scaffold_now_note(framework.label()),
                ));
            }
            messages.push(ConversationMessage::system(
                recovery::empty_workspace_scaffold_note(),
            ));
        }
        if let Some(memory_message) = self.working_memory_message() {
            messages.push(memory_message);
        }
        let case_injection = self.try_inject_case_retrieval_message();
        if let Some(ref inj) = case_injection {
            messages.push(inj.message.clone());
        }
        let anti_injection = self.try_inject_anti_pattern_message();
        if let Some(ref inj) = anti_injection {
            messages.push(inj.message.clone());
        }
        self.maybe_send_request_context_pack(&case_injection, &anti_injection);
        if let Some(repo_context_message) = self.repo_context_message() {
            messages.push(repo_context_message);
        }
    }

    fn maybe_send_request_context_pack(
        &mut self,
        case_injection: &Option<RetrievalInjection>,
        anti_injection: &Option<RetrievalInjection>,
    ) {
        if self.session.context_pack_sent_this_turn {
            return;
        }
        let selected_case_ids: Vec<String> = case_injection
            .as_ref()
            .map(|inj| inj.selected_ids.clone())
            .unwrap_or_default();
        let selected_anti_ids: Vec<String> = anti_injection
            .as_ref()
            .map(|inj| inj.selected_ids.clone())
            .unwrap_or_default();
        let selected_precaution_ids: Vec<String> = self
            .session
            .working_memory
            .active_precautions
            .iter()
            .filter(|p| p.status == crate::session::precaution::PrecautionStatus::Active)
            .map(|p| p.id.clone())
            .collect();
        let recent_tool_summary = build_recent_tool_summary(&self.session.messages);
        let gate = crate::photon::mapper::PhotonGateInputs {
            photon_present: self.photon.is_some(),
            shadow_mode: self.config.photon_shadow_mode,
            canary: self.config.photon_canary,
            session_id: self.session_store.session_id(),
            turn_idx: self.current_turn_index,
        };
        if !crate::photon::mapper::should_send_context_pack(&gate) {
            return;
        }
        let resp_opt = match &self.photon {
            Some(photon) => {
                let working_memory_text = self.session.working_memory.format_for_prompt();
                let inputs = crate::photon::mapper::ContextPackInputs {
                    task: self.session.working_memory.active_task.as_deref(),
                    repo_path: &self.work_root,
                    branch: None,
                    commit: None,
                    working_memory_text: working_memory_text.as_deref(),
                    touched_files: &self.session.working_memory.touched_files,
                    recent_tool_summary: &recent_tool_summary,
                    selected_case_ids: &selected_case_ids,
                    selected_anti_pattern_ids: &selected_anti_ids,
                    selected_precaution_ids: &selected_precaution_ids,
                };
                let req = crate::photon::mapper::build_context_pack_request(&inputs);
                let rid = req.0["request_id"].as_str().map(|s| s.to_string());
                let resp = photon.context_pack(&req);
                if self.last_context_pack_id.is_none() {
                    self.last_context_pack_id = rid;
                }
                resp
            }
            None => None,
        };
        if let Some(resp) = resp_opt.as_ref() {
            let blocked_ids: std::collections::HashSet<String> =
                if self.config.photon_respect_warnings {
                    let (ids, _stats) = crate::photon::prompt::extract_blocked_summary_ids(resp);
                    ids
                } else {
                    std::collections::HashSet::new()
                };
            let shadow_input = self.config.photon_shadow_mode;
            let _ = self.record_pam_advisory_decision(resp, &blocked_ids, shadow_input);
        }
        self.session.context_pack_sent_this_turn = true;
    }

    fn append_common_request_messages(
        &mut self,
        messages: &mut Vec<ConversationMessage>,
        protocol: prompting::ToolProtocol,
        effective_tool_policy: &EffectiveToolPolicy,
    ) {
        if let Some(ctx) =
            super::photon_feedback_derive::photon_context_pack_injection_message(self)
        {
            messages.push(ctx);
        }
        if self.config.offline {
            messages.push(ConversationMessage::system(
                "[Runtime Policy] Offline mode is enabled. Do not use network access, package installs, or general-purpose shell commands. If shell is necessary, keep it read-only or build-test only."
                    .to_string(),
            ));
        }
        if self.session.mode_state.mode == ExecutionMode::Plan
            && let Some(plan_path) = self.session.mode_state.active_plan_path.as_deref()
        {
            messages.push(ConversationMessage::system(format!(
                "[Plan File Alias] The active plan file may live outside the project root, but it is still accessible. Treat these two paths as the same file: {} and {}. Do not loop on Read because of the outside-workspace path; continue updating the same active plan file.",
                plan_path.display(),
                plan_file_alias(plan_path)
            )));
        }
        if let Some(note) = self.forced_small_edit_recovery_message() {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = super::scaffold_pipeline::post_scaffold_edit_recovery_message(self) {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) =
            super::scaffold_pipeline::post_scaffold_continuation_recovery_message(self)
        {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = self.verifier_repair_policy_message(effective_tool_policy) {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = self.artifact_directed_policy_violation_message(effective_tool_policy) {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = self.artifact_directed_recovery_message(effective_tool_policy) {
            messages.push(ConversationMessage::system(note));
        }
        let current_request_paths = extract_current_request_paths(self, &self.work_root);
        let last_suspected = self
            .session
            .last_feedback
            .as_ref()
            .map(|f| f.suspected_files.as_slice());
        messages.extend(prompting::runtime_context_messages(
            &self.config.cwd,
            &self.work_root,
            protocol,
            &self.session.working_memory.touched_files,
            last_suspected,
            &current_request_paths,
        ));
    }

    fn append_focused_edit_request_messages(
        &self,
        messages: &mut Vec<ConversationMessage>,
        effective_tool_policy: &EffectiveToolPolicy,
        target: &Path,
        focused_edit_target_already_read: bool,
        successful_repo_edits: usize,
    ) {
        let recovery_anchor = focused_edit_exact_recovery_anchor(
            &self.session.messages,
            target,
            &self.work_root,
            focused_edit_target_already_read,
            successful_repo_edits,
        );
        let compact_anchor = (recovery_anchor.is_none()
            && focused_edit_target_already_read
            && recent_truncated_tool_call_attempt(&self.session.messages) > 0)
            .then(|| {
                focused_edit_compact_recovery_anchor(
                    &self.session.messages,
                    target,
                    &self.work_root,
                )
            })
            .flatten();
        let exact_anchor = recovery_anchor.or_else(|| compact_anchor.clone());
        let target_display = target
            .strip_prefix(&self.work_root)
            .unwrap_or(target)
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(note) = focused_edit_policy_violation_feedback_note(
            &self.session.working_memory.unresolved_errors,
            effective_tool_policy.allowed_tool_names_for_prompt(),
            Some(&target_display),
        ) {
            messages.push(ConversationMessage::system(note));
        }
        messages.push(ConversationMessage::system(
            focused_edit_guidance_note_for_policy(
                effective_tool_policy,
                target,
                &self.work_root,
                focused_edit_target_already_read,
            ),
        ));
        if compact_anchor.is_some() {
            messages.push(ConversationMessage::system(
                focused_edit_compact_anchor_note(target, &self.work_root),
            ));
        }
        if successful_repo_edits == 0
            && let Some(note) = focused_edit_first_slice_note(
                &self.session.messages,
                target,
                &self.work_root,
                focused_edit_target_already_read,
            )
        {
            messages.push(ConversationMessage::system(note));
        }
        if successful_repo_edits == 1
            && let Some(note) = focused_edit_second_slice_note(
                &self.session.messages,
                target,
                &self.work_root,
                focused_edit_target_already_read,
            )
        {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(anchor) = exact_anchor {
            messages.extend(focused_edit_exact_anchor_history(
                &self.session.messages,
                target,
                &self.work_root,
                &anchor,
            ));
        } else {
            messages.extend(focused_edit_history(
                &self.session.messages,
                target,
                &self.work_root,
            ));
        }
    }

    /// Issue #664 test seam: `pub(super)` wrapper over the private
    /// `effective_tool_policy()` so `loop_run.rs`'s `#[cfg(test)]`
    /// `effective_tool_policy_for_test` seam can reach the production
    /// path without widening the `loop_run` module surface (DR3-001 /
    /// AD19). Returns the raw `EffectiveToolPolicy`; the seam in
    /// `loop_run.rs` projects it to `(Vec<String>, String)` primitives
    /// before crossing the `pub(crate)` boundary.
    #[cfg(test)]
    pub(super) fn effective_tool_policy_pub_for_test(&self) -> EffectiveToolPolicy {
        self.effective_tool_policy()
    }

    /// Issue #664 test seam: `pub(super)` wrapper over the private
    /// `build_arbiter_candidates()` so the `loop_run.rs` seam can read
    /// the candidate list without widening visibility. The returned
    /// `Vec<JobCandidate>` stays inside `loop_run`; `loop_run.rs::
    /// build_arbiter_candidates_for_test` projects each element into a
    /// 4-field primitive DTO before the `pub(crate)` boundary.
    #[cfg(test)]
    pub(super) fn build_arbiter_candidates_pub_for_test(
        &self,
    ) -> Vec<super::active_job_arbiter::JobCandidate> {
        self.build_arbiter_candidates()
    }

    /// Issue #664 iteration-3 (CB2-003) test seam: drive the
    /// `effective_tool_policy_error_for_call_with_scope` rejection +
    /// `record_artifact_completion_bash_violation` chokepoint under a
    /// caller-provided `EffectiveToolPolicy` and tool arguments. Mirrors
    /// the exact branching inside `execute_tool_call` without any
    /// network / cancellation / approval side effects.
    ///
    /// Returns `(error_string, bash_violation_recorded_count_delta)`.
    /// The count delta is observed by comparing the active job's
    /// `attempts().len()` before/after the call, which captures both
    /// the artifact-directed Bash branch (CB2-003) and the SetupBootstrap
    /// branch (CB-003 iteration-2). The error string is the same value
    /// `execute_tool_call` would surface to the LLM (sans
    /// `lifecycle::format_tool_error` cosmetic wrapping).
    ///
    /// Visibility: `pub(super)` only; `loop_run.rs` projects this
    /// through a primitive-tuple test seam.
    #[cfg(test)]
    pub(super) fn drive_policy_error_for_test(
        &mut self,
        policy: &EffectiveToolPolicy,
        name: &str,
        arguments: &serde_json::Value,
    ) -> (Option<String>, usize) {
        let before = self
            .artifact_completion_job
            .as_ref()
            .map(|j| j.attempts().len())
            .unwrap_or(0);
        let scope_for_policy = if self.missing_verifier_job.is_some() {
            Some(self.current_workspace_scope())
        } else {
            None
        };
        let err = effective_tool_policy_error_for_call_with_scope(
            policy,
            name,
            arguments,
            &self.work_root,
            scope_for_policy.as_ref(),
        );
        if let Some(err) = err.as_ref() {
            // Mirror the recording branch in `execute_tool_call` so the
            // test seam exercises the exact production wiring.
            if err.contains("artifact-directed recovery rejected") {
                if name == "Bash" {
                    let command_arg = arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let _ = self.record_artifact_completion_bash_violation(vec![command_arg]);
                } else {
                    let actual_path = arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let _ = self.record_artifact_completion_attempt(
                        super::artifact_completion_job::ArtifactAttemptOutcomeKind::WrongTarget,
                        vec![format!("{name} on {actual_path}")],
                    );
                }
            } else if err.starts_with("setup bootstrap")
                && name == "Bash"
                && self.artifact_completion_job.is_some()
            {
                let command_arg = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ = self.record_artifact_completion_bash_violation(vec![command_arg]);
            }
        }
        let after = self
            .artifact_completion_job
            .as_ref()
            .map(|j| j.attempts().len())
            .unwrap_or(0);
        (err, after.saturating_sub(before))
    }

    /// Issue #664 iteration-3 (CB2-003) test seam: build an
    /// `artifact_directed_from_job` policy from the active
    /// `ArtifactCompletionJob`. Returns `None` when no job is installed.
    #[cfg(test)]
    pub(super) fn artifact_directed_policy_for_test(&self) -> Option<EffectiveToolPolicy> {
        let job = self.artifact_completion_job.as_ref()?;
        let target = self.artifact_recovery_target_path()?;
        let target_already_read =
            focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
        Some(EffectiveToolPolicy::artifact_directed_from_job(
            target,
            target_already_read,
            job.allowed_write_actions(),
            job.allowed_read_scope(),
        ))
    }

    /// Issue #664 iteration-3 (CB2-003) test seam: inspect the latest
    /// recorded attempt's `bash_policy_violation` marker. `None` when no
    /// job / no attempts. Mirrors the projection-time `category` field
    /// without exposing `ArtifactAttemptOutcome`.
    #[cfg(test)]
    pub(super) fn last_attempt_bash_policy_violation_for_test(&self) -> Option<bool> {
        let job = self.artifact_completion_job.as_ref()?;
        let attempts = job.attempts();
        attempts.last().map(|a| a.bash_policy_violation())
    }

    /// Issue #664 iteration-3 (CB2-003) test seam: count the number of
    /// recorded attempts on the active job. `None` when no job.
    #[cfg(test)]
    pub(super) fn artifact_completion_job_attempts_len_for_test(&self) -> Option<usize> {
        self.artifact_completion_job
            .as_ref()
            .map(|j| j.attempts().len())
    }

    pub(super) fn effective_tool_policy(&self) -> EffectiveToolPolicy {
        // Issue #660: `AnswerOnlyMode` is a pre-arbitration gate (priority 0
        // in §4 of the design policy). The arbiter never sees it; we early-
        // return before constructing any selectable `JobCandidate`.
        if self.answer_only_mode_active() {
            if self.workspace_appears_empty() {
                return EffectiveToolPolicy::restricted(
                    EffectiveToolPolicyReason::AnswerOnly,
                    Vec::new(),
                );
            }
            if self.script_execution_requested() {
                return EffectiveToolPolicy::restricted(
                    EffectiveToolPolicyReason::AnswerOnly,
                    vec!["Read", "Glob", "Grep", "Bash"],
                );
            } else {
                return EffectiveToolPolicy::restricted(
                    EffectiveToolPolicyReason::AnswerOnly,
                    vec!["Read", "Glob", "Grep"],
                );
            }
        }

        // Issue #660 (Codex CB-001 / §4 design table): `PlanModeGate` is also
        // a pre-arbitration gate, not a selectable arbitration kind. The
        // Plan-file Write/Edit exception is the authority of
        // `src/tools/registry.rs::resolve_plan_mode_write_target` /
        // `enforce_plan_stage_scope`; the arbiter must not pre-empt that
        // decision with stale `task_contract_verifier_repair_pending` or
        // other selectable state. Returning `unrestricted()` keeps the
        // tool-spec surface and recovery-target gating out of the arbiter
        // while the registry-layer PAM gate enforces actual write target.
        if self.session.mode_state.mode == ExecutionMode::Plan {
            return EffectiveToolPolicy::unrestricted();
        }

        // Issue #660 (Phase E): arbiter is the **sole authority** for write
        // owner selection. The legacy if-elif chain, the
        // `#[cfg(debug_assertions)]` dual-source assertion, and the
        // `agent.active_job.divergence_detected` event emit that lived here
        // during Phase A+B-D have all been removed. `effective_tool_policy`
        // is now a thin shell over `build_arbiter_candidates` +
        // `select_active_job` + `project_policy`.
        //
        // The `agent.active_job.selected` event is emitted by
        // `emit_active_job_selected_if_changed` from the agent loop driver
        // (see L5352 `run_actor_loop` site) — `effective_tool_policy` stays
        // a pure read so it can be called freely without log-emit side
        // effects.
        let candidates = self.build_arbiter_candidates();
        let selection = super::active_job_arbiter::select_active_job(&candidates);
        super::active_job_arbiter::project_policy(&selection)
    }

    /// Issue #660: build the arbiter candidate list from the same source
    /// signals the legacy chain reads. Per DR1-004 only candidates with a
    /// determined `desired_action` are pushed — there is no
    /// `RejectionReason::NoDesiredAction`.
    ///
    /// Each branch mirrors a single legacy `if let Some(target) = ...`
    /// arm. The policy attached to the candidate is the exact value the
    /// legacy chain would have returned, so `project_policy(selection)`
    /// is value-equal to the legacy result.
    fn build_arbiter_candidates(&self) -> Vec<super::active_job_arbiter::JobCandidate> {
        use super::active_job_arbiter::{ActiveJobKind, Budget, DesiredAction, JobCandidate};

        if let Some(candidates) = self.priority_one_arbiter_candidates() {
            return candidates;
        }

        let mut candidates: Vec<JobCandidate> = Vec::new();

        // Priority 2: ForcedSmallEditRecovery.
        if let Some(target) = self.forced_small_edit_recovery_target() {
            self.push_focused_edit_candidate(
                &mut candidates,
                target,
                ActiveJobKind::ForcedSmallEditRecovery,
                EffectiveToolPolicyReason::FocusedEditRecovery,
            );
        }

        // Priority 3: ArtifactRecovery.
        // Issue #663 (Phase C / AD5 / DR1-006 / CB-001 fix): ArtifactRecovery
        // candidate is generated ONLY when an `ArtifactCompletionJob` is
        // installed. The job is the single source of truth for
        // `AllowedWriteActions` / `AllowedReadScope` / role-specific budget,
        // and the policy is always built via `artifact_directed_from_job`.
        // The previous fallback to the generic write-capable
        // `artifact_directed` policy (when target was set but job was None)
        // bypassed target validation and role-specific budget, so it is
        // removed: a bare `current_artifact_recovery_target` without a job
        // produces no write-capable candidate.
        if let (Some(target), Some(job)) = (
            self.artifact_recovery_target_path(),
            self.artifact_completion_job.as_ref(),
        ) {
            let target_already_read =
                focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
            let policy = EffectiveToolPolicy::artifact_directed_from_job(
                target.clone(),
                target_already_read,
                job.allowed_write_actions(),
                job.allowed_read_scope(),
            );
            let write_actions = job.allowed_write_actions().clone();
            let read_scope = job.allowed_read_scope().clone();
            candidates.push(JobCandidate {
                kind: ActiveJobKind::ArtifactRecovery,
                desired_action: DesiredAction::ArtifactDirected {
                    target,
                    already_read: target_already_read,
                    write_actions,
                    read_scope,
                },
                policy,
                budget: Budget::Unbounded,
            });
        }

        // Issue #664 (Priority 4 / AD22): SetupBootstrap candidate. Built
        // pure from `TaskContract` + behavior projection + verifier
        // prerequisite signal + ledger overflow. `should_install_setup_bootstrap`
        // is the SSOT decision tree (pure-fn; arbiter does not observe
        // Agent state).
        //
        // **Issue #664 iteration-2 (CB-001) — Stage A + Stage B both wired**:
        //
        // - **Stage A live observation**: `OwnedTestVerifierPlan::Missing`
        //   is observed in `run_task_contract_verifier_once` and recorded
        //   into the per-turn flag `owned_test_verifier_missing_observed_this_turn`
        //   (single producer). Reading the flag from `&self` is O(1) so the
        //   pure-`&self` candidate builder can consume the live signal at
        //   every `effective_tool_policy()` evaluation without traversing
        //   the workspace. Reset at `handle_user_message` head (per-turn
        //   rule).
        //
        // - **Stage B (BehaviorContractProjection verifier capability label)**:
        //   the behavior projection is passed to
        //   `VerifierPrerequisiteSignal::from_sources(stage_a, Some(&p))`.
        //   `should_install_setup_bootstrap` refines the OR-composed
        //   signal so Stage B alone (label-only) is insufficient — it
        //   only fires via the Setup-label fallback (step 4) when the
        //   projection also carries an explicit setup keyword. This
        //   suppresses the false positive on plain "add tests" requests
        //   where `verification_expectations = ["test"]` would otherwise
        //   trip step (2). Stage A live alone always passes step (2);
        //   `required_artifacts::Setup` is unaffected (step 1, no gate).
        if let Some(candidate) = self.setup_bootstrap_candidate() {
            candidates.push(candidate);
        }

        // Priority 5: FocusedEditRecovery.
        if let Some(target) = self.focused_edit_recovery_target() {
            self.push_focused_edit_candidate(
                &mut candidates,
                target,
                ActiveJobKind::FocusedEditRecovery,
                EffectiveToolPolicyReason::FocusedEditRecovery,
            );
        }

        // Priority 6: LocalLlmSmallEditAfterRead.
        if let Some(target) = self.local_llm_small_edit_target() {
            self.push_focused_edit_candidate(
                &mut candidates,
                target,
                ActiveJobKind::LocalLlmSmallEditAfterRead,
                EffectiveToolPolicyReason::LocalLlmSmallEditAfterRead,
            );
        }

        candidates
    }

    fn priority_one_arbiter_candidates(
        &self,
    ) -> Option<Vec<super::active_job_arbiter::JobCandidate>> {
        use super::active_job_arbiter::{ActiveJobKind, Budget, DesiredAction, JobCandidate};

        match determine_loop_control_action(LoopControlInputs {
            mode: self.session.mode_state.mode,
            task_contract_verifier_repair_pending: self.task_contract_verifier_repair_pending,
            repair_next_action: self.repair_job.as_ref().map(|job| job.next_action()),
            missing_verifier_next_action: self
                .missing_verifier_job
                .as_ref()
                .map(|job| job.next_action()),
            task_contract_action: None,
        }) {
            LoopControlAction::ContinueRepairJob { next_action } => {
                let target_hint = match &next_action {
                    super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
                        Some(target_hint.clone())
                    }
                    _ => None,
                };
                Some(vec![JobCandidate {
                    kind: ActiveJobKind::VerifierRepair,
                    desired_action: DesiredAction::VerifierRepair {
                        command: String::new(),
                        target_hint,
                    },
                    policy: self.verifier_repair_policy_for_next_action(&next_action),
                    budget: Budget::Unbounded,
                }])
            }
            LoopControlAction::ContinueMissingVerifierJob { next_action } => {
                if matches!(
                    next_action,
                    super::repair_job::VerifierBootstrapNextAction::RequestSetupEdit
                ) && let Some(job) = self.missing_verifier_job.as_ref()
                {
                    return Some(vec![JobCandidate {
                        kind: ActiveJobKind::VerifierRepair,
                        desired_action: DesiredAction::MissingVerifierCreate,
                        policy: EffectiveToolPolicy::restricted(
                            EffectiveToolPolicyReason::VerifierRepair,
                            job.allowed_tool_names().to_vec(),
                        ),
                        budget: Budget::Unbounded,
                    }]);
                }
                Some(Vec::new())
            }
            LoopControlAction::RunVerifier | LoopControlAction::RequestModelTurn => None,
        }
    }

    fn push_focused_edit_candidate(
        &self,
        candidates: &mut Vec<super::active_job_arbiter::JobCandidate>,
        target: PathBuf,
        kind: super::active_job_arbiter::ActiveJobKind,
        reason: EffectiveToolPolicyReason,
    ) {
        use super::active_job_arbiter::{Budget, DesiredAction, JobCandidate};

        let already_read =
            focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
        candidates.push(JobCandidate {
            kind,
            desired_action: DesiredAction::FocusedEdit {
                target: target.clone(),
                already_read,
            },
            policy: self.focused_edit_policy_for_target(target, reason),
            budget: Budget::Unbounded,
        });
    }

    fn setup_bootstrap_candidate(&self) -> Option<super::active_job_arbiter::JobCandidate> {
        use super::active_job_arbiter::{ActiveJobKind, Budget, DesiredAction, JobCandidate};

        if matches!(
            self.session.mode_state.work_mode,
            WorkMode::Docs | WorkMode::AnswerOnly
        ) {
            return None;
        }
        let request = self.active_request_text()?;
        let task_contract = super::task_contract::TaskContract::from_request(&request);
        let behavior_projection =
            super::required_behavior::project_behavior_contract(&task_contract);
        let verifier_signal = super::task_contract::VerifierPrerequisiteSignal::from_sources(
            self.owned_test_verifier_missing_observed_this_turn,
            behavior_projection.as_ref(),
        );
        if !super::active_job_arbiter::should_install_setup_bootstrap(
            &task_contract,
            behavior_projection.as_ref(),
            &verifier_signal,
            self.artifact_ledger.overflowed(),
        ) {
            return None;
        }
        Some(JobCandidate {
            kind: ActiveJobKind::SetupBootstrap,
            desired_action: DesiredAction::SetupBash,
            policy: EffectiveToolPolicy::setup_bootstrap(),
            budget: Budget::Unbounded,
        })
    }

    /// Issue #660 (Phase C / DD-4): per-turn diff-based emit of
    /// `agent.active_job.selected`. Computes the current
    /// `ActiveJobSelection`, compares it with the previous emission stored
    /// in `self.last_active_job_selection`, and emits a single structured
    /// log event when the selection differs. Returns `true` when the event
    /// was emitted, `false` when dedup skipped it.
    ///
    /// **Per-turn rule** (DR1-007): `self.last_active_job_selection` is
    /// reset to `None` at the head of every `handle_user_message`, so the
    /// first call of a new turn always emits.
    ///
    /// **Security** (Stage 4 DR4-001/002 / §7 of the design policy): the
    /// payload contains only short type labels, sanitized
    /// `EffectiveToolPolicyReason::as_str()` strings, counts, and
    /// non-cryptographic `stable_path_hash(mask_secrets(...))` correlators.
    /// Raw verifier commands / raw paths / raw recovery reasons are NEVER
    /// included; emit goes through `log_llm_event` so
    /// `mask_payload_inplace` is the final defense line.
    ///
    /// **`iteration_seq` semantics** (Codex CB-002): the caller passes the
    /// actor-loop iteration counter (`run_actor_loop`'s `iter_count`) so
    /// that the payload `iteration_seq` field name and value semantics
    /// agree. Two re-emits within the same turn carry distinct
    /// `iteration_seq` values, which lets #666 consumers identify the
    /// iteration at which a selection change occurred.
    pub(super) fn emit_active_job_selected_if_changed(&mut self, iteration_seq: u32) -> bool {
        let selection = self.current_active_job_selection();
        if self.last_active_job_selection.as_ref() == Some(&selection) {
            return false;
        }
        let payload = build_active_job_selected_payload(
            &selection,
            iteration_seq,
            self.repair_job_artifact_attempts as u32,
            self.artifact_completion_job
                .as_ref()
                .map(|job| job.attempts().len() as u32)
                .unwrap_or(0),
        );
        log_llm_event("agent.active_job.selected", payload);
        self.last_active_job_selection = Some(selection);
        true
    }

    /// Issue #665 (Phase 6 / S5-006 / S7-002): emit the
    /// `agent.behavior_contract.projected` event with per-turn diff-based
    /// dedup. Only emits when:
    /// - `projection` is `Some(...)` (i.e. consumed by a prompt site), AND
    /// - the payload-shaped key differs from
    ///   `self.last_behavior_contract_projection_event`.
    ///
    /// **Per-turn rule** (DR1-007): `last_behavior_contract_projection_event`
    /// is reset to `None` at the head of every `handle_user_message`, so the
    /// first consumed projection in a new turn always emits.
    ///
    /// **Security**: payload contains only `schema_version`, `session_id`,
    /// `turn_index`, `consumer`, `confidence`, `fields_used`. Raw `label` /
    /// `excerpt` are NEVER included (S5-006). `log_llm_event` →
    /// `mask_payload_inplace` is the final defense.
    ///
    /// Returns `true` when the event was emitted, `false` when dedup skipped
    /// it or no projection was supplied.
    pub(super) fn emit_behavior_contract_projected_if_changed(
        &mut self,
        projection: Option<&super::required_behavior::BehaviorContractProjection>,
        consumer: &'static str,
    ) -> bool {
        let Some(proj) = projection else {
            return false;
        };
        let key =
            super::required_behavior::BehaviorProjectionEventKey::from_projection(proj, consumer);
        if self.last_behavior_contract_projection_event.as_ref() == Some(&key) {
            return false;
        }
        let session_id = self.session_store.session_id().to_string();
        let turn_index = self.current_turn_index as u64;
        let payload = super::required_behavior::behavior_contract_projected_payload(
            &key,
            proj.confidence,
            &session_id,
            turn_index,
        );
        log_llm_event("agent.behavior_contract.projected", payload);
        self.last_behavior_contract_projection_event = Some(key);
        true
    }

    /// Issue #660 (Phase C): compute the current `ActiveJobSelection`
    /// using the same `build_arbiter_candidates` + `select_active_job`
    /// pipeline as `effective_tool_policy()`. Pure on `self` — no log
    /// emit, no state mutation. Recomputed on demand so callers
    /// (`emit_active_job_selected_if_changed`) can observe the selection
    /// independently of `effective_tool_policy()`.
    ///
    /// Issue #660 (Codex CB-001 / §4): mirrors the `effective_tool_policy`
    /// pre-arbitration gate for `ExecutionMode::Plan`. The PAM gate at
    /// `src/tools/registry.rs::resolve_plan_mode_write_target` /
    /// `enforce_plan_stage_scope` is the authority for plan-file Write/Edit
    /// arbitration; the arbiter does not see any candidate while Plan mode
    /// is active, so observers (e.g. `emit_active_job_selected_if_changed`,
    /// the `selected_skips_*` generic-retry guards in Phase D) see a
    /// `None` selection that accurately reflects the design.
    pub(super) fn current_active_job_selection(
        &self,
    ) -> super::active_job_arbiter::ActiveJobSelection {
        if self.session.mode_state.mode == ExecutionMode::Plan {
            return super::active_job_arbiter::ActiveJobSelection {
                selected: None,
                rejected: Vec::new(),
            };
        }
        let candidates = self.build_arbiter_candidates();
        super::active_job_arbiter::select_active_job(&candidates)
    }

    fn verifier_repair_policy_for_next_action(
        &self,
        action: &super::repair_job::RepairNextAction,
    ) -> EffectiveToolPolicy {
        match action {
            super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
                verifier_repair_policy_for_target_hint(
                    target_hint,
                    &self.session.messages,
                    &self.work_root,
                )
            }
            super::repair_job::RepairNextAction::RequestDiagnostic
            | super::repair_job::RepairNextAction::Replan
            | super::repair_job::RepairNextAction::RerunVerifier
            | super::repair_job::RepairNextAction::SafeStop { .. }
            | super::repair_job::RepairNextAction::VerifiedDone => EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::VerifierRepair,
                Vec::new(),
            ),
        }
    }

    fn focused_edit_policy_for_target(
        &self,
        target: PathBuf,
        reason: EffectiveToolPolicyReason,
    ) -> EffectiveToolPolicy {
        let target_already_read =
            focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
        if !target.is_file() {
            EffectiveToolPolicy::focused_edit(reason, vec!["Write"], target, target_already_read)
        } else if target_already_read {
            EffectiveToolPolicy::focused_edit(reason, vec!["Edit"], target, target_already_read)
        } else {
            EffectiveToolPolicy::focused_edit(
                reason,
                vec!["Read", "Edit"],
                target,
                target_already_read,
            )
        }
    }

    fn tool_specs_for_policy(&self, policy: &EffectiveToolPolicy) -> Vec<ToolSpec> {
        let mut specs = self.tool_registry.specs().to_vec();
        if let Some(allowed_tools) = policy.allowed_tool_names_for_prompt() {
            specs.retain(|spec| allowed_tools.contains(&spec.function.name.as_str()));
        }
        specs
    }

    fn local_llm_small_edit_target(&self) -> Option<PathBuf> {
        if !model_capabilities(&self.current_assistant_model()).read_after_small_edit_protocol {
            return None;
        }
        if self.session.mode_state.mode != ExecutionMode::Act
            || !self.session.mode_state.policy().repo_edit_required
            || !self.active_task_expects_repo_change()
        {
            return None;
        }
        if has_successful_non_plan_repo_edit(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) {
            return None;
        }
        if let Some(target) = self.artifact_recovery_target_path() {
            return focused_edit_target_already_read(
                &self.session.messages,
                &target,
                &self.work_root,
            )
            .then_some(target);
        }
        let target =
            latest_turn_preferred_read_edit_target(&self.session.messages, &self.work_root)?;
        focused_edit_target_already_read(&self.session.messages, &target, &self.work_root)
            .then_some(target)
    }

    fn mode_policy_message(&self) -> Option<ConversationMessage> {
        let work_mode = self.session.mode_state.work_mode;
        let text = match work_mode {
            WorkMode::Auto => return None,
            WorkMode::TypeScriptUi => {
                "[Mode Policy] Work mode is TypeScript UI. Prefer the existing JavaScript or TypeScript framework when present. Do not switch to Python or documentation-only output unless the user asks."
            }
            WorkMode::Python => {
                "[Mode Policy] Work mode is Python. Use Python-oriented files and verification. Do not create TypeScript, React, Next.js, Nuxt, or browser UI scaffolds unless the user asks."
            }
            WorkMode::Docs => {
                "[Mode Policy] Work mode is documentation. Edit or create documentation files only unless code changes are explicitly requested."
            }
            WorkMode::AnswerOnly => {
                "[Mode Policy] Work mode is answer-only/read-only. You may inspect files if needed, and may run an explicitly requested local script or read-only command, but do not require or perform repository edits."
            }
            WorkMode::GenericCode | WorkMode::Unknown => {
                "[Mode Policy] Work mode is generic code. Follow the repository stack and avoid TypeScript UI deterministic fallback unless the request explicitly asks for a browser UI."
            }
        };
        Some(ConversationMessage::system(text.to_string()))
    }

    fn forced_small_edit_recovery_message(&self) -> Option<String> {
        let path = self.forced_small_edit_recovery_target()?;
        let attempt = recent_truncated_tool_call_attempt(&self.session.messages).max(1);
        Some(recovery::forced_small_edit_recovery_note(
            &progress_path_display(
                &path.display().to_string(),
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
                120,
            ),
            attempt,
        ))
    }

    pub(super) fn forced_small_edit_recovery_target(&self) -> Option<PathBuf> {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return None;
        }
        if recent_truncated_tool_call_attempt(&self.session.messages) == 0 {
            return None;
        }
        if has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) {
            return None;
        }
        latest_turn_preferred_read_edit_target(&self.session.messages, &self.work_root).or_else(
            || {
                let path = last_read_tool_path(&self.session.messages)?;
                let candidate = resolve_user_path(&self.work_root, &path).ok()?;
                candidate.is_file().then_some(candidate)
            },
        )
    }

    pub(super) fn focused_edit_recovery_target(&self) -> Option<PathBuf> {
        self.forced_small_edit_recovery_target()
            .or_else(|| super::scaffold_pipeline::post_scaffold_edit_recovery_target(self))
            .or_else(|| super::scaffold_pipeline::post_scaffold_continuation_recovery_target(self))
    }

    fn repo_change_no_edit_recovery_target(&self) -> Option<PathBuf> {
        if self.session.mode_state.mode != ExecutionMode::Act
            || !self.session.mode_state.policy().repo_edit_required
            || !self.active_task_expects_repo_change()
            || has_successful_non_plan_repo_edit(
                &self.session.messages,
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
            )
        {
            return None;
        }
        if let Some(candidate) = self.artifact_recovery_target_path() {
            return Some(candidate);
        }
        if let Some(candidate) = first_existing_impl_target(&self.work_root)
            && focused_edit_target_already_read(&self.session.messages, &candidate, &self.work_root)
        {
            return Some(candidate);
        }
        latest_turn_preferred_read_edit_target(&self.session.messages, &self.work_root).or_else(
            || {
                let path = last_read_tool_path(&self.session.messages)?;
                let candidate = resolve_user_path(&self.work_root, &path).ok()?;
                candidate.is_file().then_some(candidate)
            },
        )
    }

    pub(super) fn push_repo_change_no_edit_recovery_note(&mut self, attempt: usize) -> bool {
        let Some(target) = self.repo_change_no_edit_recovery_target() else {
            return false;
        };
        let target_display = progress_path_display(
            &target.display().to_string(),
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
            120,
        );
        let note = if !target.is_file() {
            recovery::focused_edit_missing_target_recovery_note(&target_display, attempt)
        } else {
            recovery::repo_change_after_read_no_edit_note(&target_display, attempt)
        };
        self.push_system_note(note);
        true
    }

    pub(super) fn push_verifier_repair_recovery_note(&mut self, attempt: usize) -> bool {
        let Some(context) = self.repair_job.as_ref() else {
            return false;
        };
        match context.next_action() {
            super::repair_job::RepairNextAction::RequestDiagnostic
            | super::repair_job::RepairNextAction::Replan => {
                // Issue #665 Phase 5: caller-side projection (S5-005 では
                // raw label/excerpt は system note に出さないため helper
                // 内部で metadata のみに縮退する)。
                let active_request = self.active_request_text().unwrap_or_default();
                let task_contract =
                    super::task_contract::TaskContract::from_request(&active_request);
                let behavior_projection =
                    super::required_behavior::project_behavior_contract(&task_contract);
                let note =
                    verifier_repair_diagnostic_pending_note(context, behavior_projection.as_ref());
                self.push_system_note(note);
                true
            }
            super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
                let Some(relative) =
                    super::repair_job::safe_relative_path_string(&target_hint.path)
                else {
                    return false;
                };
                let target = self.work_root.join(relative);
                let target = std::fs::canonicalize(&target).unwrap_or(target);
                if !target.is_file() {
                    let target_display = verifier_repair_target_display(&target, &self.work_root);
                    self.push_system_note(recovery::focused_edit_missing_target_recovery_note(
                        &target_display,
                        attempt,
                    ));
                    return true;
                }
                self.push_system_note(task_contract_verifier_targeted_edit_required_note(
                    context,
                    &self.work_root,
                    focused_edit_target_already_read(
                        &self.session.messages,
                        &target,
                        &self.work_root,
                    ),
                    attempt,
                    TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT,
                ));
                true
            }
            super::repair_job::RepairNextAction::RerunVerifier
            | super::repair_job::RepairNextAction::SafeStop { .. }
            | super::repair_job::RepairNextAction::VerifiedDone => false,
        }
    }

    pub(super) fn prepare_verifier_diagnostic_pass(
        &mut self,
    ) -> Result<PreparedVerifierDiagnosticPass, VerifierDiagnosticPassOutcome> {
        self.reset_verifier_diagnostic_state_if_needed();
        let Some(context) = self.repair_job.clone() else {
            return Err(VerifierDiagnosticPassOutcome::Skipped);
        };
        if context.diagnostic_unavailable || context.assessment.is_some() {
            return Err(VerifierDiagnosticPassOutcome::Skipped);
        }
        let Some(attempt_spec) = verifier_diagnostic_attempt_spec(
            &self.models.main,
            self.models.sidecar.as_deref(),
            context.assessment_attempts,
        ) else {
            let error = context
                .diagnostic_error
                .clone()
                .unwrap_or_else(|| "diagnostic attempts exhausted".to_string());
            self.record_verifier_diagnostic_unavailable(error.clone());
            return Err(VerifierDiagnosticPassOutcome::Unavailable { error });
        };
        if let Some(current) = self.repair_job.as_mut() {
            current.diagnostic_attempted = true;
            current.assessment_attempts = current.assessment_attempts.saturating_add(1);
        }
        let active_request = self.active_request_text().unwrap_or_default();
        let task_contract = super::task_contract::TaskContract::from_request(&active_request);
        let behavior_projection =
            super::required_behavior::project_behavior_contract(&task_contract);
        self.emit_behavior_contract_projected_if_changed(
            behavior_projection.as_ref(),
            super::required_behavior::BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_DIAGNOSTIC,
        );
        Ok(PreparedVerifierDiagnosticPass {
            context,
            attempt_spec,
            active_request,
            behavior_projection,
        })
    }

    fn reset_verifier_diagnostic_state_if_needed(&mut self) {
        let stale_advance = self.repair_job.as_ref().is_some_and(|job| {
            super::repair_job::has_stale_assessment_after_cluster_advance(job, &self.work_root)
        });
        let target_exhausted = self
            .repair_job
            .as_ref()
            .is_some_and(|job| job.needs_diagnostic_after_target_exhaustion());
        if (stale_advance || target_exhausted)
            && let Some(current) = self.repair_job.as_mut()
        {
            current.assessment = None;
            current.diagnostic_attempted = false;
            if target_exhausted {
                current.repair_target_hint = None;
                current.assessment_bound_cluster_id = None;
            }
        }
    }

    pub(super) fn request_verifier_diagnostic_reply(
        &mut self,
        prepared: &PreparedVerifierDiagnosticPass,
    ) -> Result<String, VerifierDiagnosticPassOutcome> {
        let messages = verifier_diagnostic_messages(
            &self.work_root,
            &prepared.context,
            &prepared.active_request,
            prepared.behavior_projection.as_ref(),
        );
        let diagnostic_client = match self.client.clone_with_overrides(
            prepared.attempt_spec.timeout_secs,
            VERIFIER_DIAGNOSTIC_MAX_PREDICT,
        ) {
            Ok(client) => client,
            Err(err) => {
                return Err(self.handle_verifier_diagnostic_failure(
                    format!("client clone failed: {err}"),
                    prepared.attempt_spec.role,
                ));
            }
        };
        let reply = match diagnostic_client
            .chat_text_json_control(&prepared.attempt_spec.model, &messages)
        {
            Ok(reply) => reply,
            Err(err) => {
                return Err(
                    self.handle_verifier_diagnostic_failure(err, prepared.attempt_spec.role)
                );
            }
        };
        if !reply.tool_calls.is_empty() {
            return Err(self.handle_verifier_diagnostic_failure(
                "diagnostic reply contained unexpected tool calls".to_string(),
                prepared.attempt_spec.role,
            ));
        }
        Ok(reply.content)
    }

    pub(super) fn handle_verifier_diagnostic_failure(
        &mut self,
        error: String,
        model_role: &'static str,
    ) -> VerifierDiagnosticPassOutcome {
        let compact = self.record_verifier_diagnostic_failure(error, model_role);
        let attempts_done = self
            .repair_job
            .as_ref()
            .map(|context| context.assessment_attempts)
            .unwrap_or(VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT);
        if verifier_diagnostic_attempt_spec(
            &self.models.main,
            self.models.sidecar.as_deref(),
            attempts_done,
        )
        .is_some()
        {
            VerifierDiagnosticPassOutcome::RetryPending { error: compact }
        } else {
            self.record_verifier_diagnostic_unavailable(compact.clone());
            VerifierDiagnosticPassOutcome::Unavailable { error: compact }
        }
    }

    fn record_verifier_diagnostic_failure(
        &mut self,
        error: String,
        model_role: &'static str,
    ) -> String {
        // Issue #637 (CB-002): sanitize at the RepairJob store boundary so
        // the SSOT pipeline (mask_secrets + mask_header_family +
        // control-char neutralization) is applied before the text reaches
        // prompt payloads / log events.
        let error = super::repair_job::sanitize_repair_job_text_with_char_cap(&error, 180);
        if let Some(context) = self.repair_job.as_mut() {
            context.repair_target_hint = None;
            context.assessment = None;
            context.diagnostic_error = Some(error.clone());
            context.apply_event(super::repair_job::RepairJobEvent::DiagnosticMalformed);
        }
        // Issue #638 (CB-001 reflected): also refresh the bounded snapshot
        // on every retry-pending diagnostic failure. Without this, attempts
        // remaining mid-loop leave `repair_failure_snapshot` stale and the
        // production wiring acceptance condition (work-plan Task 1.4) only
        // holds at terminal `record_verifier_diagnostic_unavailable`.
        self.repair_failure_snapshot = self.repair_job.as_ref().map(|job| job.failure_snapshot());
        log_llm_event(
            "agent.verifier_diagnostic.failed",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "role": model_role,
                "error": error,
            }),
        );
        error
    }

    fn record_verifier_diagnostic_unavailable(&mut self, error: String) {
        // Issue #637 (CB-002): same SSOT sanitization as
        // `record_verifier_diagnostic_failure`. We deliberately call the
        // sanitizer (not the raw `compact_verifier_failure_text`) so the
        // store-boundary invariant holds for every diagnostic_error write.
        let error = super::repair_job::sanitize_repair_job_text_with_char_cap(&error, 180);
        if let Some(context) = self.repair_job.as_mut() {
            context.diagnostic_unavailable = true;
            context.diagnostic_error = Some(error.clone());
            context.apply_event(super::repair_job::RepairJobEvent::DiagnosticUnavailable);
        }
        // Issue #638 (Task 1.4): capture a bounded snapshot when diagnostic
        // becomes unavailable. Turn-local only — not pushed to session.messages
        // (design policy §5, A-only). Re-runs SSOT sanitizers as defence-in-depth.
        self.repair_failure_snapshot = self.repair_job.as_ref().map(|job| job.failure_snapshot());
        log_llm_event(
            "agent.verifier_diagnostic.unavailable",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "error": error,
            }),
        );
        // Issue #654: also emit the bounded structured safe stop report so
        // downstream consumers (`/bug-fix`, semantic repair plan) can see
        // role / expected target / actual actions / hypothesis ledger in a
        // single structured event. Per-StopReason dedup is enforced by
        // `record_safe_stop_report`.
        self.emit_safe_stop_report_for_diagnostic_target_missing();
    }

    /// Issue #654 / CB-003 — `current_role` SSOT fallback for safe-stop
    /// report emission. Resolves the per-§6.4 priority order so all five
    /// emit paths (`diagnostic_target_missing`, `verifier_failed_safe_stop`,
    /// `verifier_weak`, `verifier_missing`, `artifact_completion_failed`)
    /// reach into the same single source of truth.
    ///
    /// Priority order (first hit wins; `None` only when every source is
    /// silent):
    /// 1. `repair_job.semantic_plan.preferred_repair_role`
    ///    — the verifier-driven semantic planner's authoritative pick.
    /// 2. `current_artifact_recovery_target.role`
    ///    — the selected artifact recovery target the planner is currently
    ///    driving toward.
    /// 3. `repair_job.target_hint.role`
    ///    — the explicit repair target hint carried on `RepairJob`.
    /// 4. The first `required_artifact` of the active `TaskContract`
    ///    reconstructed from the current request text.
    ///
    /// `explicit_role` allows the `artifact_completion_failed` path to
    /// preempt the priority order with the role the host emit point already
    /// has in hand (the role the retry budget exhausted on).
    pub(super) fn resolve_current_role_for_safe_stop(
        &self,
        explicit_role: Option<super::task_contract::ArtifactRole>,
    ) -> Option<super::task_contract::ArtifactRole> {
        if let Some(role) = explicit_role {
            return Some(role);
        }
        if let Some(job) = self.repair_job.as_ref()
            && let Some(plan) = job.semantic_plan.as_ref()
        {
            return Some(plan.preferred_repair_role);
        }
        if let Some(target) = self.current_artifact_recovery_target.as_ref() {
            return Some(target.role);
        }
        if let Some(job) = self.repair_job.as_ref()
            && let Some(hint) = job.target_hint.as_ref()
        {
            return Some(hint.role);
        }
        let request = self.active_request_text().unwrap_or_default();
        if !request.is_empty() {
            let contract = super::task_contract::TaskContract::from_request(&request);
            if let Some(role) = contract.required_artifacts.first().copied() {
                return Some(role);
            }
        }
        None
    }

    /// Issue #654 — diagnostic_target_missing path: build a
    /// `SafeStopInput::FromRepair { DiagnosticTargetMissing }` and forward to
    /// `record_safe_stop_report`. Pulled into a separate method so the
    /// orchestration of context-collection from `Agent` state stays out of
    /// the lifecycle of `record_verifier_diagnostic_unavailable`.
    pub(super) fn emit_safe_stop_report_for_diagnostic_target_missing(&mut self) {
        let Some(job) = self.repair_job.clone() else {
            return;
        };
        let session_id = self.session_store.session_id().to_string();
        let turn_index = self.current_turn_index as u64;
        let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
            &self.work_root,
            self.active_request_text().unwrap_or_default().as_str(),
        );
        let candidates: Vec<String> = job
            .changed_file_hints
            .iter()
            .map(|hint| hint.path.clone())
            .collect();
        let latest_read = latest_successful_read_existing_path(
            &self.session.messages,
            &self.work_root,
            latest_verifier_repair_note_index(&self.session.messages),
        );
        let expected = job
            .target_hint
            .as_ref()
            .map(|hint| std::path::PathBuf::from(&hint.path));
        // CB-003: route through the §6.4 SSOT fallback chain so the
        // diagnostic_target_missing path is not limited to semantic_plan.
        let current_role = self.resolve_current_role_for_safe_stop(None);
        let input = super::repair_job::SafeStopInput::FromRepair {
            job: &job,
            stop_reason: super::repair_job::StopReason::DiagnosticTargetMissing,
            owned_test_artifacts: Vec::new(),
        };
        let ctx = super::repair_job::SafeStopContext {
            current_role,
            expected_target: expected.as_deref(),
            actual_actions_raw: collect_recent_action_labels(&self.session.messages),
            latest_successful_read: latest_read.as_deref(),
            task_workspace_scope: &scope,
            candidates,
            session_id: &session_id,
            turn_index,
        };
        self.record_safe_stop_report(input, ctx);
    }

    /// Issue #654 (Task D.4) — thin shell that dedups per StopReason, builds
    /// the structured report via `SafeStopReport::build_from`, renders the
    /// bounded payload via `build_safe_stop_payload`, and emits the
    /// `agent.safe_stop.report` event through `log_llm_event` (which routes
    /// through `mask_payload_inplace` — the final defense line).
    pub(super) fn record_safe_stop_report(
        &mut self,
        input: super::repair_job::SafeStopInput<'_>,
        ctx: super::repair_job::SafeStopContext<'_>,
    ) {
        let stop_reason = input.stop_reason();
        if self.safe_stop_report_emitted.contains(&stop_reason) {
            return;
        }
        // Issue #666 (CB-001 fix): emit per-turn job reports BEFORE the
        // SafeStopReport so the llm-io event order is
        // `agent.{x}.report → agent.safe_stop.report` per design Section
        // 8-2. Per-turn dedup in `maybe_emit_job_reports` makes the
        // post-`run_turn` finalizer at `handle_user_message` a no-op for
        // any kind already emitted here. We record the stop_reason in
        // `safe_stop_report_emitted` AFTER `maybe_emit_job_reports` so
        // the SafeStopLinkage built into the job reports observes the
        // pre-stop state of the dedup set (`report_emitted=false`); the
        // job reports still carry the actual stop_reason via
        // `safe_stop.reason` once the post-run finalizer is dedup'd out.
        // Order: build linkage → emit 4 job reports → emit safe_stop.
        let linkage_reason = Some(stop_reason.as_str().to_string());
        self.maybe_emit_job_reports_with_linkage(linkage_reason);
        let report = super::repair_job::SafeStopReport::build_from(input, ctx);
        let payload = build_safe_stop_payload(&report);
        log_llm_event("agent.safe_stop.report", payload);
        self.safe_stop_report_emitted.insert(stop_reason);
    }

    /// Issue #654 (E.3) — `verifier_failed_safe_stop` emit shell. Builds a
    /// `FromRepair { VerifierFailedSafeStop }` input from `self.repair_job`.
    pub(super) fn emit_safe_stop_report_for_verifier_failed_safe_stop(&mut self) {
        self.emit_repair_safe_stop_report(super::repair_job::StopReason::VerifierFailedSafeStop);
    }

    #[cfg(test)]
    pub(super) fn emit_safe_stop_report_for_verifier_weak(&mut self) {
        self.emit_repair_safe_stop_report(super::repair_job::StopReason::VerifierWeak);
    }

    /// Issue #654 (E.2) — `artifact_completion_failed` emit shell. Unlike the
    /// other shells, this path can fire BEFORE a verifier-driven `RepairJob`
    /// has been built (the role-specific retry budget exhausts during pure
    /// artifact-completion attempts). When `self.repair_job` is `None` we
    /// fall back to a synthetic empty `RepairJob` whose only meaningful field
    /// is `target_hint` (carried from the explicit role + path the caller
    /// supplies) so the emitted payload still carries `current_role` and
    /// `expected_target` for downstream `/bug-fix` consumers.
    pub(super) fn emit_safe_stop_report_for_artifact_completion_failed(
        &mut self,
        role: super::task_contract::ArtifactRole,
        expected_target_path: Option<String>,
    ) {
        if self
            .safe_stop_report_emitted
            .contains(&super::repair_job::StopReason::ArtifactCompletionFailed)
        {
            return;
        }
        let session_id = self.session_store.session_id().to_string();
        let turn_index = self.current_turn_index as u64;
        let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
            &self.work_root,
            self.active_request_text().unwrap_or_default().as_str(),
        );
        let owned_test_artifacts = self.collect_owned_test_artifacts();
        // Prefer the real RepairJob (when one exists, e.g. the RepairArtifact
        // exhaustion path at the verifier-repair stage) so failure_signature /
        // command / output_excerpt remain accurate. Otherwise fall back to a
        // synthetic shell whose only carried context is the target path the
        // caller provided.
        let job = match self.repair_job.clone() {
            Some(job) => job,
            None => {
                let mut shell = super::repair_job::RepairJob::empty_synthetic();
                if let Some(path) = expected_target_path.clone() {
                    shell.target_hint = Some(super::task_contract::RecoveryTargetHint {
                        role,
                        path,
                        reason: "artifact_completion_failed".to_string(),
                    });
                }
                shell
            }
        };
        let expected = expected_target_path
            .map(std::path::PathBuf::from)
            .or_else(|| {
                job.target_hint
                    .as_ref()
                    .map(|hint| std::path::PathBuf::from(&hint.path))
            });
        // CB-003: still preempt with the explicit role the caller supplied
        // (the artifact-completion retry budget exhausted on this role), but
        // route the no-explicit-role branches through the §6.4 helper so the
        // priority order (semantic_plan -> recovery target -> target_hint ->
        // contract) stays single-sourced.
        let current_role = self.resolve_current_role_for_safe_stop(Some(role));
        let candidates: Vec<String> = job
            .changed_file_hints
            .iter()
            .map(|hint| hint.path.clone())
            .collect();
        let input = super::repair_job::SafeStopInput::FromRepair {
            job: &job,
            stop_reason: super::repair_job::StopReason::ArtifactCompletionFailed,
            owned_test_artifacts,
        };
        let ctx = super::repair_job::SafeStopContext {
            current_role,
            expected_target: expected.as_deref(),
            actual_actions_raw: collect_recent_action_labels(&self.session.messages),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates,
            session_id: &session_id,
            turn_index,
        };
        self.record_safe_stop_report(input, ctx);
    }

    /// Shared helper for repair-job-driven emit paths (E.2 / E.3 / E.4).
    pub(super) fn emit_repair_safe_stop_report(
        &mut self,
        stop_reason: super::repair_job::StopReason,
    ) {
        let Some(job) = self.repair_job.clone() else {
            return;
        };
        let session_id = self.session_store.session_id().to_string();
        let turn_index = self.current_turn_index as u64;
        let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
            &self.work_root,
            self.active_request_text().unwrap_or_default().as_str(),
        );
        let owned_test_artifacts = self.collect_owned_test_artifacts();
        let expected = job
            .target_hint
            .as_ref()
            .map(|hint| std::path::PathBuf::from(&hint.path));
        // CB-003: route through the §6.4 SSOT fallback chain.
        let current_role = self.resolve_current_role_for_safe_stop(None);
        let candidates: Vec<String> = job
            .changed_file_hints
            .iter()
            .map(|hint| hint.path.clone())
            .collect();
        let input = super::repair_job::SafeStopInput::FromRepair {
            job: &job,
            stop_reason,
            owned_test_artifacts,
        };
        let ctx = super::repair_job::SafeStopContext {
            current_role,
            expected_target: expected.as_deref(),
            actual_actions_raw: collect_recent_action_labels(&self.session.messages),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates,
            session_id: &session_id,
            turn_index,
        };
        self.record_safe_stop_report(input, ctx);
    }

    /// Issue #654 (E.5) — `verifier_missing` emit shell. Uses the
    /// `FromMissingVerifier` builder so empty `failure_signature` fallback
    /// detection by downstream consumers does not misfire (R8).
    pub(super) fn emit_safe_stop_report_for_verifier_missing(&mut self) {
        let Some(job) = self.missing_verifier_job.clone() else {
            return;
        };
        let session_id = self.session_store.session_id().to_string();
        let turn_index = self.current_turn_index as u64;
        let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
            &self.work_root,
            self.active_request_text().unwrap_or_default().as_str(),
        );
        let owned_test_artifacts = self.collect_owned_test_artifacts();
        let input = super::repair_job::SafeStopInput::FromMissingVerifier {
            job: &job,
            owned_test_artifacts,
        };
        // CB-003: verifier_missing has no `RepairJob`, so the helper falls
        // through to current_artifact_recovery_target -> reconstructed
        // TaskContract.required_artifacts.first() for downstream `/bug-fix`.
        let current_role = self.resolve_current_role_for_safe_stop(None);
        let ctx = super::repair_job::SafeStopContext {
            current_role,
            expected_target: None,
            actual_actions_raw: collect_recent_action_labels(&self.session.messages),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates: Vec::new(),
            session_id: &session_id,
            turn_index,
        };
        self.record_safe_stop_report(input, ctx);
    }

    /// Issue #654 (DR3-002 / Task D.6) — collect Owned-validated test artifact
    /// relative paths from the current turn's edits, gated by
    /// `classify_ownership`. Returns at most `SAFE_STOP_OWNED_TEST_ARTIFACTS_MAX`
    /// paths; the builder re-applies syntactic safety as defense-in-depth.
    fn collect_owned_test_artifacts(&self) -> Vec<String> {
        let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
            &self.work_root,
            self.active_request_text().unwrap_or_default().as_str(),
        );
        let mut out: Vec<String> = Vec::new();
        for rel in self.turn_edited_relative_paths.iter() {
            // Only test files qualify.
            if !crate::util::file_classify::is_test_file(std::path::Path::new(rel)) {
                continue;
            }
            let inputs = super::artifact_ownership::OwnershipInputs {
                work_root: &self.work_root,
                relative_path: rel.as_str(),
                scope: &scope,
                edited_this_session: true,
                scaffold_changed: false,
                verifier_passed_in_scope: false,
                // Issue #661 (Task 3.1): legacy helper — not one of the 4
                // verifier-path SSOT sites. Iteration-3 may revisit if this
                // shadow consumer needs verifier-binding semantics.
                nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
            };
            if super::artifact_ownership::classify_ownership(inputs)
                == super::artifact_ownership::ArtifactOwnership::Owned
            {
                out.push(rel.clone());
            }
        }
        out.sort();
        out.truncate(8);
        out
    }

    pub(super) fn verifier_repair_pass_wall_clock_timeout_error(
        &self,
        prepared: &PreparedVerifierRepairPass,
        target_hint: &super::task_contract::RecoveryTargetHint,
        attempt: usize,
        elapsed: Duration,
    ) -> String {
        let error = verifier_repair_pass_timeout_error(elapsed);
        self.log_verifier_repair_pass_timeout(prepared, target_hint, attempt, elapsed, None);
        error
    }

    fn log_verifier_repair_pass_timeout(
        &self,
        prepared: &PreparedVerifierRepairPass,
        target_hint: &super::task_contract::RecoveryTargetHint,
        attempt: usize,
        elapsed: Duration,
        attempt_timeout_secs: Option<u64>,
    ) {
        log_llm_event(
            "agent.verifier_repair_pass.timeout",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "model": &prepared.model,
                "path": target_hint.path,
                "attempt": attempt,
                "elapsed_secs": elapsed.as_secs(),
                "limit_secs": VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS,
                "attempt_timeout_secs": attempt_timeout_secs,
            }),
        );
    }

    pub(super) fn handle_verifier_repair_pass_attempt(
        &mut self,
        prepared: &mut PreparedVerifierRepairPass,
        target_hint: &super::task_contract::RecoveryTargetHint,
        attempt: usize,
        attempt_timeout_secs: u64,
        elapsed: Duration,
        reply: Result<AssistantReply, String>,
    ) -> VerifierRepairAttemptProgress {
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                let last_error =
                    verifier_repair_pass_request_error_message(&err, attempt_timeout_secs);
                if last_error.contains("verifier_repair_pass_timeout") {
                    self.log_verifier_repair_pass_timeout(
                        prepared,
                        target_hint,
                        attempt,
                        elapsed,
                        Some(attempt_timeout_secs),
                    );
                }
                return VerifierRepairAttemptProgress::Break { last_error };
            }
        };
        if !reply.tool_calls.is_empty() {
            return VerifierRepairAttemptProgress::Continue {
                last_error: "repair reply contained unexpected tool calls".to_string(),
                last_invalid_outcome: None,
            };
        }
        match self.handle_verifier_repair_pass_reply(prepared, target_hint, attempt, &reply.content)
        {
            Ok(outcome) => VerifierRepairAttemptProgress::Return(outcome),
            Err(ValidationFailure {
                outcome: CheapCheckOutcome::Failed(message),
                weakening,
                rejection_signal,
            }) => VerifierRepairAttemptProgress::Continue {
                last_error: message,
                last_invalid_outcome: build_verifier_repair_pass_ledger_outcome(
                    weakening,
                    rejection_signal,
                    prepared.context.semantic_plan.as_ref(),
                ),
            },
            Err(ValidationFailure {
                outcome: CheapCheckOutcome::Unavailable,
                ..
            }) => {
                log_llm_event(
                    "agent.verifier_repair_pass.unavailable",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "model": &prepared.model,
                        "target_path": target_hint.path,
                        "attempt": attempt,
                    }),
                );
                VerifierRepairAttemptProgress::Return(VerifierRepairPassOutcome::Unavailable {
                    relative_path: target_hint.path.clone(),
                })
            }
        }
    }

    pub(super) fn prepare_verifier_repair_pass(
        &mut self,
        target_hint: &super::task_contract::RecoveryTargetHint,
    ) -> Result<PreparedVerifierRepairPass, VerifierRepairPassOutcome> {
        let Some(context) = self.repair_job.clone() else {
            return Err(VerifierRepairPassOutcome::Skipped);
        };
        let active_request = self.active_request_text().unwrap_or_default();
        let task_contract = super::task_contract::TaskContract::from_request(&active_request);
        let behavior_projection =
            super::required_behavior::project_behavior_contract(&task_contract);
        self.emit_behavior_contract_projected_if_changed(
            behavior_projection.as_ref(),
            super::required_behavior::BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_REPAIR,
        );
        let behavior_contract_has_repair_authority =
            super::required_behavior::behavior_contract_has_repair_authority(&task_contract);
        let legacy_input = context
            .assessment
            .as_ref()
            .map(|assessment| legacy_repair_brief_input_from_assessment(assessment, 0.6));
        let accepted_plan = match super::repair_plan_admission::validate_repair_plan_admission(
            super::repair_plan_admission::RepairPlanAdmissionInput {
                context: &context,
                active_request: &active_request,
                behavior_contract_present: behavior_contract_has_repair_authority,
                legacy_input,
            },
        ) {
            Ok(plan) => plan,
            Err(err) => {
                let reason = err.message();
                let error = format!("verifier_repair_pass_invalid: {reason}");
                if let Some(job) = self.repair_job.as_mut() {
                    job.apply_event(super::repair_plan_admission::admission_error_event(&err));
                }
                log_llm_event(
                    "agent.verifier_repair_plan.rejected",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "target_path": target_hint.path,
                        "reason": reason,
                    }),
                );
                return Err(VerifierRepairPassOutcome::Invalid {
                    error,
                    repair_attempt_outcome: None,
                });
            }
        };
        if let Err(err) = validate_accepted_repair_plan_authorizes_target(
            &accepted_plan,
            target_hint,
            &target_hint.path,
        ) {
            return Err(VerifierRepairPassOutcome::Invalid {
                error: format!("verifier_repair_pass_invalid: {}", err.reason_label()),
                repair_attempt_outcome: build_verifier_repair_pass_ledger_outcome(
                    err.weakening,
                    err.rejection_signal,
                    context.semantic_plan.as_ref(),
                ),
            });
        }
        let messages = match verifier_repair_pass_messages(
            &self.work_root,
            &context,
            target_hint,
            &active_request,
            behavior_projection.as_ref(),
        ) {
            Ok(messages) => messages,
            Err(err) => {
                return Err(VerifierRepairPassOutcome::Invalid {
                    error: format!("verifier_repair_pass_invalid: {err}"),
                    repair_attempt_outcome: None,
                });
            }
        };
        Ok(PreparedVerifierRepairPass {
            context,
            accepted_plan,
            messages,
            model: self.models.main.clone(),
        })
    }

    fn handle_verifier_repair_pass_reply(
        &mut self,
        prepared: &mut PreparedVerifierRepairPass,
        target_hint: &super::task_contract::RecoveryTargetHint,
        attempt: usize,
        reply_content: &str,
    ) -> Result<VerifierRepairPassOutcome, ValidationFailure> {
        let validation =
            super::repair_patch_validation::parse_verifier_repair_patch_proposal_reply(
                reply_content,
                verifier_repair_intent_limits(),
            )
            .map_err(|message| {
                ValidationFailure::failed_with_signal(message, RepairRejectionSignal::Malformed)
            })
            .and_then(|proposal| {
                let shadow_validation = emit_patch_proposal_shadow_validation_event(
                    self.session_store.session_id(),
                    &prepared.model,
                    attempt,
                    &self.work_root,
                    &prepared.accepted_plan,
                    &proposal,
                );
                if shadow_validation.is_decisive() && !shadow_validation.accepted() {
                    return Err(ValidationFailure::failed_with_signal(
                        format!(
                            "patch provider admission rejected: {}",
                            shadow_validation.reason
                        ),
                        RepairRejectionSignal::Malformed,
                    ));
                }
                let intents =
                    super::repair_patch_validation::patch_proposal_to_verifier_repair_intents(
                        proposal.clone(),
                        verifier_repair_intent_limits(),
                    )
                    .map_err(|message| {
                        ValidationFailure::failed_with_signal(
                            message,
                            RepairRejectionSignal::Malformed,
                        )
                    })?;
                let validation = validate_verifier_repair_intents_with_accepted_plan(
                    &self.work_root,
                    &prepared.context,
                    target_hint,
                    &prepared.accepted_plan,
                    intents,
                );
                emit_patch_proposal_legacy_validation_comparison_event(
                    self.session_store.session_id(),
                    &prepared.model,
                    attempt,
                    &proposal,
                    shadow_validation,
                    &validation,
                );
                validation
            });
        match validation {
            Ok(edit) => super::verifier_orchestration::apply_verifier_repair_pass_edit(
                self,
                prepared,
                target_hint,
                attempt,
                edit,
            ),
            Err(error) => Err(error),
        }
    }

    pub(super) fn push_artifact_directed_recovery_note(&mut self, attempt: usize) -> bool {
        if self.focused_edit_recovery_target().is_some() {
            return false;
        }
        let Some(target) = self.current_artifact_recovery_target.as_ref() else {
            return false;
        };
        self.push_system_note(recovery::artifact_directed_recovery_note(
            target.role.label(),
            &target.path,
            attempt,
        ));
        true
    }

    /// Issue #652: record an attempt against the active
    /// `ArtifactCompletionJob` (no-op when no job exists). Triggers the
    /// turn-local `artifact_completion_failed` diagnostic when the
    /// recording causes the job to transition to `Exhausted`.
    pub(super) fn record_artifact_completion_attempt(
        &mut self,
        kind: super::artifact_completion_job::ArtifactAttemptOutcomeKind,
        actual_actions: Vec<String>,
    ) -> bool {
        let expected_target = match self.artifact_completion_job.as_ref() {
            Some(job) => job.target_path().to_string(),
            None => return false,
        };
        let outcome = super::artifact_completion_job::ArtifactAttemptOutcome::new(
            kind,
            actual_actions,
            expected_target,
        );
        self.record_artifact_completion_outcome(outcome)
    }

    /// Issue #664 iteration-2 (CB-003): record a Bash policy violation
    /// against the active `ArtifactCompletionJob`. The outcome carries the
    /// non-raw `bash_policy_violation = true` marker so
    /// `attempt_outcome_to_json_value` emits
    /// `category = "bash_out_of_policy"`.
    ///
    /// Raw command bytes are NOT stored verbatim — `actual_actions` is
    /// sanitized at `ArtifactAttemptOutcome::new` (`mask_secrets` + length
    /// cap + control-char neutralize) and hashed via `stable_path_hash`
    /// at projection time (AD5 / CB-004).
    fn record_artifact_completion_bash_violation(&mut self, actual_actions: Vec<String>) -> bool {
        let expected_target = match self.artifact_completion_job.as_ref() {
            Some(job) => job.target_path().to_string(),
            None => return false,
        };
        let outcome =
            super::artifact_completion_job::ArtifactAttemptOutcome::new_bash_policy_violation(
                actual_actions,
                expected_target,
            );
        self.record_artifact_completion_outcome(outcome)
    }

    /// Shared core: append `outcome` to the active job's attempt history
    /// and trigger the turn-local exhaustion diagnostic when the job
    /// transitions to `Exhausted`.
    fn record_artifact_completion_outcome(
        &mut self,
        outcome: super::artifact_completion_job::ArtifactAttemptOutcome,
    ) -> bool {
        let status_after = match self.artifact_completion_job.as_mut() {
            Some(job) => job.record_attempt(outcome),
            None => return false,
        };
        if matches!(
            status_after,
            super::artifact_completion_job::ArtifactCompletionStatus::Exhausted { .. }
        )
        // Issue #663 (Phase A): with 5 variants the `Exhausted` match
        // remains the only terminal-failure trigger; new states do not
        // alter the diagnostic surface.
        {
            // CB-002: tag the turn so the actor loop terminates with
            // `MissingRepoEdits` even on call paths that previously
            // dropped the return value (artifact-directed policy
            // WrongTarget). Emit the diagnostic only once per
            // exhaustion (`maybe_emit_..._diagnostic` is gated by the
            // same flag).
            let first_exhaustion = !self.artifact_completion_exhausted_this_turn;
            self.artifact_completion_exhausted_this_turn = true;
            if first_exhaustion {
                self.maybe_emit_artifact_completion_failed_diagnostic();
            }
            true
        } else {
            false
        }
    }

    pub(super) fn focused_edit_no_tool_note_for_target(
        &self,
        target: &Path,
        target_already_read: bool,
        attempt: usize,
    ) -> String {
        let target_display = progress_path_display(
            &target.display().to_string(),
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
            120,
        );
        if !target.is_file() {
            recovery::focused_edit_missing_target_recovery_note(&target_display, attempt)
        } else {
            recovery::focused_edit_no_tool_recovery_note(
                &target_display,
                target_already_read,
                attempt,
            )
        }
    }

    pub(super) fn focused_edit_no_tool_note_for_policy(
        &self,
        policy: &FocusedEditPolicy,
        effective_tool_policy: &EffectiveToolPolicy,
        attempt: usize,
    ) -> String {
        let target_display = progress_path_display(
            &policy.target.display().to_string(),
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
            120,
        );
        if effective_tool_policy.reason() == EffectiveToolPolicyReason::VerifierRepair {
            match effective_tool_policy.allowed_tool_names_for_prompt() {
                Some(["Read"]) => {
                    return format!(
                        "Verifier repair is waiting for a fresh read of {target_display}. The previous response was not executed. Emit exactly one Read tool call on that file now. Do not call Edit, Bash, Glob, Grep, or answer in prose. verifier_repair_read_attempt={attempt}"
                    );
                }
                Some(["Edit"]) => {
                    return format!(
                        "Verifier repair is waiting for a compact edit of {target_display}. The previous response was not executed. Emit exactly one Edit tool call on that file now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_edit_attempt={attempt}"
                    );
                }
                Some(["Write"]) => {
                    return format!(
                        "Verifier repair is waiting for the missing target {target_display}. The previous response was not executed. Emit exactly one Write tool call on that exact path now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_write_attempt={attempt}"
                    );
                }
                _ => {}
            }
        }
        self.focused_edit_no_tool_note_for_target(
            &policy.target,
            policy.target_already_read,
            attempt,
        )
    }

    fn artifact_directed_recovery_message(
        &self,
        effective_tool_policy: &EffectiveToolPolicy,
    ) -> Option<String> {
        let policy = effective_tool_policy.artifact_directed_policy()?;
        let target = self.current_artifact_recovery_target.as_ref()?;
        let target_display = progress_path_display(
            &policy.target.display().to_string(),
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
            120,
        );
        let allowed = effective_tool_policy
            .allowed_tool_names_for_prompt()
            .map(|tools| tools.join(", "))
            .unwrap_or_else(|| "Read, Write, Edit".to_string());
        let read_guidance = if policy.target_already_read {
            " The target has already been read in this session, so do not call Read again."
        } else {
            ""
        };
        Some(format!(
            "[Artifact Directed Recovery] Missing role: {}. Target file: {target_display}. Allowed tools for this turn are {allowed} on that exact target path only.{read_guidance} Do not call Bash, Glob, Grep, or switch files. Use Write if a small scaffold file should be replaced; otherwise use a compact Edit.",
            target.role.label()
        ))
    }

    fn verifier_repair_policy_message(
        &self,
        effective_tool_policy: &EffectiveToolPolicy,
    ) -> Option<String> {
        (effective_tool_policy.reason() == EffectiveToolPolicyReason::VerifierRepair).then(|| {
            let context = self.repair_job.as_ref();
            let diagnostics = verifier_repair_context_diagnostics(context);
            match context.map(|context| context.next_action()) {
                Some(
                    super::repair_job::RepairNextAction::RequestDiagnostic
                    | super::repair_job::RepairNextAction::Replan,
                ) => self.verifier_repair_diagnostic_policy_message(),
                Some(super::repair_job::RepairNextAction::RequestPatch { target_hint }) => {
                    self.verifier_repair_request_patch_message(&diagnostics, &target_hint)
                }
                Some(super::repair_job::RepairNextAction::SafeStop { .. }) => {
                    verifier_repair_safe_stop_message()
                }
                Some(
                    super::repair_job::RepairNextAction::RerunVerifier
                    | super::repair_job::RepairNextAction::VerifiedDone,
                ) => verifier_repair_transition_message(),
                None if self.missing_verifier_job.is_some() => {
                    verifier_setup_policy_message(&self.active_request_text().unwrap_or_default())
                }
                None => verifier_repair_transition_message(),
            }
        })
    }

    fn verifier_repair_diagnostic_policy_message(&self) -> String {
        let active_request = self.active_request_text().unwrap_or_default();
        let task_contract = super::task_contract::TaskContract::from_request(&active_request);
        let behavior_projection =
            super::required_behavior::project_behavior_contract(&task_contract);
        self.repair_job
            .as_ref()
            .map(|context| {
                verifier_repair_diagnostic_pending_note(context, behavior_projection.as_ref())
            })
            .unwrap_or_else(|| {
                "[Verifier Repair Policy] A verifier failure is pending. Output a compact diagnosis JSON object only; do not call tools.".to_string()
            })
    }

    fn verifier_repair_request_patch_message(
        &self,
        diagnostics: &str,
        target_hint: &super::task_contract::RecoveryTargetHint,
    ) -> String {
        let Some(relative) = super::repair_job::safe_relative_path_string(&target_hint.path) else {
            return verifier_repair_unsafe_target_message();
        };
        let target = self.work_root.join(relative);
        let target = std::fs::canonicalize(&target).unwrap_or(target);
        let target_display = verifier_repair_target_display(&target, &self.work_root);
        if !target.is_file() {
            format!(
                "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Missing target file: {target_display}. Next required action: exactly one Write on that target. Do not use Bash, switch files, or finish with prose. Anvil will rerun the verifier after the write."
            )
        } else if focused_edit_target_already_read(&self.session.messages, &target, &self.work_root)
        {
            format!(
                "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Target file: {target_display}. Next required action: exactly one compact Edit on that target. Do not call Read again, Bash, switch files, or finish with prose. Anvil will rerun the verifier after the edit."
            )
        } else {
            format!(
                "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Target file: {target_display}. Next required action: exactly one Read on that target. Do not use Edit, Bash, switch files, or finish with prose."
            )
        }
    }

    fn artifact_directed_policy_violation_message(
        &self,
        effective_tool_policy: &EffectiveToolPolicy,
    ) -> Option<String> {
        let policy = effective_tool_policy.artifact_directed_policy()?;
        let target_display = policy
            .target
            .strip_prefix(&self.work_root)
            .unwrap_or(&policy.target)
            .to_string_lossy()
            .replace('\\', "/");
        focused_edit_policy_violation_feedback_note(
            &self.session.working_memory.unresolved_errors,
            effective_tool_policy.allowed_tool_names_for_prompt(),
            Some(&target_display),
        )
    }

    pub(super) fn push_deterministic_ui_recovery_continuation_note(
        &mut self,
        target_path: &str,
        attempt: usize,
    ) {
        self.push_system_note(format!(
            "Deterministic UI recovery updated {target_path}, but this is recovery context, not completion. Inspect the file if needed, then make one small model-produced Edit or run the project verifier before finalizing. deterministic_ui_recovery_attempt={attempt}"
        ));
    }

    pub(super) fn execute_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        effective_tool_policy: Option<&EffectiveToolPolicy>,
        cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> String {
        if let Some(err) = self.answer_only_policy_error(name, arguments) {
            self.session.working_memory.note_error(err.clone());
            return lifecycle::format_tool_error(&err);
        }
        if let Some(err) =
            super::scaffold_pipeline::empty_workspace_scaffold_policy_error(self, name, arguments)
        {
            self.session.working_memory.note_error(err.clone());
            return lifecycle::format_tool_error(&err);
        }
        // Issue #646 (A1/A3): when a first-class MissingVerifierJob is
        // active, hand the active workspace scope to the policy gate so
        // out-of-scope `Write`/`Edit` paths are rejected even when the
        // restricted whitelist would otherwise admit them.
        if let Some(err) =
            self.effective_tool_policy_error_for_execution(name, arguments, effective_tool_policy)
        {
            return self.handle_tool_execution_rejection(name, arguments, &err);
        }
        if cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            return user_interrupt_result();
        }
        let context = self.tool_context(cancel_flag);
        if name == "Bash" {
            return self.execute_bash_tool_call(name, arguments, &context);
        }

        self.execute_non_bash_tool_call(name, arguments, &context)
    }

    fn effective_tool_policy_error_for_execution(
        &self,
        name: &str,
        arguments: &serde_json::Value,
        effective_tool_policy: Option<&EffectiveToolPolicy>,
    ) -> Option<String> {
        let scope_for_policy = self
            .missing_verifier_job
            .as_ref()
            .map(|_| self.current_workspace_scope());
        if let Some(policy) = effective_tool_policy {
            effective_tool_policy_error_for_call_with_scope(
                policy,
                name,
                arguments,
                &self.work_root,
                scope_for_policy.as_ref(),
            )
        } else {
            self.effective_tool_policy_error(name, arguments)
        }
    }

    fn handle_tool_execution_rejection(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        err: &str,
    ) -> String {
        let _tool_outcome = rejected_outcome_for_call(name, err);
        self.session.working_memory.note_error(err.to_string());
        if err.contains("artifact-directed recovery rejected") {
            if name == "Bash" {
                let command_arg = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ = self.record_artifact_completion_bash_violation(vec![command_arg]);
            } else {
                let actual_path = arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ = self.record_artifact_completion_attempt(
                    super::artifact_completion_job::ArtifactAttemptOutcomeKind::WrongTarget,
                    vec![format!("{name} on {actual_path}")],
                );
            }
        } else if err.starts_with("setup bootstrap")
            && name == "Bash"
            && self.artifact_completion_job.is_some()
        {
            let command_arg = arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let _ = self.record_artifact_completion_bash_violation(vec![command_arg]);
        }
        lifecycle::format_tool_error(err)
    }

    fn tool_context(
        &self,
        cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> ToolContext {
        let tmp_tests_root = Some(
            self.session_store
                .state_root()
                .join("sessions")
                .join(self.session_store.session_id())
                .join("tmp-tests"),
        );
        ToolContext {
            root: self.work_root.clone(),
            mode: self.session.mode_state.mode,
            plan_path: self.session.mode_state.active_plan_path.clone(),
            plan_stage: self.session.mode_state.plan_stage,
            auto_approve: self.config.yes_mode,
            interactive_approval: io::stdin().is_terminal(),
            offline: self.config.offline,
            cancel_flag,
            tmp_tests_root,
            tester_active: self.tester_called_this_turn,
        }
    }

    fn execute_bash_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> String {
        let (result, outcome) = self
            .tool_registry
            .execute_bash_with_outcome(arguments, context);
        if let Some(outcome) = outcome.as_ref()
            && let Some(frame) = build_feedback_for_bash(outcome, &self.work_root)
        {
            self.session.record_feedback(frame);
        }
        if let Some(outcome) = outcome.as_ref() {
            self.observe_evidence_from_bash_outcome(outcome);
        }
        match result {
            Ok(text) => {
                self.maybe_update_work_root(name, arguments, &text);
                text
            }
            Err((err, class)) => {
                self.session
                    .working_memory
                    .note_error(format!("{name}: {err}"));
                if class == BashErrorClass::DangerousBlock {
                    let cmd = arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let frame = build_feedback_for_unsafe_block_reason(cmd, &err, &self.work_root);
                    self.session.record_feedback(frame);
                    self.session.unsafe_blocks_this_turn =
                        self.session.unsafe_blocks_this_turn.saturating_add(1);
                }
                lifecycle::format_tool_error(&err)
            }
        }
    }

    fn capture_pre_tool_hash_if_needed(&mut self, arguments: &serde_json::Value, name: &str) {
        if matches!(name, "Write" | "Edit")
            && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
            && let Some(rel) = workspace_relative_path_for_tool_arg(&self.work_root, raw_path)
        {
            let pre_hash = current_file_hash_for_relative_path(&self.work_root, &rel);
            self.turn_pre_tool_file_hashes.insert(rel, pre_hash);
        }
    }

    fn execute_non_bash_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> String {
        self.capture_pre_tool_hash_if_needed(arguments, name);
        match self.tool_registry.execute(name, arguments, context) {
            Ok(result) => {
                let tool_outcome = success_outcome_for_call(name, arguments, &self.work_root);
                if let Some(edit) = tool_outcome.repo_edit_evidence() {
                    self.session
                        .working_memory
                        .note_touched_file(normalize_memory_path(edit.raw_path(), &self.work_root));
                    self.session.repo_edit_succeeded_this_turn = true;
                    self.observe_evidence_from_repo_edit(edit.raw_path());
                }
                self.maybe_update_work_root(name, arguments, &result);
                result
            }
            Err(err) => {
                let _tool_outcome = failed_outcome_for_call(name, &err);
                self.session
                    .working_memory
                    .note_error(format!("{name}: {err}"));
                if name == "Edit" {
                    let path = arguments.get("path").and_then(serde_json::Value::as_str);
                    let frame = build_feedback_for_edit_failure(path, &err, &self.work_root);
                    self.session.record_feedback(frame);
                }
                lifecycle::format_tool_error(&err)
            }
        }
    }

    /// Issue #606 (T-1.6): post-hoc observation of a Bash invocation as
    /// `VerifierExitZero` completion evidence. A signal is recorded **only**
    /// when:
    ///
    /// 1. `outcome.exit_code == Some(0)` — non-zero / timeout / interrupted
    ///    invocations are explicit failures, not silent passes.
    /// 2. `outcome.class == BuildTest` — read-only / network / mutating
    ///    classes don't represent verification work even when they
    ///    happen to exit 0.
    /// 3. `is_completion_verifier_command(&outcome.command) == true` —
    ///    rejects commands containing shell control operators that can
    ///    mask the real exit code (DR4-002, e.g. `cargo test || true`).
    ///
    /// The evidence is consumed by
    /// `ProtocolKind::evidence_set_satisfies` in `success.rs`.
    fn observe_evidence_from_bash_outcome(
        &mut self,
        outcome: &crate::tools::bash::BashExecutionOutcome,
    ) {
        use crate::tools::bash::BashCommandClass;
        // Issue #608 Phase α-2 (AP-09): record `last_verifier_command` /
        // `last_verifier_invocation` for any BuildTest invocation (regardless
        // of exit code) so the rerun-trigger handler can surface the most
        // recent verifier attempt — even failed ones (the user often types
        // `再実行` precisely because the last run failed).
        if matches!(outcome.class, BashCommandClass::BuildTest)
            && super::completion_evidence::is_completion_verifier_command(&outcome.command)
        {
            let redacted =
                crate::session::feedback::redact_verifier_command_for_storage(&outcome.command);
            // Drop empty redacted commands (e.g. all-control-char input).
            if !redacted.trim().is_empty() {
                self.session.last_verifier_command = Some(redacted.clone());
                self.session.last_verifier_invocation =
                    Some(crate::session::store::VerifierInvocationRecord {
                        command: redacted,
                        exit_code: outcome.exit_code.unwrap_or(-1),
                        recorded_at: rfc3339_now_utc(),
                    });
            }
        }

        // Issue #607 (β): build VerifierExitZero evidence for BuildTest |
        // EnvSetup exit-zero outcomes (per `build_verifier_exit_zero_evidence`).
        let Some(evidence) = build_verifier_exit_zero_evidence(outcome) else {
            return;
        };
        let crate::agent::loop_run::completion_evidence::CompletionEvidence::VerifierExitZero {
            class,
            ..
        } = evidence
        else {
            // build_verifier_exit_zero_evidence only ever constructs
            // VerifierExitZero today; the match keeps us honest if a future
            // helper returns a different variant.
            self.evidence_set_this_turn.push(evidence.clone());
            if self.current_artifact_recovery_target.is_none() {
                self.task_contract_evidence_set_this_turn.push(evidence);
            }
            return;
        };
        self.evidence_set_this_turn.push(evidence.clone());
        if self.current_artifact_recovery_target.is_none() {
            self.task_contract_evidence_set_this_turn
                .push(evidence.clone());
        }
        crate::logging::log_completion_evidence_observed(
            self.current_turn_index,
            0, // α-1: iter_index plumbing is α-2 work; emit 0 for now.
            "verifier_exit_zero",
            serde_json::json!({
                // Issue #607 BP-07 / S3-002: snake_case label matches serde
                // rename_all so `command_class` reads `"env_setup"` /
                // `"build_test"` instead of `"EnvSetup"` / `"BuildTest"`.
                "command_class": class.as_str(),
            }),
        );
    }

    /// Issue #606 (T-1.7): post-hoc observation of an Edit/Write success
    /// as `RepoEdit` completion evidence. The path is run through
    /// `classify_repo_edit_path` which uses the SSOT in `util::file_classify`
    /// and applies the DR1-001 ordering rule (`.mdx → Docs` even though
    /// `is_implementation_file` would otherwise claim it).
    pub(super) fn observe_evidence_from_repo_edit(&mut self, path: &str) {
        let Some(relative_path) = workspace_relative_path_for_tool_arg(&self.work_root, path)
        else {
            return;
        };
        if is_ignored_workspace_display_path(&relative_path) {
            crate::logging::log_completion_evidence_observed(
                self.current_turn_index,
                0,
                "repo_edit_ignored_controller_state",
                serde_json::json!({
                    "path_hash": stable_path_hash(&relative_path),
                }),
            );
            return;
        }
        let category = super::completion_evidence::classify_repo_edit_path(std::path::Path::new(
            &relative_path,
        ));
        if !super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(self, &relative_path) {
            crate::logging::log_completion_evidence_observed(
                self.current_turn_index,
                0,
                "repo_edit_scaffold_unchanged",
                serde_json::json!({
                    "category": format!("{:?}", category),
                    "path": relative_path,
                }),
            );
            return;
        }
        // Issue #646 (C2 / A4): even after the scaffold-delta gate, a
        // Write/Edit can be a content no-op for a NON-scaffold file (e.g.
        // model writes the same body back, or `Edit` whose `old_string`
        // equals `new_string`). Compare the pre-tool hash captured in
        // `execute_tool_call` against the current on-disk hash. Identical
        // hashes mean the file did not actually change — bail out so the
        // path does NOT enter `turn_edited_relative_paths` and does NOT
        // contribute completion evidence. The pre-tool entry is removed in
        // either branch to keep the cache turn-local and bounded.
        let pre_tool_hash = self.turn_pre_tool_file_hashes.remove(&relative_path);
        let current_hash = current_file_hash_for_relative_path(&self.work_root, &relative_path);
        if is_repo_edit_no_op(
            pre_tool_hash.as_ref().and_then(Option::as_deref),
            current_hash.as_deref(),
        ) {
            crate::logging::log_completion_evidence_observed(
                self.current_turn_index,
                0,
                "repo_edit_no_op",
                serde_json::json!({
                    "category": format!("{:?}", category),
                    "path": relative_path,
                }),
            );
            return;
        }
        // Issue #646 (C2): record the edited path AFTER both the scaffold-
        // delta gate AND the no-op hash check so a content-unchanged
        // Write/Edit (scaffold body re-written, or `Edit` with
        // `old_string == new_string`) never promotes the file to `Owned`.
        self.turn_edited_relative_paths
            .insert(relative_path.clone());
        // Issue #646 (A1/B2): once an in-scope edit has landed, the
        // MissingVerifierJob can begin retrying verifier creation.
        if self.missing_verifier_job.is_some() {
            let in_scope = self.current_workspace_scope().contains(&relative_path);
            if in_scope && let Some(job) = self.missing_verifier_job.as_mut() {
                job.record_in_scope_edit();
            }
        }
        self.evidence_set_this_turn
            .push(super::completion_evidence::CompletionEvidence::RepoEdit { category, count: 1 });
        if self.repo_edit_satisfies_current_artifact_target(category, &relative_path) {
            self.task_contract_evidence_set_this_turn.push(
                super::completion_evidence::CompletionEvidence::RepoEdit { category, count: 1 },
            );
            // Issue #636: capture bounded post-edit excerpt for the
            // current role so `plan_artifact_recovery` can assert that
            // the edit actually carries the requested behavior. Silent
            // skip on role-miss / read failure (back-compat with the
            // existing `repo_edit_has_post_scaffold_delta` no-data path).
            if let Some(role) = super::task_contract::role_from_repo_edit(category)
                && let Some(excerpt) = self.bounded_post_edit_excerpt(&relative_path)
            {
                self.task_contract_excerpts.insert(role, excerpt);
            }
        }
        crate::logging::log_completion_evidence_observed(
            self.current_turn_index,
            0,
            "repo_edit",
            serde_json::json!({
                "category": format!("{:?}", category),
                "path": relative_path,
            }),
        );
        // Issue #659 Task 2.5: write-through seed into the ArtifactLedger
        // SSOT. `relative_path` has already passed the workspace-relative /
        // scaffold-delta / no-op guards; the legacy
        // `turn_edited_relative_paths` insert above stays as the adapter-
        // period authority. The seed is gated by category-to-role mapping
        // so the `Other` category (which legacy callers do not classify
        // into a role) does not inject an ambiguous event.
        if let Some(role) = super::task_contract::role_from_repo_edit(category) {
            let scope = self.current_workspace_scope();
            self.seed_artifact_ledger_repo_edit(&relative_path, role, &scope);
        }
    }

    /// Issue #659 (Task 2.2) — per-turn reset of `artifact_ledger`. Caller
    /// (`handle_user_message` head) invokes this alongside the existing
    /// `turn_edited_relative_paths.clear()` / `turn_pre_tool_file_hashes.clear()`
    /// resets. Kept as its own helper so the test seam in
    /// `artifact_ledger_phase2_tests` can drive the reset without spinning
    /// up the full `handle_user_message` pipeline.
    ///
    /// Issue #659 PR-001: after the reset, stamp the per-turn observability
    /// log context (`session_id`, `turn_index`) so every subsequent
    /// `event_recorded` / `turn_summary` payload carries the join keys
    /// required by Section 7.1 of the design policy.
    ///
    /// Issue #659 PR-002 (Option B): callers pass `upcoming_turn_index`
    /// **explicitly** rather than letting this helper read
    /// `self.current_turn_index`. Decoupling production sequencing
    /// (`current_turn_index.saturating_add(1)` happens later in
    /// `handle_user_message`) from stamp timing fixes the off-by-one that
    /// caused `event_recorded` / `turn_summary` to carry a `turn_index`
    /// one less than the matching `agent.work_mode.classified` and other
    /// observability events on the same user turn. The parameter is
    /// `u32` because the ledger payload schema declares `turn_index` as
    /// `u32`; callers using `usize` should narrow via `try_into`, saturating
    /// to `u32::MAX` for pathological session lengths beyond 4B turns
    /// (losing granularity is acceptable; wrapping is not).
    pub(super) fn clear_per_turn_ledger_state_for_turn(&mut self, upcoming_turn_index: u32) {
        self.artifact_ledger.clear();
        let session_id = self.session_store.session_id().to_string();
        self.artifact_ledger.set_log_context(
            super::artifact_ledger::ArtifactLedgerLogContext::new(session_id, upcoming_turn_index),
        );
    }

    /// Issue #659 PR-002 — back-compat shim for unit tests that already
    /// position `self.current_turn_index` to the value they want stamped
    /// before calling the per-turn reset. Production code MUST use
    /// `clear_per_turn_ledger_state_for_turn(upcoming_turn_index)` so the
    /// upcoming index is explicit at the call site (decoupling production
    /// sequencing from stamp timing). This shim narrows `current_turn_index`
    /// the same way the original helper did.
    #[cfg(test)]
    pub(super) fn clear_per_turn_ledger_state(&mut self) {
        let upcoming = u32::try_from(self.current_turn_index).unwrap_or(u32::MAX);
        self.clear_per_turn_ledger_state_for_turn(upcoming);
    }

    /// Issue #659 (Task 2.2) — emit the end-of-turn
    /// `agent.artifact_ledger.turn_summary` event exactly once. Caller
    /// (`run_actor_loop` tail, next to `agent.milestone.turn_completed`)
    /// invokes this on every termination path so SafeStop / Done turns
    /// share a single observability surface. The event itself is masked by
    /// `log_llm_event` (final-defence `mask_payload_inplace`).
    pub(super) fn record_turn_end_artifact_ledger_summary(&self) {
        self.artifact_ledger.emit_turn_summary();
    }

    /// Issue #659 (Task 2.3) — admit an Existing-origin event into the
    /// ledger for a workspace-relative path that came from the
    /// task-contract candidate iteration / workspace scan SSOT
    /// (`existing_workspace_candidate_for_role_in_scope`). Baseline seeds
    /// are idempotent on `(origin, role, path)` so repeated contract
    /// evaluations during the same turn do NOT inflate the event count.
    pub(super) fn seed_artifact_ledger_existing(
        &mut self,
        relative_path: &str,
        role: super::task_contract::ArtifactRole,
        scope: &super::task_workspace_scope::TaskWorkspaceScope,
    ) {
        if is_ignored_workspace_display_path(relative_path) {
            return;
        }
        let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&self.work_root, scope);
        let _ = self
            .artifact_ledger
            .record_existing_event(&ctx, relative_path.to_string(), role);
    }

    /// Issue #659 (Task 2.4) — admit a Scaffold-origin event into the
    /// ledger for a workspace-relative path that came from the
    /// `scaffold_candidate_for_missing_role` / `scaffold_artifact_snapshots`
    /// SSOT. `post_scaffold_delta` should be the **caller-computed**
    /// signal (`repo_edit_has_post_scaffold_delta(...)`) so the seed is
    /// always consistent with the production no-op gate. Baseline seeds
    /// are idempotent on `(origin, role, path)`.
    pub(super) fn seed_artifact_ledger_scaffold(
        &mut self,
        relative_path: &str,
        role: super::task_contract::ArtifactRole,
        post_scaffold_delta: bool,
        scope: &super::task_workspace_scope::TaskWorkspaceScope,
    ) {
        if is_ignored_workspace_display_path(relative_path) {
            return;
        }
        let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&self.work_root, scope);
        let _ = self.artifact_ledger.record_scaffold_event(
            &ctx,
            relative_path.to_string(),
            role,
            post_scaffold_delta,
        );
    }

    /// Issue #659 (Task 2.5) — write-through adapter for a successful,
    /// non-no-op Write/Edit observation. The legacy
    /// `turn_edited_relative_paths` set is updated in the **same**
    /// instruction so adapter-period divergence (Task 2.7) is detectable
    /// at turn end. Callers MUST have already passed the no-op /
    /// scaffold-delta guards in `observe_evidence_from_repo_edit` so the
    /// seed never promotes a content-unchanged write to Owned.
    pub(super) fn seed_artifact_ledger_repo_edit(
        &mut self,
        relative_path: &str,
        role: super::task_contract::ArtifactRole,
        scope: &super::task_workspace_scope::TaskWorkspaceScope,
    ) {
        if is_ignored_workspace_display_path(relative_path) {
            return;
        }
        // Write-through adapter (divergence anchor): keep the legacy set
        // in lockstep with the ledger seed. Caller-facing decisions stay
        // on the legacy authority during the adapter period (Phase 6.1).
        self.turn_edited_relative_paths
            .insert(relative_path.to_string());
        let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&self.work_root, scope);
        let _ = self.artifact_ledger.record_repo_edit_event(
            &ctx,
            relative_path.to_string(),
            role,
            true,
        );
    }

    /// Issue #659 (Task 2.6) — record a verifier observation for each
    /// bound test path produced by a structured `VerifierCommand`. Legacy
    /// / unbound verifier paths (empty `bound_paths`) intentionally
    /// produce no observation so the projection-side absence-as-`NotRun`
    /// rule stays consistent.
    pub(super) fn seed_artifact_ledger_verifier_observation(
        &mut self,
        bound_paths: &[String],
        last_outcome: super::artifact_ledger::VerifierOutcome,
        scope: &super::task_workspace_scope::TaskWorkspaceScope,
    ) {
        if bound_paths.is_empty() {
            return;
        }
        let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&self.work_root, scope);
        for path in bound_paths {
            if is_ignored_workspace_display_path(path) {
                continue;
            }
            let _ = self.artifact_ledger.record_verifier_observation(
                &ctx,
                path,
                super::artifact_ledger::VerifierObservation {
                    argv_path_matched: true,
                    last_outcome,
                },
            );
        }
    }

    /// Issue #659 (Task 2.7) — dual-source divergence assertion. Adapter-
    /// period contract (Section 6.1 of design policy): the legacy
    /// `turn_edited_relative_paths` set remains the caller-facing
    /// authority while the ledger applies a **stricter** admission gate
    /// (`classify_ownership` re-validates workspace-relative / scope /
    /// control-char / role-mismatch). Legitimate divergence is therefore
    /// "ledger is a SUBSET of legacy": the ledger may reject a path the
    /// legacy set kept (e.g. an LLM-supplied path with embedded control
    /// chars that passed the legacy `workspace_relative_path_for_tool_arg`
    /// guard but failed `classify_ownership`).
    ///
    /// Hard fail (panic in debug builds) is reserved for the **inverse**
    /// case — the ledger admits a path the legacy set does NOT carry,
    /// which would be a write-through adapter bug since the seed helper
    /// inserts into both sources in the same instruction. In release
    /// builds the same condition emits a masked
    /// `agent.artifact_ledger.divergence_detected` event WITHOUT
    /// panicking, preserving the agent loop.
    pub(super) fn assert_dual_source_alignment_at_turn_end(&self) {
        let (legacy, ledger) = self.collect_dual_source_repo_edit_sets();
        let ledger_only: std::collections::BTreeSet<&String> = ledger.difference(&legacy).collect();
        #[cfg(debug_assertions)]
        {
            // CB-004: the panic message must not leak raw workspace-relative
            // paths (they may contain LLM-injected secrets). Mirror the
            // release-shape observability schema: counts + masked path
            // hashes only.
            assert!(
                ledger_only.is_empty(),
                "DR3 dual-source divergence: ledger admitted paths absent from legacy set \
                 (write-through adapter bug). legacy_count={legacy_count}, \
                 ledger_count={ledger_count}, ledger_only_count={only_count}, \
                 ledger_only_hashes={hashes:?}",
                legacy_count = legacy.len(),
                ledger_count = ledger.len(),
                only_count = ledger_only.len(),
                hashes = masked_path_hash_bounded_list(ledger_only.iter().map(|s| s.as_str())),
            );
            if legacy != ledger {
                // Stricter-ledger case: legitimate gating difference. Still
                // emit the observability event so dataset consumers can
                // count how often the ledger rejects what legacy accepts.
                self.emit_artifact_ledger_divergence_event(&legacy, &ledger);
            }
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = ledger_only;
            if legacy != ledger {
                self.emit_artifact_ledger_divergence_event(&legacy, &ledger);
            }
        }
    }

    /// Issue #659 (Task 2.7) — release-shape divergence helper. Always
    /// emits the event (no panic) when sources disagree. Exposed so the
    /// in-crate test suite can drive the release shape regardless of the
    /// `debug_assertions` cfg under which `cargo test` actually runs. The
    /// production code path goes through `assert_dual_source_alignment_at_turn_end`
    /// which compiles to a `debug_assert_eq!`-style panic in debug builds
    /// and dispatches to this emitter in release builds.
    #[allow(dead_code)]
    pub(super) fn emit_artifact_ledger_divergence_if_any(&self) {
        let (legacy, ledger) = self.collect_dual_source_repo_edit_sets();
        if legacy != ledger {
            self.emit_artifact_ledger_divergence_event(&legacy, &ledger);
        }
    }

    fn collect_dual_source_repo_edit_sets(
        &self,
    ) -> (
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    ) {
        let legacy: std::collections::BTreeSet<String> =
            self.turn_edited_relative_paths.iter().cloned().collect();
        // Issue #659 Task 3.1: route through the ledger's SSOT projection
        // method so the divergence helper and the Phase 3 internal-switch
        // helpers share a single source of truth (`repo_edit_projection_set`).
        let ledger = self.artifact_ledger.repo_edit_projection_set();
        (legacy, ledger)
    }

    #[allow(dead_code)] // invoked from release-build `assert_dual_source_alignment_at_turn_end` and the in-crate test seam.
    fn emit_artifact_ledger_divergence_event(
        &self,
        legacy: &std::collections::BTreeSet<String>,
        ledger: &std::collections::BTreeSet<String>,
    ) {
        let only_legacy: Vec<String> = legacy.difference(ledger).cloned().collect();
        let only_ledger: Vec<String> = ledger.difference(legacy).cloned().collect();
        // Issue #659 PR-001: emit bounded masked path-hash lists per
        // Section 7.1 of the design policy. The hash space matches
        // `event_recorded.path_hash` exactly (mask_secrets → DefaultHasher,
        // 16-char hex) so dataset consumers can join divergence rows back
        // to per-event rows. Hard cap at 16 entries each (raw path is
        // never emitted).
        let legacy_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(legacy.iter().map(String::as_str));
        let ledger_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(ledger.iter().map(String::as_str));
        log_llm_event(
            "agent.artifact_ledger.divergence_detected",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "turn_index": self.current_turn_index,
                "authority": "legacy",
                "legacy_count": legacy.len() as u32,
                "ledger_count": ledger.len() as u32,
                "only_legacy_count": only_legacy.len() as u32,
                "only_ledger_count": only_ledger.len() as u32,
                "legacy_path_hashes": legacy_path_hashes,
                "ledger_path_hashes": ledger_path_hashes,
            }),
        );
    }

    fn repo_edit_satisfies_current_artifact_target(
        &self,
        category: super::completion_evidence::RepoEditCategory,
        relative_path: &str,
    ) -> bool {
        super::task_contract::repo_edit_satisfies_artifact_recovery_target(
            category,
            relative_path,
            self.current_artifact_recovery_target.as_ref(),
        )
    }

    /// Issue #636: read a workspace-confined, cap-bounded excerpt of
    /// `relative_path` for behavior-coverage judgement.
    ///
    /// Path confinement (defense-in-depth: callers already pass a
    /// normalized relative path, but we re-resolve here):
    /// - `resolve_user_path(&self.work_root, relative_path)` + canonical
    ///   root + `strip_prefix` rejects absolute / `..` escape / out-of-
    ///   workspace symlinks / canonicalize failures / non-files.
    ///
    /// Cap-before-read:
    /// - `File::open` + `Read::take(MAX_ARTIFACT_EXCERPT_BYTES + 1)` so
    ///   we never read more than 8 KiB + 1 byte from disk. `std::fs::read`
    ///   / `read_to_string` are intentionally avoided.
    ///
    /// Content guards:
    /// - UTF-8 invalid → `None`. Embedded NUL → `None` (non-text).
    /// - When the cap boundary splits a multi-byte UTF-8 character we
    ///   truncate down to the last valid char boundary instead of giving
    ///   up (CB-001 / Issue #636 Phase 4).
    /// - `session::feedback::mask_secrets` then
    ///   `session::feedback::mask_header_family` stacked, matching the
    ///   redactor SSOT used elsewhere (DR4-002).
    /// - Post-masking re-truncation on a UTF-8 char boundary so masking
    ///   expansion can never blow past `MAX_ARTIFACT_EXCERPT_BYTES`.
    ///
    /// TOCTOU hardening: on Unix we open with `O_NOFOLLOW` so a symlink
    /// swap between the path confinement check and the open call cannot
    /// redirect us outside the workspace (CB-002 / Issue #636 Phase 4).
    pub(super) fn bounded_post_edit_excerpt(&self, relative_path: &str) -> Option<String> {
        use std::io::Read;
        let target = resolve_user_path(&self.work_root, relative_path).ok()?;
        let root = std::fs::canonicalize(&self.work_root).ok()?;
        if target.strip_prefix(&root).is_err() {
            return None;
        }
        if !target.is_file() {
            return None;
        }
        let cap = super::task_contract::MAX_ARTIFACT_EXCERPT_BYTES;
        let file = open_excerpt_file_nofollow(&target)?;
        let mut buf: Vec<u8> = Vec::with_capacity(cap + 1);
        file.take((cap as u64) + 1).read_to_end(&mut buf).ok()?;
        if buf.contains(&0u8) {
            return None;
        }
        let text = utf8_prefix_respecting_cap(&buf, cap)?.to_string();
        let masked = crate::session::feedback::mask_header_family(
            &crate::session::feedback::mask_secrets(&text),
        );
        Some(truncate_on_char_boundary(masked, cap))
    }

    /// Issue #659 (Task 3.2) test seam — re-exports
    /// `task_contract_artifact_states` to the sibling
    /// `artifact_ledger_phase3_tests` module **without** changing the
    /// caller-facing signature of the production helper (which stays
    /// private `fn`). The seam is `#[cfg(test)]` only and not compiled into
    /// the production binary.
    #[cfg(test)]
    pub(super) fn task_contract_artifact_states_for_test(
        &mut self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        self.task_contract_artifact_states(contract)
    }

    /// Issue #659 (Task 3.2) test seam — re-exports the legacy
    /// derivation so the equivalence test can read both projections
    /// without going through the public helper (which would emit a
    /// divergence event if the two derivations disagreed).
    #[cfg(test)]
    pub(super) fn task_contract_artifact_states_legacy_for_test(
        &mut self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        self.task_contract_artifact_states_legacy(contract)
    }

    /// Issue #659 (Task 3.2) test seam — re-exports the ledger
    /// projection helper. Mirrors `task_contract_artifact_states_legacy_for_test`.
    #[cfg(test)]
    pub(super) fn task_contract_artifact_states_from_ledger_for_test(
        &self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        self.task_contract_artifact_states_from_ledger(contract)
    }

    fn task_contract_artifact_states(
        &mut self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        // v0.4.8: make the ledger projection the production authority for
        // artifact state. The legacy derivation is still computed first because
        // it seeds Existing / Scaffold baseline events into the ledger, and it
        // remains useful as a shadow divergence signal. It must not remain the
        // returned value, otherwise current-task nested test edits can be
        // admitted by the ledger but still dropped by legacy verifier binding.
        let legacy_states = self.task_contract_artifact_states_legacy(contract);
        let ledger_states = self.task_contract_artifact_states_from_ledger(contract);
        if legacy_states != ledger_states {
            self.emit_artifact_state_projection_divergence(&legacy_states, &ledger_states);
        }
        ledger_states
    }

    /// Issue #659 (Task 3.2): the pre-Phase-3 implementation of
    /// `task_contract_artifact_states`. Kept intact (other than being renamed)
    /// so the caller-facing decision uses the legacy authority during the
    /// adapter period. Seed side-effects into the ledger remain here — the
    /// ledger reads the seeded events back in
    /// `task_contract_artifact_states_from_ledger`.
    fn task_contract_artifact_states_legacy(
        &mut self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        let scope = self.current_workspace_scope();
        let mut states = Vec::new();
        for role in &contract.required_artifacts {
            if let Some(path) =
                super::scaffold_pipeline::scaffold_candidate_for_missing_role(self, *role)
            {
                // Issue #659 (Task 2.4): seed the Scaffold-origin event
                // alongside the existing `ArtifactState::scaffold` push so
                // the ledger sees the same scaffold baseline. `post_scaffold_delta`
                // is the production no-op gate, kept consistent with
                // `observe_evidence_from_repo_edit`.
                let post_scaffold_delta =
                    super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(self, &path);
                self.seed_artifact_ledger_scaffold(&path, *role, post_scaffold_delta, &scope);
                states.push(super::task_contract::ArtifactState::scaffold(*role, path));
            }
            // Issue #646: an existing workspace artifact only enters as
            // `ExistsButUnverified` when ownership classification returns
            // `Owned`. Pre-existing nested-subtree artifacts the active
            // task did not produce stay out of the artifact-state vector
            // and therefore cannot satisfy `artifact_ready_for_verification`.
            if let Some(path) =
                existing_workspace_candidate_for_role_in_scope(&self.work_root, *role, &scope)
            {
                let scaffold_changed =
                    super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(self, &path);
                let edited_this_session = self.turn_edited_relative_paths.contains(&path);
                let ownership = super::artifact_ownership::classify_ownership(
                    super::artifact_ownership::OwnershipInputs {
                        work_root: &self.work_root,
                        relative_path: &path,
                        scope: &scope,
                        edited_this_session,
                        scaffold_changed,
                        verifier_passed_in_scope: false,
                        // Issue #661 (Task 3.1): Existing-origin classification
                        // path — preserves legacy semantics. The verifier-path
                        // SSOT switch lands in iteration-3.
                        nested_test_admission:
                            super::artifact_ownership::NestedTestAdmission::default(),
                    },
                );
                // Issue #659 (Task 2.3): seed the Existing-origin event
                // regardless of the `Owned` gate above. The ledger's own
                // admission re-runs `classify_ownership`, so role-mismatch
                // / OutOfScope downgrades happen at admission time;
                // baseline seed is idempotent across repeated evaluations.
                self.seed_artifact_ledger_existing(&path, *role, &scope);
                if matches!(
                    ownership,
                    super::artifact_ownership::ArtifactOwnership::Owned
                ) {
                    states.push(super::task_contract::ArtifactState::exists(*role, path));
                }
            }
        }
        for evidence in self.task_contract_evidence_set_this_turn.iter() {
            if let super::completion_evidence::CompletionEvidence::RepoEdit { category, .. } =
                evidence
                && let Some(role) = super::task_contract::role_from_repo_edit(*category)
            {
                states.push(super::task_contract::ArtifactState::changed(role));
            }
        }
        states
    }

    /// Issue #659 (Task 3.2): build `Vec<ArtifactState>` from the
    /// `ArtifactLedger` projection. The legacy helper above seeds Scaffold /
    /// Existing events as a side-effect of its own iteration; this helper
    /// reads those seeded events back, projecting:
    /// - `Scaffold`-origin events → `ArtifactState::scaffold(role, path)`
    /// - `Existing` or `RepoEdit` events with `Owned` ownership
    ///   → `ArtifactState::exists(role, path)`
    /// - `task_contract_evidence_set_this_turn` rows (RepoEdit category)
    ///   → `ArtifactState::changed(role)` (path-less, ledger does NOT carry
    ///   these rows because their `path: None` shape is rejected at
    ///   admission per DR2-002 of the design policy)
    ///
    /// Order follows `contract.required_artifacts` iteration so the legacy
    /// shape is matched byte-for-byte; `(role, path)` dedupe is applied to
    /// guard against the `Existing + RepoEdit` overlap that
    /// `seed_artifact_ledger_repo_edit` produces when an existing file is
    /// edited this turn.
    fn task_contract_artifact_states_from_ledger(
        &self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<super::task_contract::ArtifactState> {
        use super::artifact_ledger::ArtifactOrigin;
        use super::artifact_ownership::ArtifactOwnership;
        use std::collections::BTreeSet;

        let mut states = Vec::new();
        for role in &contract.required_artifacts {
            // Scaffold rows for this role.
            let mut scaffold_seen: BTreeSet<&str> = BTreeSet::new();
            for ev in self.artifact_ledger.events_for_role(*role) {
                if matches!(ev.origin, ArtifactOrigin::Scaffold)
                    && scaffold_seen.insert(ev.path.as_str())
                {
                    states.push(super::task_contract::ArtifactState::scaffold(
                        *role,
                        ev.path.clone(),
                    ));
                }
            }
            // Exists rows for this role (Existing or RepoEdit with Owned
            // ownership). Dedupe by path so the same workspace-relative
            // entry doesn't appear twice when an Existing baseline + a
            // RepoEdit observation collide.
            let mut exists_seen: BTreeSet<&str> = BTreeSet::new();
            for ev in self.artifact_ledger.events_for_role(*role) {
                if matches!(ev.origin, ArtifactOrigin::Scaffold) {
                    continue;
                }
                if !matches!(ev.ownership, ArtifactOwnership::Owned) {
                    continue;
                }
                if exists_seen.insert(ev.path.as_str()) {
                    states.push(super::task_contract::ArtifactState::exists(
                        *role,
                        ev.path.clone(),
                    ));
                }
            }
        }
        for evidence in self.task_contract_evidence_set_this_turn.iter() {
            if let super::completion_evidence::CompletionEvidence::RepoEdit { category, .. } =
                evidence
                && let Some(role) = super::task_contract::role_from_repo_edit(*category)
            {
                states.push(super::task_contract::ArtifactState::changed(role));
            }
        }
        states
    }

    /// Issue #659 (Task 3.2): masked observability emit when the legacy and
    /// ledger-projection derivations of `task_contract_artifact_states`
    /// disagree. v0.4.8 makes the ledger projection the production authority,
    /// so `authority="ledger"` is emitted for these projection-level rows.
    /// No raw paths are emitted; only role / kind counts.
    ///
    /// Issue #659 PR-001: also emit bounded masked path-hash lists (max
    /// 16 entries each, deterministic order via BTreeSet) so dataset
    /// consumers can join divergence rows back to per-event rows. Rows
    /// without a path (`ArtifactState::changed(role)`) are skipped per
    /// DR2-002 — only path-bearing states contribute.
    fn emit_artifact_state_projection_divergence(
        &self,
        legacy: &[super::task_contract::ArtifactState],
        ledger: &[super::task_contract::ArtifactState],
    ) {
        let legacy_paths: std::collections::BTreeSet<&str> =
            legacy.iter().filter_map(|s| s.path.as_deref()).collect();
        let ledger_paths: std::collections::BTreeSet<&str> =
            ledger.iter().filter_map(|s| s.path.as_deref()).collect();
        let legacy_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(legacy_paths.iter().copied());
        let ledger_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(ledger_paths.iter().copied());
        log_llm_event(
            "agent.artifact_ledger.divergence_detected",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "turn_index": self.current_turn_index,
                "authority": "ledger",
                "projection": "task_contract_artifact_states",
                "legacy_count": legacy.len() as u32,
                "ledger_count": ledger.len() as u32,
                "legacy_path_hashes": legacy_path_hashes,
                "ledger_path_hashes": ledger_path_hashes,
            }),
        );
    }

    /// Issue #646: build the active `TaskWorkspaceScope` for the current
    /// task. Pure projection of `work_root` + the active user request; no
    /// filesystem mutation. Called from `task_contract_artifact_states` and
    /// `task_contract_recovery_target`; not cached because the bounded
    /// shallow read of `work_root` is cheap and the scope is recomputed
    /// only a handful of times per turn.
    pub(super) fn current_workspace_scope(
        &self,
    ) -> super::task_workspace_scope::TaskWorkspaceScope {
        let request = self.active_request_text().unwrap_or_default();
        super::task_workspace_scope::TaskWorkspaceScope::detect(&self.work_root, &request)
    }

    /// Issue #651 Phase 5: produce the SSOT `owned_test_artifacts` slice
    /// for the given `TaskContract`. Always classifies via
    /// `artifact_ownership::owned_test_artifacts`, with the closure
    /// predicates pointing back at `turn_edited_relative_paths` /
    /// `repo_edit_has_post_scaffold_delta` so the planner-side ownership
    /// signals stay consistent across all call sites (DR1-008).
    ///
    /// Issue #659 (Task 3.3): internal implementation now reads the
    /// `ArtifactLedger` projection
    /// (`artifact_ledger::owned_test_artifacts(ArtifactRole::Test)`) in
    /// parallel with the legacy `artifact_ownership::owned_test_artifacts`
    /// derivation. The two should agree by construction (write-through
    /// adapter from Task 2.5 keeps both sources in lockstep); when they
    /// diverge the adapter-period contract (Phase 6.1) preserves legacy
    /// authority — we emit a masked
    /// `agent.artifact_ledger.divergence_detected` event and return the
    /// legacy slice. The caller signature (`&mut self`,
    /// `&TaskContract`, `Vec<String>`) is unchanged so every existing
    /// consumer (`success.rs`, `turn.rs::run_task_contract_verifier_once`,
    /// `task_contract_recovery_action`) remains source-compatible.
    pub(super) fn owned_test_artifacts_for_verifier(
        &mut self,
        contract: &super::task_contract::TaskContract,
    ) -> Vec<String> {
        let scope = self.current_workspace_scope();
        // task_contract_artifact_states has the side effect of seeding the
        // ledger with Existing / Scaffold baseline events. We MUST call it
        // before reading the ledger projection so the Phase 3 path sees the
        // same baseline the legacy derivation sees.
        let states = self.task_contract_artifact_states(contract);
        let legacy = super::artifact_ownership::owned_test_artifacts(
            &states,
            &self.work_root,
            &scope,
            &|path| self.turn_edited_relative_paths.contains(path),
            &|path| super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(self, path),
        );
        let ledger = self
            .artifact_ledger
            .owned_test_artifacts(super::task_contract::ArtifactRole::Test);
        if legacy != ledger {
            self.emit_owned_test_artifacts_projection_divergence(&legacy, &ledger);
        }
        ledger
    }

    /// Issue #659 (Task 3.3): masked observability emit when the legacy and
    /// ledger-projection derivations of `owned_test_artifacts_for_verifier`
    /// disagree. v0.4.8 makes the ledger projection the production authority,
    /// so `authority="ledger"` is emitted for these projection-level rows.
    /// No raw paths are emitted; only role / count metadata.
    ///
    /// Issue #659 PR-001: also emit bounded masked path-hash lists (max
    /// 16 entries each, deterministic order via BTreeSet) so dataset
    /// consumers can join divergence rows back to per-event rows.
    fn emit_owned_test_artifacts_projection_divergence(
        &self,
        legacy: &[String],
        ledger: &[String],
    ) {
        let legacy_set: std::collections::BTreeSet<&str> =
            legacy.iter().map(String::as_str).collect();
        let ledger_set: std::collections::BTreeSet<&str> =
            ledger.iter().map(String::as_str).collect();
        let legacy_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(legacy_set.iter().copied());
        let ledger_path_hashes =
            super::artifact_ledger::bounded_masked_path_hashes(ledger_set.iter().copied());
        log_llm_event(
            "agent.artifact_ledger.divergence_detected",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "turn_index": self.current_turn_index,
                "authority": "ledger",
                "projection": "owned_test_artifacts_for_verifier",
                "legacy_count": legacy.len() as u32,
                "ledger_count": ledger.len() as u32,
                "legacy_path_hashes": legacy_path_hashes,
                "ledger_path_hashes": ledger_path_hashes,
            }),
        );
    }

    fn task_contract_repair_state(
        &self,
        repair_edit_count: Option<usize>,
        repo_edit_calls_made_this_turn: usize,
    ) -> super::task_contract::VerifierRepairState {
        super::repair_job::task_contract_repair_state_from_job(
            self.task_contract_verifier_repair_pending,
            self.repair_job.as_ref(),
            repair_edit_count,
            repo_edit_calls_made_this_turn,
        )
    }

    pub(super) fn task_contract_recovery_action(
        &mut self,
        contract: &super::task_contract::TaskContract,
        repair_edit_count: Option<usize>,
        repo_edit_calls_made_this_turn: usize,
    ) -> super::task_contract::ArtifactRecoveryAction {
        let verifier_repair_ready_to_verify = self.task_contract_verifier_repair_pending
            && (repair_edit_count
                .is_some_and(|edit_count| repo_edit_calls_made_this_turn > edit_count)
                || self.repair_job.as_ref().is_some_and(|job| {
                    matches!(
                        job.next_action(),
                        super::repair_job::RepairNextAction::RerunVerifier
                            | super::repair_job::RepairNextAction::VerifiedDone
                    )
                }));
        if verifier_repair_ready_to_verify {
            return super::task_contract::ArtifactRecoveryAction::RunVerifier;
        }
        let artifacts = self.task_contract_artifact_states(contract);
        let repair_state =
            self.task_contract_repair_state(repair_edit_count, repo_edit_calls_made_this_turn);
        let missing_verifier_suppress_retry = self
            .missing_verifier_job
            .as_ref()
            .is_some_and(|job| job.should_suppress_verifier_retry());
        // Issue #651 Phase 5: feed the SSOT `owned_test_artifacts` slice
        // into the planner so the SafeStop gate (test_execution_required
        // && owned_test_artifacts.is_empty()) can fire.
        let owned_test_artifacts = self.owned_test_artifacts_for_verifier(contract);
        let action = super::task_contract::plan_artifact_recovery(
            super::task_contract::ArtifactRecoveryInputs {
                contract,
                evidence: &self.task_contract_evidence_set_this_turn,
                artifacts: &artifacts,
                repair_state: &repair_state,
                artifact_excerpts: &self.task_contract_excerpts,
                missing_verifier_suppress_retry,
                owned_test_artifacts: &owned_test_artifacts,
            },
        );
        if matches!(
            action,
            super::task_contract::ArtifactRecoveryAction::Continue { .. }
        ) {
            let request = self.active_request_text().unwrap_or_default();
            let scope = self.current_workspace_scope();
            let probe = super::project_probe::probe_completion(
                &self.work_root,
                &request,
                contract,
                &scope,
                &self.turn_edited_relative_paths,
            );
            match probe {
                super::project_probe::CompletionProbeDecision::RunVerifier {
                    reason,
                    project_unit,
                } => {
                    log_llm_event(
                        "agent.completion_probe.decision",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "turn_index": self.current_turn_index,
                            "decision": "run_verifier",
                            "reason": reason,
                            "project_unit": project_unit.summary(),
                        }),
                    );
                    return super::task_contract::ArtifactRecoveryAction::RunVerifier;
                }
                super::project_probe::CompletionProbeDecision::RejectStackMismatch { reason } => {
                    log_llm_event(
                        "agent.completion_probe.decision",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "turn_index": self.current_turn_index,
                            "decision": "reject_stack_mismatch",
                            "reason": reason,
                        }),
                    );
                }
                super::project_probe::CompletionProbeDecision::KeepArtifactFlow => {}
            }
        }
        action
    }

    pub(super) fn task_contract_recovery_target(
        &self,
        decision: &super::task_contract::CompletionDecision,
    ) -> Option<super::task_contract::RecoveryTargetHint> {
        let super::task_contract::CompletionDecision::Continue { missing } = decision else {
            return None;
        };
        let role = missing.first().copied()?;
        if let Some(path) =
            super::scaffold_pipeline::scaffold_candidate_for_missing_role(self, role)
        {
            return Some(super::task_contract::RecoveryTargetHint {
                role,
                path,
                reason: "bootstrap scaffold artifact for the missing role is still unchanged"
                    .to_string(),
            });
        }
        // Issue #646 (D1): two-step gate — first scope, then full ownership
        // classifier. A scope-internal but non-`Owned` artifact (e.g. an
        // unchanged scaffold body, a CandidateOnly README the user never
        // mentioned) MUST NOT be surfaced as a recovery target either. The
        // ownership signal — edit / scaffold delta / explicit scope mention
        // — is the same one the planner uses upstream.
        let scope = self.current_workspace_scope();
        if let Some(path) =
            existing_workspace_candidate_for_role_in_scope(&self.work_root, role, &scope)
        {
            let scaffold_changed =
                super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(self, &path);
            let edited_this_session = self.turn_edited_relative_paths.contains(&path);
            let ownership = super::artifact_ownership::classify_ownership(
                super::artifact_ownership::OwnershipInputs {
                    work_root: &self.work_root,
                    relative_path: &path,
                    scope: &scope,
                    edited_this_session,
                    scaffold_changed,
                    verifier_passed_in_scope: false,
                    // Keep target selection conservative. The ledger/verifier
                    // projection is the only place that broadens nested-test
                    // admission for current-task evidence; generic recovery must
                    // not claim pre-existing nested tests without evidence.
                    nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(
                    ),
                },
            );
            if matches!(
                ownership,
                super::artifact_ownership::ArtifactOwnership::Owned
            ) {
                return Some(super::task_contract::RecoveryTargetHint {
                    role,
                    path,
                    reason: "existing workspace artifact matches the missing role".to_string(),
                });
            }
        }
        if let Some(path) = self.synthesized_missing_implementation_target_path(role) {
            return Some(super::task_contract::RecoveryTargetHint {
                role,
                path,
                reason: "no existing implementation artifact for the active request; create a conventional implementation file".to_string(),
            });
        }
        None
    }

    fn synthesized_missing_implementation_target_path(
        &self,
        role: super::task_contract::ArtifactRole,
    ) -> Option<String> {
        let request = self.active_request_text()?;
        synthesized_missing_implementation_target_path_for_request(role, &request)
    }

    pub(super) fn set_artifact_recovery_target_for_decision(
        &mut self,
        decision: &super::task_contract::CompletionDecision,
        attempt: usize,
    ) -> Option<super::task_contract::RecoveryTargetHint> {
        let hint = self.task_contract_recovery_target(decision)?;
        self.set_artifact_recovery_target_from_hint(hint, attempt)
    }

    pub(super) fn set_artifact_recovery_target_for_action(
        &mut self,
        action: &super::task_contract::ArtifactRecoveryAction,
        attempt: usize,
    ) -> Option<super::task_contract::RecoveryTargetHint> {
        let hint = match action {
            super::task_contract::ArtifactRecoveryAction::Continue { target_hint, .. }
            | super::task_contract::ArtifactRecoveryAction::RepairArtifact { target_hint } => {
                target_hint.clone()?
            }
            _ => return None,
        };
        self.set_artifact_recovery_target_from_hint(hint, attempt)
    }

    fn align_recovery_target_hint_to_request(
        &self,
        mut hint: super::task_contract::RecoveryTargetHint,
    ) -> super::task_contract::RecoveryTargetHint {
        if hint.role != super::task_contract::ArtifactRole::Test {
            return hint;
        }
        let Some(request) = self.active_request_text() else {
            return hint;
        };
        let Some((target_path, stack_label)) =
            synthesized_missing_test_target_path_for_request(&request)
        else {
            return hint;
        };
        if hint.path == target_path
            || test_target_path_compatible_with_request(&hint.path, &request)
        {
            return hint;
        }
        hint.path = target_path.to_string();
        hint.reason = format!(
            "synthesized test artifact aligned with requested {stack_label} project family"
        );
        hint
    }

    pub(super) fn set_artifact_recovery_target_from_hint(
        &mut self,
        hint: super::task_contract::RecoveryTargetHint,
        attempt: usize,
    ) -> Option<super::task_contract::RecoveryTargetHint> {
        let hint = self.align_recovery_target_hint_to_request(hint);
        // Issue #652 PR-001: the `ArtifactCompletionJob` is the SSOT for
        // target + role-specific retry budget. We must NOT update
        // `current_artifact_recovery_target` before the job has been
        // validated and installed — otherwise a validation failure would
        // leave the projection set with no job attached, and
        // `EffectiveToolPolicy::artifact_directed` would grant write
        // access for a target with no role-specific budget. Ordering:
        //   1. attempt to install / refresh the job for the hint
        //   2. on success → commit the projection (atomic SWAP from any
        //      prior state)
        //   3. on failure (only possible for Test role) → clear BOTH
        //      the projection and the job so no stale slot remains.
        let install = self.maybe_install_artifact_completion_job_for_hint(&hint);
        match install {
            JobInstallOutcome::InstalledOrSkipped => {
                let target = super::task_contract::RecoveryTarget::from_hint(hint.clone(), attempt);
                let changed = self.current_artifact_recovery_target.as_ref() != Some(&target);
                if changed {
                    log_llm_event(
                        "agent.artifact_recovery_target.selected",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "turn_index": self.current_turn_index,
                            "role": target.role.label(),
                            "path": target.path,
                            "reason": target.reason,
                            "attempt": target.attempt,
                        }),
                    );
                }
                self.current_artifact_recovery_target = Some(target);
                Some(hint)
            }
            JobInstallOutcome::ValidationFailed => {
                // PR-001 atomic clear: the new Test hint failed
                // `ArtifactCompletionJob::new` validation. Drop the prior
                // projection too — otherwise the artifact-directed
                // policy would keep granting write permission for a
                // target with no attached role-specific budget.
                if self.current_artifact_recovery_target.take().is_some() {
                    log_llm_event(
                        "agent.artifact_recovery_target.cleared",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "turn_index": self.current_turn_index,
                            "reason": "artifact_completion_job_validation_failed",
                        }),
                    );
                }
                None
            }
        }
    }

    pub(super) fn clear_artifact_recovery_target(&mut self, reason: &'static str) {
        if let Some(target) = self.current_artifact_recovery_target.take() {
            log_llm_event(
                "agent.artifact_recovery_target.cleared",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "turn_index": self.current_turn_index,
                    "role": target.role.label(),
                    "path": target.path,
                    "reason": reason,
                }),
            );
        }
        // Issue #652: drop the SSOT artifact-completion job along with the
        // projection so a subsequent role change cannot reuse stale budget.
        self.artifact_completion_job = None;
    }

    /// Issue #652: install (or refresh) the `ArtifactCompletionJob` for a
    /// fresh `RecoveryTargetHint` when the target role is `Test` (the only
    /// role for which `RequiredBehaviorContract::requires_test_execution()`
    /// currently fires). Refresh is identity-based on `(role, target_path)`
    /// — re-pointing at the same path leaves the existing job (and its
    /// retry budget) intact so wrong-target attempts already recorded keep
    /// counting.
    ///
    /// Returns:
    /// - `InstalledOrSkipped` when the job was installed, the existing
    ///   identity-refresh was kept, or the hint role does not require a
    ///   job (non-Test). The caller may commit the projection.
    /// - `ValidationFailed` ONLY when a Test-role hint did not pass
    ///   `ArtifactCompletionJob::new` validation. The caller MUST clear
    ///   `current_artifact_recovery_target` as well (PR-001 atomic clear)
    ///   so no stale projection survives.
    pub(super) fn maybe_install_artifact_completion_job_for_hint(
        &mut self,
        hint: &super::task_contract::RecoveryTargetHint,
    ) -> JobInstallOutcome {
        // Issue #663 (Phase C / AD5): the legacy `hint.role != Test`
        // early-return is removed — all required roles (Implementation /
        // Test / UsageDocs / Setup) install/refresh an
        // `ArtifactCompletionJob` so the role-specific budget and
        // attempt-history apply uniformly. The previous "drop the stale
        // Test job" behaviour for non-Test roles is preserved below by
        // the identity-refresh + atomic-clear ordering, which now applies
        // to every role.
        let trimmed = hint.path.trim();
        // Identity refresh: same role + same target → keep the existing
        // job (and its retry budget) intact. Same-target hints must NOT
        // reset the budget so accumulated wrong-target attempts keep
        // counting toward exhaustion.
        if let Some(job) = self.artifact_completion_job.as_ref()
            && job.role() == hint.role
            && job.target_path() == trimmed
        {
            return JobInstallOutcome::InstalledOrSkipped;
        }
        // CB-005: when the new hint points at a *different* target than
        // the current job, the prior job's expected target is now
        // stale. Drop it BEFORE attempting to validate the new hint so
        // a validation failure cannot leave the agent with a stale job
        // whose budget belongs to an old `current_artifact_recovery_target`.
        // The atomic ordering is: clear → validate-and-install. If the
        // new hint validates, we install it (atomic SWAP). PR-001: if
        // it does NOT validate, the caller MUST also clear
        // `current_artifact_recovery_target` so no stale projection
        // remains (signalled by `JobInstallOutcome::ValidationFailed`).
        self.artifact_completion_job = None;
        let scope = self.current_workspace_scope();
        match super::artifact_completion_job::ArtifactCompletionJob::new(
            &self.work_root,
            &scope,
            hint.clone(),
            self.turn_edited_relative_paths.contains(trimmed),
            false,
        ) {
            Ok(job) => {
                self.artifact_completion_job = Some(job);
                JobInstallOutcome::InstalledOrSkipped
            }
            Err(_) => {
                // Validation failure for a Test-role hint: signal the
                // caller to drop the projection too (PR-001 SSOT).
                JobInstallOutcome::ValidationFailed
            }
        }
    }

    /// Issue #652: emit a turn-local `artifact_completion_failed` diagnostic
    /// when the active job has exhausted its budget. Sinks are limited to:
    /// system note, working-memory error, and an agent-controlled failure
    /// JSON event (design judgement #7). The payload is rendered from the
    /// sanitized `failure_snapshot()` (mask + cap + control-char neutralize
    /// already applied) and passed through `mask_payload_inplace` as the
    /// defensive final-defence line.
    fn maybe_emit_artifact_completion_failed_diagnostic(&mut self) -> bool {
        let snapshot = match self.artifact_completion_job.as_ref() {
            Some(job) => match job.failure_snapshot() {
                Some(s) => s,
                None => return false,
            },
            None => return false,
        };
        // CB-002 / CB2-003: once the current turn has already emitted the
        // exhaustion diagnostic, subsequent identical-kind attempts (which
        // are no-ops on `record_attempt`) must NOT re-fire the system note
        // / log / working-memory tuple. The dedup is gated on a
        // **turn-local** flag (`artifact_completion_failed_diagnostic_emitted_this_turn`),
        // reset at every `handle_user_message` head.
        //
        // The previous implementation matched against
        // `working_memory.unresolved_errors` for the
        // `artifact_completion_failed role=<role>` prefix, but
        // `working_memory` is session state — `handle_user_message` does
        // NOT clear it. So a residual error from the prior turn would
        // suppress the very first emission of the current turn. CB2-003
        // moves the dedup to a per-turn boolean instead.
        if self.artifact_completion_failed_diagnostic_emitted_this_turn {
            return false;
        }
        let role_label = snapshot.current_role.label();
        let expected_target = snapshot.expected_target.clone();
        let actions_preview = snapshot
            .actual_actions
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        // Sink 1: system note — sanitized snapshot fields only.
        self.push_system_note(format!(
            "[Artifact Completion Failed] Missing role: {role_label}. Expected target: {expected_target}. Recent actions: {actions_preview}. Retry budget exhausted."
        ));
        // Sink 2: working-memory error.
        self.session.working_memory.note_error(format!(
            "artifact_completion_failed role={role_label} target={expected_target}"
        ));
        // Sink 3: agent-controlled failure result emitted as a JSON event
        // run through `mask_payload_inplace` as a defensive final pass.
        let mut payload = serde_json::json!({
            "session_id": self.session_store.session_id(),
            "turn_index": self.current_turn_index,
            "role": role_label,
            "expected_target": expected_target,
            "actual_actions": snapshot.actual_actions,
            "attempts": snapshot.attempts.len(),
        });
        crate::logging::mask_payload_inplace(&mut payload);
        log_llm_event("agent.artifact_completion_failed", payload);
        // CB2-003: flip the turn-local dedup flag AFTER the three sinks
        // have actually run, so a within-turn second call short-circuits
        // at the top guard above. Cross-turn dedup is handled by the
        // per-turn reset in `handle_user_message`, which restores this
        // flag to `false` at every fresh user turn.
        self.artifact_completion_failed_diagnostic_emitted_this_turn = true;
        true
    }

    fn artifact_recovery_target_path(&self) -> Option<PathBuf> {
        // Issue #652 PR-001 SSOT: when an `ArtifactCompletionJob` is
        // active, read the target straight from the job — that is the
        // single source of truth for the in-flight artifact-completion
        // task this turn. `current_artifact_recovery_target` is kept in
        // sync at `set_artifact_recovery_target_from_hint` (atomic
        // install + commit), but reading the job first makes the SSOT
        // invariant explicit and means that any future drift between
        // the two surfaces still resolves to the job's authoritative
        // path. For non-Test roles (no attached job today) we still
        // fall through to the legacy projection so the existing
        // artifact-directed recovery semantics for Implementation /
        // UsageDocs / Setup roles continue to work.
        let path_str = if let Some(job) = self.artifact_completion_job.as_ref() {
            job.target_path().to_string()
        } else {
            self.current_artifact_recovery_target.as_ref()?.path.clone()
        };
        resolve_user_path(&self.work_root, &path_str).ok()
    }

    fn answer_only_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        if !self.answer_only_mode_active() {
            return None;
        }
        if matches!(name, "Read" | "Glob" | "Grep") {
            return None;
        }
        if name == "Bash"
            && self.script_execution_requested()
            && arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(answer_only_script_command_allowed)
        {
            return None;
        }
        Some(format!(
            "Error: answer-only mode is read-only. Use Read, Glob, or Grep if inspection is needed, and only run Bash for an explicitly requested local script or read-only command. Blocked tool: {name}."
        ))
    }

    pub(super) fn answer_only_mode_active(&self) -> bool {
        // Issue #576 / DR3-001: tool policy must honour the second-pass-
        // corrected `session.mode_state.work_mode` as the single source of
        // truth. The previous OR with `infer_work_mode_from_text(active_request_text())`
        // bypassed the second-pass result whenever the lexical pre-classifier
        // still inferred `AnswerOnly`, defeating the whole point of this Issue.
        self.session.mode_state.work_mode == WorkMode::AnswerOnly
    }

    fn script_execution_requested(&self) -> bool {
        self.active_request_text()
            .as_deref()
            .is_some_and(request_explicitly_requests_script_execution)
    }

    fn effective_tool_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        let effective_tool_policy = self.effective_tool_policy();
        let scope = if self.missing_verifier_job.is_some() {
            Some(self.current_workspace_scope())
        } else {
            None
        };
        effective_tool_policy_error_for_call_with_scope(
            &effective_tool_policy,
            name,
            arguments,
            &self.work_root,
            scope.as_ref(),
        )
    }

    fn workspace_appears_empty(&self) -> bool {
        workspace_appears_empty(&self.work_root)
    }

    pub(super) fn active_task_expects_repo_change(&self) -> bool {
        self.session.mode_state.mode == ExecutionMode::Act
            && self.session.mode_state.policy().repo_edit_required
            && self.active_request_text().as_deref().is_some_and(|task| {
                recovery::classify_action_expectation(task, self.session.mode_state.mode)
                    == recovery::ActionExpectation::RepoChange
            })
    }

    pub(super) fn active_request_text(&self) -> Option<String> {
        repo_change_request_text(
            self.session.working_memory.active_task.as_deref(),
            &self.session.messages,
        )
    }

    pub(super) fn current_request_needs_playable_ui_quality_gate(&self) -> bool {
        self.session.mode_state.mode == ExecutionMode::Act
            && self.session.mode_state.policy().quality_gate_enabled
            && !self.unsupported_ui_framework_context()
            && self
                .active_request_text()
                .as_deref()
                .is_some_and(request_needs_playable_ui_quality_gate)
    }

    pub(super) fn accepted_repo_change_quality_issue(
        &mut self,
    ) -> Option<(String, String, String)> {
        if !self.session.mode_state.policy().quality_gate_enabled {
            return None;
        }
        if self.unsupported_ui_framework_context() {
            return None;
        }
        let request = self.active_request_text()?;
        let request = request.trim().to_string();
        if !request_needs_playable_ui_quality_gate(&request) {
            return None;
        }
        let target = first_existing_impl_target(&self.work_root)?;
        let content = std::fs::read_to_string(&target).ok()?;
        // Issue #580: route through the second-pass adapter so borderline UI
        // verdicts can be confirmed/overridden by the sidecar LLM.
        let issue = self.implementation_quality_issue_with_confirm(&request, &content)?;
        let relative = target
            .strip_prefix(&self.work_root)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        Some((request, relative, issue))
    }

    pub(super) fn accepted_repo_change_polish_target(&mut self) -> Option<(String, String)> {
        if !self.session.mode_state.policy().allow_polish_fallback {
            return None;
        }
        if self.unsupported_ui_framework_context() {
            return None;
        }
        let request = self.active_request_text()?;
        let request = request.trim().to_string();
        if !request_allows_fast_polish_fallback(&request) {
            return None;
        }
        let target = first_existing_impl_target(&self.work_root)?;
        let content = std::fs::read_to_string(&target).ok()?;
        // Issue #580: a second-pass `interactive=false` verdict surfaces here
        // as `Some(...)` which correctly suppresses the polish action
        // (treating the file as a quality issue rather than polishing static
        // code).
        if self
            .implementation_quality_issue_with_confirm(&request, &content)
            .is_some()
        {
            return None;
        }
        deterministic::playable_ui_polish(&request, &target, &content)?;
        let relative = target
            .strip_prefix(&self.work_root)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        Some((request, relative))
    }

    fn unsupported_ui_framework_context(&self) -> bool {
        self.active_request_text()
            .as_deref()
            .is_some_and(request_mentions_unsupported_ui_framework)
            || workspace_has_unsupported_ui_framework(&self.work_root)
    }

    pub(super) fn active_python_request_requires_tests(&self) -> bool {
        self.session.mode_state.work_mode == WorkMode::Python
            && self
                .active_request_text()
                .as_deref()
                .is_some_and(request_explicitly_requires_tests)
    }

    pub(super) fn python_verifier_available_for_requested_tests(&self) -> bool {
        AutoTestRunner::detect(&self.work_root, &self.session.working_memory.touched_files)
            .is_some_and(|plan| plan.auto_test_kind() == AutoTestKind::Test)
    }

    pub(super) fn python_test_artifact_exists(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.work_root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return false;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            (name.starts_with("test_") && name.ends_with(".py"))
                || name.ends_with("_test.py")
                || name == "tests.py"
        })
    }

    fn refresh_working_memory(&mut self) {
        let constraints = self
            .current_plan_contents()
            .ok()
            .flatten()
            .map(|contents| lifecycle::extract_plan_constraints(&contents))
            .unwrap_or_default();
        self.session.working_memory.replace_constraints(constraints);
    }

    fn working_memory_message(&mut self) -> Option<ConversationMessage> {
        if !self.session.mode_state.policy().include_working_memory {
            return None;
        }
        self.refresh_working_memory();

        // Issue #453: build the per-prompt precaution slice from
        // (active_precautions × mode × touched_files × last_feedback.suspected_files)
        // before handing it to the renderer. The Reminder Sidecar path
        // (handle_user_message → format_for_prompt() wrapper) keeps using the
        // full active-only list (design judgment #5).
        let suspected_owned: Option<Vec<PathBuf>> = self
            .session
            .last_feedback
            .as_ref()
            .map(|f| f.suspected_files.clone());
        let precautions_for_prompt = select_precautions_for_prompt(
            &self.session.working_memory.active_precautions,
            self.session.mode_state.mode,
            &self.session.working_memory.touched_files,
            suspected_owned.as_deref(),
        );
        self.session
            .working_memory
            .format_for_prompt_with_precautions(&precautions_for_prompt)
            .map(ConversationMessage::system)
    }

    pub(super) fn answer_only_fallback_response(&self) -> String {
        let request = self.active_request_text().unwrap_or_default();
        let lower = request.to_ascii_lowercase();
        if request_explicitly_requests_script_execution(&request)
            && let Some(output) = latest_tool_result_since_last_user(&self.session.messages, "Bash")
        {
            return answer_only_script_execution_fallback_response(output);
        }
        if lower.contains("modepolicy") || lower.contains("構造化状態") {
            return "ファイルは変更せず、読み取り専用で整理します。\n\n利点:\n- モード判断を会話履歴から分離できるため、古い発話や回復プロンプトに引きずられにくい。\n- `repo_edit_required` や fallback 許可などを明示的な実行ポリシーとして扱えるため、ツール制御と品質ゲートを安定させやすい。\n- セッション保存や compaction 後も、必要な状態だけを小さく復元できる。\n\nリスク:\n- 状態更新の境界が曖昧だと、ユーザーの最新意図と ModePolicy がずれる。\n- ポリシーが強すぎると、読み取り専用のスクリプト実行など正当な作業まで止める。\n- LLM の自然言語判断と構造化状態の差分を観測できないと、誤分類の原因調査が難しい。\n\n方向性としては、ModePolicy は構造化状態で保持し、最新ユーザー要求から毎ターン再評価できるようにするのが妥当です。会話履歴へ埋め込むのは補助説明に留め、実際のツール許可と品質条件は構造化フィールドを正とするのが安定します。".to_string();
        }
        if lower.contains("rust") && lower.contains("cli") {
            return "ファイルは変更せず、Rust CLI 化の構成案だけを整理します。\n\n- `Cargo.toml`: crate 名、依存、bin 設定を管理する。\n- `src/main.rs`: 引数解析と終了コード制御だけを置く。\n- `src/cli.rs`: CLI オプション、help、入力検証をまとめる。\n- `src/lib.rs`: 実処理をライブラリ化し、CLI 以外からもテスト可能にする。\n- `tests/cli.rs`: 代表コマンド、異常入力、終了コードを E2E 寄りに検証する。\n- `README.md`: インストール、実行例、検証コマンド、制約を記載する。\n\n方針としては、CLI 表層とドメイン処理を分離し、`cargo test` でロジック、必要なら `assert_cmd` 系でコマンド挙動を確認するのが扱いやすいです。".to_string();
        }
        if lower.contains("readme")
            && (request.contains("要約") || lower.contains("summarize"))
            && let Ok(readme) = std::fs::read_to_string(self.work_root.join("README.md"))
        {
            let summary = readme
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            return format!(
                "README の要約: {summary}\n\n設計上の課題: README から確認できる情報は概要レベルに限られており、内部構成、実行手順、検証方法、制約、fallback や session 管理の責務分担が文書化されていません。そのため、初見の開発者が変更範囲や品質確認方法を判断しにくい状態です。ファイルは変更していません。"
            );
        }
        "ファイルは変更せず、読み取り専用の回答として整理します。目的、前提、推奨構成、検証方法、残リスクを分け、実装や編集が必要な場合だけ次のターンで明示的に依頼してください。".to_string()
    }

    fn repo_context_message(&mut self) -> Option<ConversationMessage> {
        if !self.session.mode_state.policy().allow_repo_context {
            return None;
        }
        self.refresh_working_memory();
        let task = self.session.working_memory.active_task.clone()?;

        // Issue #469: cache key is widened to include graph ranking inputs.
        let suspected_files: Vec<PathBuf> = self
            .session
            .last_feedback
            .as_ref()
            .map(|f| f.suspected_files.clone())
            .unwrap_or_default();
        let suspected_strings: Vec<String> = suspected_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let changed_files: Vec<String> = self.session.touched_files_at_turn_start.clone();
        let last_feedback_kind: Option<String> = self
            .session
            .last_feedback
            .as_ref()
            .map(|f| format!("{:?}", f.kind));
        let repo_graph_present = self.repo_graph.is_some();
        let suspected_fp = super::fingerprint_paths(&suspected_strings);
        let touched_fp = super::fingerprint_paths(&changed_files);

        if let Some(cache) = &self.repo_context_cache
            && cache.task == task
            && cache.work_root == self.work_root
            && cache.repo_graph_present == repo_graph_present
            && cache.last_feedback_kind == last_feedback_kind
            && cache.suspected_files_fingerprint == suspected_fp
            && cache.touched_files_fingerprint == touched_fp
        {
            return cache.message.clone();
        }

        let session_id = self.session_store.session_id().to_string();
        let model = self.models.main.clone();
        let inputs = prompting::RepoContextInputs {
            repo_graph: self.repo_graph.as_deref(),
            suspected_files: &suspected_files,
            changed_files: &changed_files,
            session_id: &session_id,
            model: Some(model.as_str()),
        };
        let message = prompting::repo_context_message(&self.work_root, Some(&task), &inputs);
        self.repo_context_cache = Some(super::RepoContextCache {
            task,
            work_root: self.work_root.clone(),
            repo_graph_present,
            last_feedback_kind,
            suspected_files_fingerprint: suspected_fp,
            touched_files_fingerprint: touched_fp,
            message: message.clone(),
        });
        message
    }

    pub(super) fn prepare_tool_call(&self, mut tool_call: ToolCall) -> ToolCall {
        tool_call.arguments = normalize_tool_call_arguments(&tool_call.name, tool_call.arguments);
        if matches!(tool_call.name.as_str(), "Read" | "Write" | "Edit")
            && let Some(arguments) = tool_call.arguments.as_object_mut()
            && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
            && let Ok(resolved) = resolve_user_path(&self.work_root, raw_path)
        {
            let resolved = if tool_call.name == "Read" {
                self.effective_tool_policy()
                    .focused_edit_policy()
                    .and_then(|policy| {
                        focused_read_target_for_directory(&resolved, &policy.target)
                            .then_some(policy.target.clone())
                    })
                    .unwrap_or(resolved)
            } else {
                resolved
            };
            arguments.insert(
                "path".to_string(),
                serde_json::Value::String(resolved.display().to_string()),
            );
        }
        tool_call
    }

    pub(super) fn push_system_note(&mut self, note: String) {
        if prompting::should_skip_system_note(&self.session.messages, &note) {
            return;
        }
        self.session
            .messages
            .push(ConversationMessage::system(note));
    }

    fn push_user_message(&mut self, content: String) {
        self.session
            .working_memory
            .set_active_task(Some(content.clone()));
        self.session
            .messages
            .push(ConversationMessage::user(content));
    }
}

pub(super) fn last_read_tool_path(messages: &[ConversationMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        message.tool_calls.iter().rev().find_map(|tool_call| {
            if tool_call.name != "Read" {
                return None;
            }
            tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
    })
}

pub(super) fn latest_turn_preferred_read_edit_target(
    messages: &[ConversationMessage],
    work_root: &Path,
) -> Option<PathBuf> {
    let mut latest_existing = None;
    for message in latest_user_turn_slice(messages).iter().rev() {
        if message.role != "assistant" {
            continue;
        }
        for tool_call in message.tool_calls.iter().rev() {
            if tool_call.name != "Read" {
                continue;
            }
            let Some(path) = tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Ok(candidate) = resolve_user_path(work_root, path) else {
                continue;
            };
            if !candidate.is_file() {
                continue;
            }
            latest_existing.get_or_insert_with(|| candidate.clone());
            if is_preferred_read_edit_target(&candidate) {
                return Some(candidate);
            }
        }
    }
    latest_existing
}
