use std::path::Path;

use super::completion_evidence::{CompletionEvidence, EvidenceSet};
use super::protocol::{
    ExecutionProtocol, ProtocolKind, ProtocolSuccessContext, RequestContext,
    requested_paths_from_text,
};
use super::summary::{ExitReason, LoopStats};
use super::task_contract::TaskContract;
use super::task_workspace_scope::TaskWorkspaceScope;
use super::tester;
use super::verifier_skill::VerifierInputs;
use crate::agent::loop_run::Agent;
use crate::agent::orchestration::RepoVerification;
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ExecutionMode, WorkMode};
use crate::session::feedback::{
    FeedbackFrame, FeedbackFrameDraft, FeedbackKind, build_feedback_frame,
};
use crate::session::store::{ConversationMessage, WorkingMemory};
use crate::tools::bash::BashCommandClass;
use std::collections::VecDeque;

/// Fixed marker for deterministic recovery content. Protocol success treats
/// this as recovery context, not as proof that the model completed the task.
pub(super) const DETERMINISTIC_CONTENT_FALLBACK_TAG: &str = "deterministic_content_fallback";

/// CB-002 (Issue #459): which verifier should run on a successful turn.
///
/// `select_success_verifier` is a pure decision function over three boolean
/// inputs so dispatch can be unit tested without spinning up the full agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SuccessVerifier {
    /// Run `AutoTestRunner::run(plan)`.
    AutoTest,
    /// Delegate to the Tester Skill from the facade.
    Tester,
    /// No verifier ran; record `NoVerifierAvailable` feedback.
    NoVerifier,
    /// Verification is not demanded and no Tester candidate exists.
    Skip,
}

/// CB-002 (Issue #459): Tester is gated independently of
/// `protocol_demands_verifier`. Explicit AutoTest wins only when the protocol
/// demanded verification.
pub(super) fn select_success_verifier(
    protocol_demands_verifier: bool,
    auto_test_some: bool,
    tester_some: bool,
) -> SuccessVerifier {
    if protocol_demands_verifier && auto_test_some {
        SuccessVerifier::AutoTest
    } else if tester_some {
        SuccessVerifier::Tester
    } else if protocol_demands_verifier {
        SuccessVerifier::NoVerifier
    } else {
        SuccessVerifier::Skip
    }
}

/// Issue #607: pure predicate. True when the request is pure install-deps
/// AND EnvSetup-only satisfaction granted, so post-loop AutoTest / Tester /
/// NoVerifier dispatch should be suppressed. The `success.rs` orchestration
/// and the agent-level context-aware verifier check both consult this fn so
/// the suppression decision lives in exactly one place (DR1-001 SSOT).
pub(super) fn suppress_success_verifier_for_context(
    ctx: &RequestContext,
    env_setup_only_satisfied: bool,
) -> bool {
    ctx.is_env_setup_only && env_setup_only_satisfied && !ctx.requires_tests
}

/// Issue #607 (CB-001 fix): pure predicate. True iff the suppression of
/// post-loop verifier dispatch is **specifically grounded** in EnvSetup
/// evidence (i.e. the agent actually saw at least one
/// `VerifierExitZero { class: EnvSetup, .. }`) for a setup-only request on
/// a code-bearing protocol.
///
/// Rationale: the previous implementation used
/// `evidence_set_satisfies_with_context` which returns true for **any**
/// accepted evidence (RepoEdit, BuildTest, EnvSetup). That made setup-only
/// requests over-suppress: a turn with only a stray RepoEdit (or no
/// evidence at all) would silence the verifier dispatch even though no
/// EnvSetup proof was observed. This helper narrows the suppression so it
/// fires only when EnvSetup evidence is the explicit basis.
///
/// AnswerOnly / Docs intentionally return `false` — `npm install` is not an
/// answer-only artifact, and Docs never accepts EnvSetup at all.
pub(super) fn env_setup_only_evidence_satisfies(
    set: &EvidenceSet,
    kind: ProtocolKind,
    ctx: &RequestContext,
) -> bool {
    if !ctx.is_env_setup_only || ctx.requires_tests {
        return false;
    }
    if !matches!(
        kind,
        ProtocolKind::Python | ProtocolKind::TypeScriptUi | ProtocolKind::GenericCode
    ) {
        return false;
    }
    set.iter().any(|ev| {
        matches!(
            ev,
            CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::EnvSetup,
                ..
            }
        )
    })
}

