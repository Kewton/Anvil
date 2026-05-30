use super::active_job_arbiter::RecoveryOwner;
use super::actor_loop_flow::format_iteration_status;
use super::auto_test::{AutoTestKind, AutoTestRunner};
use super::completion_evidence::is_repo_edit_no_op;
use super::repair_driver::{
    VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS, VerifierRepairPassOutcome,
    verifier_repair_pass_timeout_error,
};
use super::repair_patch_validation::{
    CheapCheckOutcome, RepairRejectionSignal, ValidationFailure,
    build_verifier_repair_pass_ledger_outcome, validate_accepted_repair_plan_authorizes_target,
};

use super::answer_only_mode::answer_only_script_command_allowed;
use super::feedback_builders::{
    build_feedback_for_bash, build_feedback_for_edit_failure,
    build_feedback_for_unsafe_block_reason,
};
use super::file_excerpt::{
    current_file_hash_for_relative_path, open_excerpt_file_nofollow, truncate_on_char_boundary,
    utf8_prefix_respecting_cap,
};
use super::path_helpers::normalize_memory_path;
use super::photon_feedback_derive::request_explicitly_requests_script_execution;
use super::plan_mode_helpers::assistant_model_for_mode;
use super::safe_stop_payload::{build_safe_stop_payload, collect_recent_action_labels};
use super::small_helpers::{raw_mode_safe_text, rfc3339_now_utc, user_interrupt_result};
use super::summary::ExitReason;
use super::tool_display::progress_path_display;
use super::tool_execution::{
    failed_outcome_for_call, rejected_outcome_for_call, success_outcome_for_call,
};
use super::tool_history::{
    focused_edit_target_already_read, focused_read_target_for_directory,
    has_successful_non_plan_repo_edit,
    has_successful_non_plan_repo_edit_after_latest_truncated_tool_call,
    is_preferred_read_edit_target, latest_successful_read_existing_path, latest_user_turn_slice,
    latest_verifier_repair_note_index, recent_truncated_tool_call_attempt,
};
use super::tool_policy::{
    EffectiveToolPolicy, EffectiveToolPolicyReason, FocusedEditPolicy,
    effective_tool_policy_error_for_call_with_scope, focused_edit_policy_violation_feedback_note,
    workspace_relative_path_for_tool_arg,
};
use super::verifier_diagnostic_attempt::{
    VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT, verifier_diagnostic_attempt_spec,
};
use super::verifier_orchestration::{
    PreparedVerifierDiagnosticPass, PreparedVerifierRepairPass, VerifierDiagnosticPassOutcome,
    VerifierRepairAttemptProgress, build_verifier_exit_zero_evidence,
    emit_patch_proposal_legacy_validation_comparison_event,
    emit_patch_proposal_shadow_validation_event,
    synthesized_missing_implementation_target_path_for_request, task_contract_no_verifier_note,
    task_contract_verifier_targeted_edit_required_note,
    validate_verifier_repair_intents_with_accepted_plan, verifier_diagnostic_messages,
    verifier_repair_context_diagnostics, verifier_repair_diagnostic_pending_note,
    verifier_repair_intent_limits, verifier_repair_pass_messages,
    verifier_repair_pass_request_error_message, verifier_repair_safe_stop_message,
    verifier_repair_target_display, verifier_repair_transition_message,
    verifier_repair_unsafe_target_message, verifier_setup_policy_message,
};
use super::verifier_repair_shadow::legacy_repair_brief_input_from_assessment;
use super::workspace_candidates::existing_workspace_candidate_for_role_in_scope;
use super::workspace_walk::workspace_appears_empty;
use super::*;
use crate::logging::{log_llm_event, stable_path_hash};
use crate::model_capabilities::model_capabilities;
use crate::modes::plan_act::WorkMode;
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::session::store::ScaffoldArtifactFileSnapshot;
use crate::tools::registry::{BashErrorClass, ToolSpec};
use crate::util::workspace_paths::is_ignored_workspace_display_path;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::deterministic;
use super::quality::{
    first_existing_impl_target, repo_change_request_text, request_allows_fast_polish_fallback,
    request_explicitly_requires_tests, request_mentions_unsupported_ui_framework,
    request_needs_playable_ui_quality_gate, workspace_has_unsupported_ui_framework,
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
pub(super) struct AssistantReplyRetryState {
    pub(super) downgraded_native_tools: bool,
    pub(super) retries_remaining: usize,
    pub(super) tool_call_format_retries_remaining: usize,
    pub(super) extra_transport_retries: usize,
    pub(super) transport_retry_count: usize,
    pub(super) focused_edit_timeout_retry_count: usize,
    pub(super) tool_call_format_retry_count: usize,
}

pub(super) enum AssistantReplyRetryDecision {
    Retry,
    ReturnReply(AssistantReply),
    Fail(String),
}

impl AssistantReplyRetryState {
    pub(super) fn new(chat_retries: usize, message_count: usize) -> Self {
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

    pub(super) fn tool_policy_violation_exit_reason(recovery_owner: RecoveryOwner) -> ExitReason {
        if recovery_owner == RecoveryOwner::MissingVerifierJob {
            ExitReason::MissingVerification
        } else if recovery_owner.is_verifier_owned() {
            ExitReason::VerifierFailed
        } else {
            ExitReason::ToolCallFormatError
        }
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

    pub(super) fn emit_safe_stop_report_for_repair_terminal(
        &mut self,
        reason: super::repair_job::RepairTerminalReason,
    ) {
        let Some(stop_reason) = reason.safe_stop_reason() else {
            return;
        };
        self.emit_repair_safe_stop_report(stop_reason);
    }
    pub(super) fn current_assistant_model(&self) -> String {
        assistant_model_for_mode(
            self.session.mode_state.mode,
            &self.models.main,
            self.plan_model_override.as_deref(),
        )
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
        super::effective_tool_policy_flow::effective_tool_policy(self)
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
        super::effective_tool_policy_flow::build_arbiter_candidates(self)
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
        let target = super::artifact_recovery_flow::artifact_recovery_target_path(self)?;
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

    pub(super) fn tool_specs_for_policy(&self, policy: &EffectiveToolPolicy) -> Vec<ToolSpec> {
        let mut specs = self.tool_registry.specs().to_vec();
        if let Some(allowed_tools) = policy.allowed_tool_names_for_prompt() {
            specs.retain(|spec| allowed_tools.contains(&spec.function.name.as_str()));
        }
        specs
    }

    pub(super) fn local_llm_small_edit_target(&self) -> Option<PathBuf> {
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
        if let Some(target) = super::artifact_recovery_flow::artifact_recovery_target_path(self) {
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

    pub(super) fn mode_policy_message(&self) -> Option<ConversationMessage> {
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

    pub(super) fn forced_small_edit_recovery_message(&self) -> Option<String> {
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
        if let Some(candidate) = super::artifact_recovery_flow::artifact_recovery_target_path(self)
        {
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
        super::active_job_emit::emit_behavior_contract_projected_if_changed(
            self,
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
        super::active_job_emit::emit_behavior_contract_projected_if_changed(
            self,
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
                super::artifact_recovery_flow::maybe_emit_artifact_completion_failed_diagnostic(
                    self,
                );
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

    pub(super) fn artifact_directed_recovery_message(
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

    pub(super) fn verifier_repair_policy_message(
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

    pub(super) fn artifact_directed_policy_violation_message(
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
        if super::task_contract::repo_edit_satisfies_artifact_recovery_target(
            category,
            &relative_path,
            self.current_artifact_recovery_target.as_ref(),
        ) {
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
            super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
                self,
                &relative_path,
                role,
                &scope,
            );
        }
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
        let states =
            super::artifact_state_projection::task_contract_artifact_states(self, contract);
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
        let artifacts =
            super::artifact_state_projection::task_contract_artifact_states(self, contract);
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
        if let Some(path) = self
            .active_request_text()
            .as_deref()
            .and_then(|req| synthesized_missing_implementation_target_path_for_request(role, req))
        {
            return Some(super::task_contract::RecoveryTargetHint {
                role,
                path,
                reason: "no existing implementation artifact for the active request; create a conventional implementation file".to_string(),
            });
        }
        None
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

    pub(super) fn script_execution_requested(&self) -> bool {
        self.active_request_text()
            .as_deref()
            .is_some_and(request_explicitly_requests_script_execution)
    }

    fn effective_tool_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        let effective_tool_policy = super::effective_tool_policy_flow::effective_tool_policy(self);
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

    pub(super) fn workspace_appears_empty(&self) -> bool {
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
        let issue = super::classify_confirm_flow::implementation_quality_issue_with_confirm(
            self, &request, &content,
        )?;
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
        if super::classify_confirm_flow::implementation_quality_issue_with_confirm(
            self, &request, &content,
        )
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
    pub(super) fn prepare_tool_call(&self, mut tool_call: ToolCall) -> ToolCall {
        tool_call.arguments = normalize_tool_call_arguments(&tool_call.name, tool_call.arguments);
        if matches!(tool_call.name.as_str(), "Read" | "Write" | "Edit")
            && let Some(arguments) = tool_call.arguments.as_object_mut()
            && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
            && let Ok(resolved) = resolve_user_path(&self.work_root, raw_path)
        {
            let resolved = if tool_call.name == "Read" {
                super::effective_tool_policy_flow::effective_tool_policy(self)
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

    pub(super) fn push_user_message(&mut self, content: String) {
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
