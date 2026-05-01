use std::path::Path;

use super::auto_test::AutoTestRunner;
use super::protocol::{ExecutionProtocol, ProtocolSuccessContext, requested_paths_from_text};
use super::summary::{ExitReason, LoopStats};
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
        if AutoTestRunner::detect(&self.work_root, &[]).is_some() {
            return true;
        }
        self.active_request_text()
            .is_some_and(|request| super::quality::request_explicitly_requires_tests(&request))
    }

    pub(super) fn run_post_loop_success_verifier(
        &mut self,
        final_verif: &RepoVerification,
        stats: &LoopStats,
        model_repo_edits_this_turn: usize,
        exit_reason: &mut ExitReason,
        error_text: &mut String,
    ) -> Vec<String> {
        let mut verify_commands_collected: Vec<String> = Vec::new();
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
            if let Some(issue) = protocol.success_issue_with_context(ProtocolSuccessContext {
                stats,
                deterministic_recovery_recorded,
                model_repo_edits_this_turn,
                requested_paths: &requested_paths,
                verifier_passed_after_edit: None,
            }) {
                *exit_reason = ExitReason::MissingRepoEdits;
                *error_text = issue;
                false
            } else {
                true
            }
        } else {
            false
        };

        let tester_candidate_some = if should_dispatch_success_verifier {
            tester::TesterCandidate::detect(&self.work_root, &stats.changed_files).is_some()
        } else {
            false
        };
        let protocol_demands_verifier = self.should_run_auto_test_for_success();
        let session_id = self.session_store.session_id().to_string();
        let model = self.models.main.clone();
        let recent_successful_bash_commands =
            recent_successful_bash_commands_since_last_user(&self.session.messages);
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
                    log_llm_event(
                        "agent.autotest.completed",
                        serde_json::json!({
                            "session_id": &session_id,
                            "command": auto_test_command,
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
                        *exit_reason = ExitReason::MissingRepoEdits;
                        *error_text = format!(
                            "auto test failed for protocol {kind_dbg}: {}\n{}",
                            auto_test_command, auto_test_output
                        );
                    }
                }
                if let VerifierOutcome::AutoTestRan {
                    feedback: Some(fb), ..
                } = &outcome
                {
                    self.session.record_feedback_if_unset(fb.clone());
                }
                if let VerifierOutcome::NoVerifier { feedback, .. } = &outcome {
                    self.session.record_feedback_if_unset(feedback.clone());
                }
                if matches!(outcome, VerifierOutcome::TesterDelegated { .. }) {
                    let tester_recorded = self.try_invoke_tester(&stats.changed_files);
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