pub(super) fn build_feedback_for_no_verifier(workspace_root: &Path) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::NoVerifierAvailable,
        primary_error: Some("no auto_test verifier detected for this workspace".to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

impl Agent {
    pub(super) fn should_run_auto_test_for_success(&self) -> bool {
        if self.session.mode_state.work_mode == WorkMode::Python {
            return true;
        }
        if self.session.mode_state.work_mode == WorkMode::TypeScriptUi {
            return true;
        }
        if !recent_successful_bash_commands_since_last_user(&self.session.messages).is_empty() {
            return true;
        }
        let scope = self.current_workspace_scope();
        let project_unit = if let Some(request) = self.active_request_text() {
            super::project_probe::probe_project_unit_for_request(
                &self.work_root,
                &request,
                &scope,
                &self.turn_edited_relative_paths,
            )
        } else {
            super::project_probe::probe_project_unit(
                &self.work_root,
                &scope,
                &self.turn_edited_relative_paths,
            )
        };
        if project_unit.is_some_and(|unit| !unit.verifier_candidates.is_empty()) {
            return true;
        }
        self.active_request_text()
            .is_some_and(|request| super::quality::request_explicitly_requires_tests(&request))
    }

    /// Issue #607 (DR1-001 SSOT): build the current `RequestContext` from
    /// the active request text. Same fn used by every sink so the EnvSetup
    /// suppression decision matches across `evidence_set_satisfies_with_context`,
    /// `evidence_set_missing_shapes_with_context`, and
    /// `should_run_auto_test_for_success_with_context`.
    pub(super) fn current_request_context(&self) -> RequestContext {
        let request = self.active_request_text().unwrap_or_default();
        RequestContext {
            requires_tests: super::quality::request_explicitly_requires_tests(&request),
            is_env_setup_only: super::quality::request_is_env_setup_only(&request),
        }
    }

    /// Issue #607: context-aware variant. When the request is pure
    /// setup-only AND the agent observed enough evidence to satisfy the
    /// active protocol via EnvSetup alone, suppress every post-loop
    /// verifier dispatch (AutoTest / Tester / NoVerifier). Otherwise the
    /// existing heuristic (`should_run_auto_test_for_success`) wins.
    pub(super) fn should_run_auto_test_for_success_with_context(
        &self,
        ctx: &RequestContext,
        env_setup_only_satisfied: bool,
    ) -> bool {
        if suppress_success_verifier_for_context(ctx, env_setup_only_satisfied) {
            return false;
        }
        self.should_run_auto_test_for_success()
    }

    /// Issue #651 Phase 5.1: derive `(owned_test_artifacts, test_execution_required)`
    /// from the active request text for the success-path verifier
    /// dispatch. Builds a one-shot `TaskContract` from
    /// `active_request_text` (the same SSOT producer that drives the
    /// post-loop verifier flow) and asks the contract for both the
    /// `RequiredBehaviorContract.test_execution_required` flag and the
    /// classified `owned_test_artifacts` slice.
    ///
    /// Returns `(vec![], false)` when there is no active request — the
    /// structured Weak/Missing branch is then skipped and the legacy
    /// `detect_with_recent_successes -> run` path runs verbatim.
    fn success_verifier_test_binding(&mut self) -> (Vec<String>, bool) {
        let Some(request) = self.active_request_text() else {
            return (Vec::new(), false);
        };
        let contract = TaskContract::from_request(&request);
        let test_execution_required = contract.required_behavior.test_execution_required;
        let owned_test_artifacts =
            super::owned_test_projection::owned_test_artifacts_for_verifier(self, &contract);
        (owned_test_artifacts, test_execution_required)
    }

    pub(super) fn run_post_loop_success_verifier(
        &mut self,
        final_verif: &RepoVerification,
        stats: &LoopStats,
        model_repo_edits_this_turn: usize,
        verifier_already_dispatched: bool,
        exit_reason: &mut ExitReason,
        error_text: &mut String,
    ) -> Vec<String> {
        let mut verify_commands_collected: Vec<String> = Vec::new();
        // Issue #607: build the request context once and reuse it across the
        // satisfaction / missing-shapes / post-loop verifier-demand sinks.
        let ctx = self.current_request_context();
        let should_dispatch_success_verifier = if exit_reason.is_success() {
            let protocol = ExecutionProtocol::from_work_mode(self.session.mode_state.work_mode);
            let deterministic_recovery_recorded =
                self.session.last_feedback.as_ref().is_some_and(|frame| {
                    frame.primary_error.as_deref() == Some(DETERMINISTIC_CONTENT_FALLBACK_TAG)
                });
            let requested_paths = self
                .active_request_text()
                .map(|text| requested_paths_from_text(&text))
                .unwrap_or_default();
            // Issue #606 T-1.5 / Issue #607: Stage-2 short-circuit. Context-
            // aware so setup-only EnvSetup evidence satisfies Python /
            // TypeScriptUi / GenericCode protocols when the user only asked
            // to install dependencies.
            let kind = protocol.kind();
            let evidence_satisfied =
                kind.evidence_set_satisfies_with_context(&self.evidence_set_this_turn, &ctx);
            let success_context = ProtocolSuccessContext {
                stats,
                deterministic_recovery_recorded,
                model_repo_edits_this_turn,
                requested_paths: &requested_paths,
                verifier_passed_after_edit: None,
                evidence_satisfied,
            };
            let deterministic_rescued = protocol
                .success_evidence(success_context)
                .deterministic_only
                && evidence_satisfied;
            if let Some(issue) = protocol.success_issue_with_context(success_context) {
                let missing = kind
                    .evidence_set_missing_shapes_with_context(&self.evidence_set_this_turn, &ctx);
                crate::logging::log_completion_evidence_unsatisfied(
                    self.current_turn_index,
                    kind.label(),
                    &missing,
                    self.evidence_set_this_turn.len(),
                );
                *exit_reason = ExitReason::MissingRepoEdits;
                *error_text = issue;
                false
            } else {
                if deterministic_rescued {
                    crate::logging::log_completion_evidence_deterministic_rescued(
                        self.current_turn_index,
                        kind.label(),
                        self.evidence_set_this_turn.len(),
                    );
                }
                if evidence_satisfied {
                    crate::logging::log_completion_evidence_satisfied(
                        self.current_turn_index,
                        kind.label(),
                        self.evidence_set_this_turn.len(),
                    );
                }
                !verifier_already_dispatched
            }
        } else {
            false
        };

        // Issue #607 (CB-001 fix): suppress AutoTest / Tester / NoVerifier
        // dispatch only when the request was pure install-deps AND we
        // explicitly observed at least one EnvSetup VerifierExitZero. The
        // previous implementation used the broad `evidence_set_satisfies_*`
        // OR-fold, which fired for any accepted evidence (RepoEdit only,
        // BuildTest only, etc.) and over-suppressed legitimate verifier
        // runs. See `env_setup_only_evidence_satisfies` doc for details.
        let protocol_kind =
            ExecutionProtocol::from_work_mode(self.session.mode_state.work_mode).kind();
        let env_setup_only_satisfied =
            env_setup_only_evidence_satisfies(&self.evidence_set_this_turn, protocol_kind, &ctx);
        let suppress_success_verifier =
            suppress_success_verifier_for_context(&ctx, env_setup_only_satisfied);

        let tester_candidate_some =
            if should_dispatch_success_verifier && !suppress_success_verifier {
                tester::TesterCandidate::detect(&self.work_root, &stats.changed_files).is_some()
            } else {
                false
            };
        let protocol_demands_verifier = !suppress_success_verifier
            && self.should_run_auto_test_for_success_with_context(&ctx, env_setup_only_satisfied);
        let session_id = self.session_store.session_id().to_string();
        let model = self.models.main.clone();
        let recent_successful_bash_commands =
            recent_successful_bash_commands_since_last_user(&self.session.messages);
        // Issue #651 Phase 5.1: structured verifier binding inputs.
        let (owned_test_artifacts, test_execution_required) = self.success_verifier_test_binding();
        let workspace_scope: TaskWorkspaceScope = self.current_workspace_scope();
        let project_unit = if let Some(request) = self.active_request_text() {
            super::project_probe::probe_project_unit_for_request(
                &self.work_root,
                &request,
                &workspace_scope,
                &self.turn_edited_relative_paths,
            )
        } else {
            super::project_probe::probe_project_unit(
                &self.work_root,
                &workspace_scope,
                &self.turn_edited_relative_paths,
            )
        };
        let v_inputs = VerifierInputs {
            score_inputs: crate::session::anvil_score::AnvilScoreInputs {
                unsafe_blocks_this_turn: self.session.unsafe_blocks_this_turn,
                repo_edit_succeeded_this_turn: self.session.repo_edit_succeeded_this_turn,
                consecutive_no_progress_turns: self.session.consecutive_no_progress_turns,
                prev: self.session.last_anvil_score.as_ref(),
            },
            repo_verification: Some(final_verif),
            should_dispatch_success_verifier,
            protocol_demands_verifier,
            changed_files: &stats.changed_files,
            recent_successful_bash_commands: &recent_successful_bash_commands,
            tester_candidate_some,
            workspace_root: &self.work_root,
            owned_test_artifacts: &owned_test_artifacts,
            project_unit: project_unit.as_ref(),
            test_execution_required,
            workspace_scope: &workspace_scope,
        };

        let started = std::time::Instant::now();
        let snapshot_for_state = self.session.clone();
        let runtime_state = crate::agent::skills::RuntimeState {
            plan_mode: self.session.mode_state.mode == ExecutionMode::Plan,
            interrupted: false,
            turn_index: self.current_turn_index,
            session: &snapshot_for_state,
            last_anvil_score: self.session.last_anvil_score.as_ref(),
            reminder_sidecar_available: false,
            reminder_kind_eligible: false,
            reminder_called_this_turn: self.reminder_called_this_turn,
        };
        let mut events_local: Vec<(&'static str, serde_json::Value)> = Vec::new();
        let invocation = {
            let mut wm = WorkingMemory::default();
            let ctx = crate::agent::skills::SkillExecutionContext {
                working_memory: &mut wm,
                workspace_root: &self.work_root,
            };
            let request = crate::agent::skills::SkillInvocationRequest {
                skill_name: "verifier",
                trigger: crate::agent::skills::SkillTrigger::PostLoop,
                state: &runtime_state,
                input: crate::agent::skills::SkillInput::Verifier(v_inputs),
                ctx,
                get_env: &|k: &str| std::env::var_os(k),
                emit_event: &mut |k, p| events_local.push((k, p)),
                session_id: &session_id,
                model: Some(&model),
            };
            self.skill_registry.invoke(request)
        };
        for (k, p) in events_local {
            log_llm_event(k, p);
        }

        let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
        let final_score = match invocation.output {
            Some(crate::agent::skills::SkillOutput::Verifier(boxed)) => {
                use super::verifier_skill::{
                    AutoTestKindView, VerifierOutcome, sanitize_verify_command_for_case_record,
                };
                let outcome: VerifierOutcome = *boxed;
                let score = match &outcome {
                    VerifierOutcome::AutoTestRan { score, .. }
                    | VerifierOutcome::AutoTestTransportError { score, .. }
                    | VerifierOutcome::TesterDelegated { score }
                    | VerifierOutcome::NoVerifier { score, .. }
                    | VerifierOutcome::Skipped { score }
                    | VerifierOutcome::EnvDisabled { score } => score.clone(),
                    // Issue #651: Weak / Missing carry an AnvilScore so the
                    // post-loop dashboard reads survive the safe stop; the
                    // exit_reason / error_text translation lives below.
                    VerifierOutcome::Weak { score, .. }
                    | VerifierOutcome::Missing { score, .. } => score.clone(),
                };

                if let VerifierOutcome::AutoTestRan {
                    auto_test_kind,
                    auto_test_passed,
                    auto_test_command,
                    auto_test_output,
                    auto_test_reason,
                    ..
                } = &outcome
                {
                    // Issue #651 Phase 5.1 / DR4-004: the structured runner
                    // already redacted `auto_test_command` via
                    // `redact_verifier_command_for_storage` inside
                    // `AutoTestRunner::run_structured`. Re-applying the SSOT
                    // here keeps the legacy `AutoTestRunner::run` path
                    // (shell-based, command field not pre-redacted) safe by
                    // construction.
                    let safe_command =
                        crate::session::feedback::redact_verifier_command_for_storage(
                            auto_test_command,
                        );
                    log_llm_event(
                        "agent.autotest.completed",
                        serde_json::json!({
                            "session_id": &session_id,
                            "command": safe_command,
                            "passed": auto_test_passed,
                            "reason": auto_test_reason,
                        }),
                    );
                    if let Some(sanitized) =
                        sanitize_verify_command_for_case_record(auto_test_command)
                    {
                        verify_commands_collected.push(sanitized);
                    }
                    if !auto_test_passed {
                        let kind_dbg = match auto_test_kind {
                            AutoTestKindView::Build => "Build",
                            AutoTestKindView::Test => "Test",
                        };
                        *exit_reason = ExitReason::VerifierFailed;
                        *error_text = format!(
                            "auto test failed for protocol {kind_dbg}: {}\n{}",
                            auto_test_command, auto_test_output
                        );
                    }
                }
                if let VerifierOutcome::AutoTestRan {
                    feedback: Some(fb),
                    auto_test_combined_output,
                    ..
                } = &outcome
                {
                    // Issue #579: clone the frame so we can override `kind`
                    // when the LLM second-pass disagrees with the first-pass
                    // classification. `auto_test_combined_output` is the
                    // exact string `classify_auto_test` consumed (DR1-001
                    // SSoT), so the orchestrator's `should_request_*`
                    // predicate runs on identical input.
                    let mut fb = fb.clone();
                    let combined_output = auto_test_combined_output.as_str();
                    if let Some(confirmed_kind) =
                        super::classify_confirm_flow::classify_with_feedback_confirm(
                            self,
                            &fb.kind,
                            combined_output,
                        )
                    {
                        fb.kind = confirmed_kind;
                    }
                    self.session.record_feedback_if_unset(fb);
                }
                if let VerifierOutcome::NoVerifier { feedback, .. } = &outcome {
                    self.session.record_feedback_if_unset(feedback.clone());
                }
                if matches!(outcome, VerifierOutcome::TesterDelegated { .. }) {
                    let tester_recorded =
                        super::tester_invocation::try_invoke_tester(self, &stats.changed_files);
                    if !tester_recorded && self.should_run_auto_test_for_success() {
                        let frame = build_feedback_for_no_verifier(&self.work_root);
                        self.session.record_feedback_if_unset(frame);
                    }
                }
                if let VerifierOutcome::AutoTestTransportError { error, .. } = &outcome {
                    *exit_reason = ExitReason::TransportError;
                    *error_text = error.clone();
                }
                if matches!(outcome, VerifierOutcome::EnvDisabled { .. }) {
                    log_llm_event(
                        "agent.autotest.disabled",
                        serde_json::json!({
                            "session_id": &session_id,
                            "reason": "ANVIL_NO_AUTO_TEST",
                        }),
                    );
                }
                // Issue #651 Phase 5.1 / 6.1: SafeStop telemetry. Emit the
                // dedicated `agent.verifier.{weak,missing}` log keys under
                // the per-turn cap (`verifier_safe_stop_emitted_this_turn`)
                // so a multi-iteration turn can re-evaluate without
                // duplicating the alert; the surrounding flow already
                // promoted the post-loop exit_reason to MissingVerification
                // via the Verifier failure path, but this site is the SSOT
                // for the structured stop telemetry.
                if let VerifierOutcome::Weak {
                    owned_test_artifacts_count,
                    command_runner,
                    ..
                } = &outcome
                {
                    if !self.session.verifier_safe_stop_emitted_this_turn {
                        self.session.verifier_safe_stop_emitted_this_turn = true;
                        log_llm_event(
                            "agent.verifier.weak",
                            serde_json::json!({
                                "session_id": &session_id,
                                "turn_index": self.current_turn_index,
                                "iter_index": self.session.iter_count_this_turn,
                                "owned_test_artifacts_count": owned_test_artifacts_count,
                                "command_runner": command_runner,
                                "auto_test_detected": true,
                                "test_execution_required": true,
                            }),
                        );
                    }
                    *exit_reason = ExitReason::SafeStopVerifierWeak;
                    *error_text = ExitReason::SafeStopVerifierWeak
                        .default_error_text()
                        .to_string();
                }
                if let VerifierOutcome::Missing {
                    owned_test_artifacts_count,
                    ..
                } = &outcome
                {
                    if !self.session.verifier_safe_stop_emitted_this_turn {
                        self.session.verifier_safe_stop_emitted_this_turn = true;
                        log_llm_event(
                            "agent.verifier.missing",
                            serde_json::json!({
                                "session_id": &session_id,
                                "turn_index": self.current_turn_index,
                                "iter_index": self.session.iter_count_this_turn,
                                "owned_test_artifacts_count": owned_test_artifacts_count,
                                "auto_test_detected": false,
                                "test_execution_required": true,
                            }),
                        );
                    }
                    *exit_reason = ExitReason::SafeStopVerifierMissing;
                    *error_text = ExitReason::SafeStopVerifierMissing
                        .default_error_text()
                        .to_string();
                }
                Some(score)
            }
            Some(crate::agent::skills::SkillOutput::PermissionDenied(_)) => {
                debug_assert!(false, "PermissionDenied must not reach facade");
                None
            }
            _ => None,
        };

        if let Some(score) = final_score {
            let rendered = score.format_for_prompt();
            let render_chars = rendered.chars().count();
            log_llm_event(
                "agent.anvil_score.computed",
                serde_json::json!({
                    "session_id": &session_id,
                    "turn_index": self.current_turn_index,
                    "score": &score,
                    "render_chars": render_chars,
                    "compute_ms": compute_ms,
                }),
            );
            self.session.last_anvil_score = Some(score);
            self.anvil_score_computed_this_turn = true;
        }

        verify_commands_collected
    }
}

pub(super) fn recent_successful_bash_commands_since_last_user(
    messages: &[ConversationMessage],
) -> Vec<String> {
    let start = messages
        .iter()
        .rposition(|message| message.role == "user")
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut pending_bash_commands: VecDeque<String> = VecDeque::new();
    let mut commands = Vec::new();
    for message in &messages[start..] {
        match message.role.as_str() {
            "assistant" => {
                pending_bash_commands = message
                    .tool_calls
                    .iter()
                    .filter(|tool_call| tool_call.name == "Bash")
                    .filter_map(|tool_call| {
                        tool_call
                            .arguments
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|command| !command.is_empty())
                            .map(ToOwned::to_owned)
                    })
                    .collect();
            }
            "tool" if message.name.as_deref() == Some("Bash") => {
                let Some(command) = pending_bash_commands.pop_front() else {
                    continue;
                };
                if bash_tool_result_succeeded(&message.content) {
                    commands.push(command);
                }
            }
            _ => {}
        }
    }
    if commands.len() > 6 {
        commands.drain(..commands.len() - 6);
    }
    commands
}

fn bash_tool_result_succeeded(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .any(|line| line == "exit_code=0" || line.starts_with("exit_code=0 "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::xml_fallback::ToolCall;
    use serde_json::json;

    #[test]
    fn select_success_verifier_runs_auto_test_when_protocol_demands_it() {
        assert_eq!(
            select_success_verifier(true, true, false),
            SuccessVerifier::AutoTest
        );
        assert_eq!(
            select_success_verifier(true, true, true),
            SuccessVerifier::AutoTest
        );
    }

    #[test]
    fn select_success_verifier_runs_tester_independent_of_protocol_demand() {
        assert_eq!(
            select_success_verifier(false, false, true),
            SuccessVerifier::Tester
        );
        assert_eq!(
            select_success_verifier(false, true, true),
            SuccessVerifier::Tester
        );
    }

    #[test]
    fn select_success_verifier_records_no_verifier_when_required() {
        assert_eq!(
            select_success_verifier(true, false, false),
            SuccessVerifier::NoVerifier
        );
    }

    #[test]
    fn select_success_verifier_skips_when_no_protocol_or_tester_need() {
        assert_eq!(
            select_success_verifier(false, false, false),
            SuccessVerifier::Skip
        );
        assert_eq!(
            select_success_verifier(false, true, false),
            SuccessVerifier::Skip
        );
    }

    #[test]
    fn no_verifier_feedback_uses_structured_kind() {
        let dir = tempfile::tempdir().unwrap();
        let frame = build_feedback_for_no_verifier(dir.path());
        assert_eq!(frame.kind, FeedbackKind::NoVerifierAvailable);
        assert_eq!(
            frame.primary_error.as_deref(),
            Some("no auto_test verifier detected for this workspace")
        );
    }

    // -----------------------------------------------------------------
    // Issue #607 VR-β-04 (f2): setup-only suppression matrix.
    // -----------------------------------------------------------------

    #[test]
    fn suppress_success_verifier_for_context_blocks_pure_setup_only() {
        let ctx = RequestContext {
            requires_tests: false,
            is_env_setup_only: true,
        };
        // setup-only request + EnvSetup-only evidence satisfied → suppress.
        assert!(suppress_success_verifier_for_context(&ctx, true));
        // setup-only request but evidence not yet satisfied → do not
        // suppress (model may still need to act).
        assert!(!suppress_success_verifier_for_context(&ctx, false));
    }

    #[test]
    fn suppress_success_verifier_for_context_keeps_dispatch_when_tests_required() {
        let ctx = RequestContext {
            requires_tests: true,
            is_env_setup_only: true,
        };
        assert!(!suppress_success_verifier_for_context(&ctx, true));
    }

    #[test]
    fn suppress_success_verifier_for_context_keeps_dispatch_for_non_setup_request() {
        let ctx = RequestContext {
            requires_tests: false,
            is_env_setup_only: false,
        };
        // Regular feature request → verifier never suppressed.
        assert!(!suppress_success_verifier_for_context(&ctx, false));
        assert!(!suppress_success_verifier_for_context(&ctx, true));
    }

    #[test]
    fn suppressed_context_skips_post_loop_verifier() {
        let ctx = RequestContext {
            requires_tests: false,
            is_env_setup_only: true,
        };
        assert!(suppress_success_verifier_for_context(&ctx, true));
        // When suppression fires in `run_post_loop_success_verifier`,
        // both `protocol_demands_verifier` and `tester_candidate_some`
        // collapse to false, so `select_success_verifier` returns `Skip`.
        assert_eq!(
            select_success_verifier(false, false, false),
            SuccessVerifier::Skip,
        );
    }

    // -----------------------------------------------------------------
    // Issue #607 (Codex CB-001): env-setup-only evidence helper. The
    // suppression must require an explicit `VerifierExitZero { class:
    // EnvSetup, .. }` observation — RepoEdit-only / empty turns must not
    // trigger the post-loop verifier dispatch suppression.
    // -----------------------------------------------------------------

    use super::super::completion_evidence::{
        CompletionEvidence as CE, EvidenceSet as ES, RepoEditCategory,
    };

    fn setup_only_ctx() -> RequestContext {
        RequestContext {
            requires_tests: false,
            is_env_setup_only: true,
        }
    }

    fn tests_required_ctx() -> RequestContext {
        RequestContext {
            requires_tests: true,
            is_env_setup_only: true,
        }
    }

    fn env_setup_evidence() -> CE {
        CE::VerifierExitZero {
            class: BashCommandClass::EnvSetup,
            command: "npm install".to_string(),
            bound_test_artifacts_count: None,
        }
    }

    fn build_test_evidence() -> CE {
        CE::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: None,
        }
    }

    /// CB-001 (a): setup-only request + only RepoEdit evidence → must NOT
    /// satisfy the EnvSetup-only suppression predicate. Previously the
    /// broad `evidence_set_satisfies_with_context` claimed any accepted
    /// evidence (including a stray RepoEdit) as satisfaction, so the
    /// verifier dispatch was silenced even when no install ran.
    #[test]
    fn env_setup_only_evidence_rejects_repo_edit_only_turn() {
        let mut set = ES::new();
        set.push(CE::RepoEdit {
            category: RepoEditCategory::Impl,
            count: 1,
        });
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
        ] {
            assert!(
                !env_setup_only_evidence_satisfies(&set, kind, &setup_only_ctx()),
                "RepoEdit-only must not count as EnvSetup satisfaction ({kind:?})"
            );
        }
    }

    /// CB-001 (b): setup-only request + empty evidence set → must NOT
    /// satisfy the EnvSetup-only suppression predicate. Empty turns are
    /// not proof that the model installed anything.
    #[test]
    fn env_setup_only_evidence_rejects_empty_evidence_set() {
        let set = ES::new();
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
        ] {
            assert!(
                !env_setup_only_evidence_satisfies(&set, kind, &setup_only_ctx()),
                "empty set must not satisfy EnvSetup ({kind:?})"
            );
        }
    }

    /// CB-001 (c): setup-only request + actual EnvSetup VerifierExitZero
    /// evidence → satisfies suppression on code-bearing protocols (BP-04a
    /// behaviour preserved).
    #[test]
    fn env_setup_only_evidence_accepts_env_setup_verifier_evidence() {
        let mut set = ES::new();
        set.push(env_setup_evidence());
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
        ] {
            assert!(
                env_setup_only_evidence_satisfies(&set, kind, &setup_only_ctx()),
                "EnvSetup verifier should satisfy suppression for {kind:?}"
            );
        }
    }

    /// CB-001: setup-only + BuildTest verifier only (no EnvSetup) must NOT
    /// suppress. BuildTest is the "real" verifier; if the model already
    /// ran tests, completion is decided by the protocol, not by the
    /// setup-only short-circuit.
    #[test]
    fn env_setup_only_evidence_rejects_build_test_only_turn() {
        let mut set = ES::new();
        set.push(build_test_evidence());
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
        ] {
            assert!(
                !env_setup_only_evidence_satisfies(&set, kind, &setup_only_ctx()),
                "BuildTest alone must not trigger EnvSetup suppression ({kind:?})"
            );
        }
    }

    /// CB-001: AnswerOnly / Docs never accept EnvSetup as a basis for
    /// suppression — running `npm install` is not an answer-only artifact,
    /// and Docs never wants verifier evidence at all.
    #[test]
    fn env_setup_only_evidence_rejects_answer_only_and_docs() {
        let mut set = ES::new();
        set.push(env_setup_evidence());
        for kind in [ProtocolKind::AnswerOnly, ProtocolKind::Docs] {
            assert!(
                !env_setup_only_evidence_satisfies(&set, kind, &setup_only_ctx()),
                "{kind:?} must not enable EnvSetup-only suppression"
            );
        }
    }

    /// CB-001: tests-required context flips the predicate off even when
    /// EnvSetup evidence is present — the user expects a real verifier to
    /// run, and BP-04a explicitly defers to BuildTest in that case.
    #[test]
    fn env_setup_only_evidence_rejects_when_tests_required() {
        let mut set = ES::new();
        set.push(env_setup_evidence());
        assert!(!env_setup_only_evidence_satisfies(
            &set,
            ProtocolKind::GenericCode,
            &tests_required_ctx(),
        ));
    }

    /// CB-001: non-setup-only ctx (regular feature request) → suppression
    /// is off, regardless of evidence shape.
    #[test]
    fn env_setup_only_evidence_rejects_non_setup_only_ctx() {
        let ctx = RequestContext {
            requires_tests: false,
            is_env_setup_only: false,
        };
        let mut set = ES::new();
        set.push(env_setup_evidence());
        assert!(!env_setup_only_evidence_satisfies(
            &set,
            ProtocolKind::GenericCode,
            &ctx,
        ));
    }

    /// CB-001 (integration): when the agent sees only a RepoEdit (no
    /// EnvSetup verifier) on a setup-only request, the post-loop verifier
    /// must NOT be suppressed. Compose the helpers the way `success.rs`
    /// does at the dispatch site.
    #[test]
    fn cb001_setup_only_with_only_repo_edit_does_not_suppress_verifier() {
        let mut set = ES::new();
        set.push(CE::RepoEdit {
            category: RepoEditCategory::Setup,
            count: 1,
        });
        let ctx = setup_only_ctx();
        let satisfied = env_setup_only_evidence_satisfies(&set, ProtocolKind::GenericCode, &ctx);
        assert!(!satisfied);
        assert!(!suppress_success_verifier_for_context(&ctx, satisfied));
    }

    /// CB-001 (integration): an empty evidence set on a setup-only
    /// request must still leave the verifier free to dispatch.
    #[test]
    fn cb001_setup_only_with_empty_evidence_does_not_suppress_verifier() {
        let set = ES::new();
        let ctx = setup_only_ctx();
        let satisfied = env_setup_only_evidence_satisfies(&set, ProtocolKind::GenericCode, &ctx);
        assert!(!satisfied);
        assert!(!suppress_success_verifier_for_context(&ctx, satisfied));
    }

    /// CB-001 (integration): setup-only + EnvSetup verifier observed →
    /// suppression fires as designed (BP-04a regression pin).
    #[test]
    fn cb001_setup_only_with_env_setup_verifier_suppresses() {
        let mut set = ES::new();
        set.push(env_setup_evidence());
        let ctx = setup_only_ctx();
        let satisfied = env_setup_only_evidence_satisfies(&set, ProtocolKind::GenericCode, &ctx);
        assert!(satisfied);
        assert!(suppress_success_verifier_for_context(&ctx, satisfied));
    }

    #[test]
    fn recent_successful_bash_commands_collects_only_current_turn_successes() {
        let messages = vec![
            ConversationMessage::user("old task".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "old".to_string(),
                    name: "Bash".to_string(),
                    arguments: json!({"command": "cargo test"}),
                }],
            ),
            ConversationMessage::tool("Bash".to_string(), "exit_code=0\n".to_string()),
            ConversationMessage::user("fix python tests".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![
                    ToolCall {
                        id: "read".to_string(),
                        name: "Read".to_string(),
                        arguments: json!({"path": "app.py"}),
                    },
                    ToolCall {
                        id: "bash".to_string(),
                        name: "Bash".to_string(),
                        arguments: json!({"command": "python3 -m pytest"}),
                    },
                ],
            ),
            ConversationMessage::tool("Read".to_string(), "contents".to_string()),
            ConversationMessage::tool("Bash".to_string(), "stdout\nexit_code=0\n".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "fail".to_string(),
                    name: "Bash".to_string(),
                    arguments: json!({"command": "npm test"}),
                }],
            ),
            ConversationMessage::tool("Bash".to_string(), "exit_code=1\n".to_string()),
        ];

        assert_eq!(
            recent_successful_bash_commands_since_last_user(&messages),
            vec!["python3 -m pytest".to_string()]
        );
    }
}
