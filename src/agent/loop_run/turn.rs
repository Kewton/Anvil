use super::active_job_arbiter::RecoveryOwner;
use super::actor_loop_flow::format_iteration_status;
use super::auto_test::{AutoTestKind, AutoTestRunner};
use super::completion_evidence::is_repo_edit_no_op;

use super::answer_only_mode::answer_only_script_command_allowed;
use super::file_excerpt::{
    current_file_hash_for_relative_path, open_excerpt_file_nofollow, truncate_on_char_boundary,
    utf8_prefix_respecting_cap,
};
use super::photon_feedback_derive::request_explicitly_requests_script_execution;
use super::plan_mode_helpers::assistant_model_for_mode;
use super::small_helpers::raw_mode_safe_text;
use super::summary::ExitReason;
use super::tool_display::progress_path_display;
use super::tool_history::{
    focused_edit_target_already_read, focused_read_target_for_directory,
    has_successful_non_plan_repo_edit,
    has_successful_non_plan_repo_edit_after_latest_truncated_tool_call,
    is_preferred_read_edit_target, latest_user_turn_slice, recent_truncated_tool_call_attempt,
};
use super::tool_policy::{
    EffectiveToolPolicy, effective_tool_policy_error_for_call_with_scope,
    workspace_relative_path_for_tool_arg,
};
use super::verifier_orchestration::{
    task_contract_no_verifier_note, task_contract_verifier_targeted_edit_required_note,
    verifier_repair_diagnostic_pending_note, verifier_repair_target_display,
};
use super::workspace_walk::workspace_appears_empty;
use super::*;
use crate::logging::stable_path_hash;
use crate::model_capabilities::model_capabilities;
use crate::modes::plan_act::WorkMode;
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::session::store::ScaffoldArtifactFileSnapshot;
use crate::tools::registry::ToolSpec;
use crate::util::workspace_paths::is_ignored_workspace_display_path;
use std::path::{Path, PathBuf};

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
            super::safe_stop_emit::emit_safe_stop_report_for_verifier_missing(self);
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
                    let _ = super::artifact_completion_record::record_artifact_completion_bash_violation(self, vec![command_arg]);
                } else {
                    let actual_path = arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let _ = super::artifact_completion_record::record_artifact_completion_attempt(
                        self,
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
                let _ =
                    super::artifact_completion_record::record_artifact_completion_bash_violation(
                        self,
                        vec![command_arg],
                    );
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

    pub(super) fn answer_only_policy_error(
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

    pub(super) fn effective_tool_policy_error(
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
