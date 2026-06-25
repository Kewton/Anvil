use std::collections::BTreeSet;
use std::path::Path;

use anyhow::bail;
use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::mode::ExecutionMode;
use crate::providers::ChatClient;
use crate::state::{ConversationMessage, SessionSnapshot};
use crate::tools::path_guard::{
    resolve_existing, resolve_optional_existing, validate_workspace_relative,
};
use crate::tools::registry::{
    ToolContext, ToolRegistry, missing_arg_name, recoverable_tool_error, tool_error_kind,
};
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};

use super::compact::compact_if_needed;
use super::completion::{CompletionContract, format_verify_feedback};
use super::import_scan::{format_missing_import_feedback, scan_relative_imports};
use super::prompt::{ToolPromptMode, build_request_messages};
use super::repair_progress::{
    RepairProgressVerdict, VerificationSignature, classify_repair_progress,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStopReason {
    AssistantFinal,
    RequiredArtifactsSatisfiedAfterTool,
    CompletionContractSatisfied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSessionOutcome {
    pub final_text: String,
    pub stop_reason: RunStopReason,
    pub changed_paths: Vec<String>,
    pub iterations: usize,
    pub tool_calls: usize,
    pub missing_required_paths: Vec<String>,
    pub verify_attempts: usize,
    pub last_blocking_reason: Option<String>,
    pub last_provider_error: Option<String>,
}

const ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD: usize = 3;
const ARTIFACT_RECOVERY_ATTEMPT_LIMIT: usize = 2;
const VERIFY_REPAIR_NO_EDIT_LIMIT: usize = 2;

#[derive(Debug, Default)]
struct VerifyRepairState {
    pending_signature: Option<VerificationSignature>,
    no_edit_turns: usize,
}

#[derive(Debug)]
struct VerifyFailureFeedback {
    feedback: String,
    signature: VerificationSignature,
}

pub fn run_session(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    config: &Config,
) -> anyhow::Result<String> {
    run_session_with_required_paths(client, session, user_prompt, &[], config)
}

pub fn run_session_with_required_paths(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
) -> anyhow::Result<String> {
    run_session_with_required_paths_with_ui(
        client,
        session,
        user_prompt,
        required_paths,
        config,
        &NOOP_UI,
    )
}

pub fn run_session_with_required_paths_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    Ok(
        run_session_with_outcome_with_ui(client, session, user_prompt, required_paths, config, ui)?
            .final_text,
    )
}

pub fn run_session_with_outcome_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    let registry = ToolRegistry::default();
    let mut native_tools_enabled =
        client.supports_native_tools(&config.model) && !session.native_tools_disabled;
    let completion_contract = CompletionContract::load_for_config(config)?;
    let explicit_required_paths = !required_paths.is_empty();
    let required_paths = effective_required_paths(
        &config.workspace_root,
        required_paths,
        user_prompt,
        completion_contract
            .as_ref()
            .map(|contract| contract.required_paths.as_slice())
            .unwrap_or(&[]),
    );
    let initially_missing_paths = missing_paths(&config.workspace_root, &required_paths);
    let mut pending_feedback: Option<String> = None;
    let mut verify_attempts = 0usize;
    let mut last_blocking_reason: Option<String> = None;
    let last_provider_error: Option<String> = None;
    let mut write_or_edit_seen = false;
    let mut no_tool_feedbacks = 0usize;
    let mut empty_feedbacks = 0usize;
    let mut changed_paths: Vec<String> = Vec::new();
    let mut tool_call_count = 0usize;
    let artifact_recovery_enabled = completion_contract.is_some() || explicit_required_paths;
    let mut artifact_non_edit_streak = 0usize;
    let mut artifact_recovery_attempts = 0usize;
    let mut verify_repair_state = VerifyRepairState::default();
    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));

    for iteration in 0..config.max_iterations {
        if ui.interrupted() {
            bail!("interrupted by user");
        }
        compact_if_needed(&mut session.messages, config.context_budget);
        let specs = registry.specs().to_vec();
        let request_tools = if native_tools_enabled {
            specs.clone()
        } else {
            Vec::new()
        };
        let request_messages = build_request_messages(
            &session.messages,
            &specs,
            &config.workspace_root,
            pending_feedback.as_deref(),
            if native_tools_enabled {
                ToolPromptMode::Native
            } else {
                ToolPromptMode::XmlFallback
            },
        );
        let label = format!("{} {}", client.label(), config.model);
        let chat_result = {
            let _guard = ui.before_model_call(&label);
            client.chat(
                &config.model,
                &request_messages,
                &request_tools,
                native_tools_enabled,
            )
        };
        let reply = match chat_result {
            Ok(reply) => {
                pending_feedback = None;
                reply
            }
            Err(err)
                if native_tools_enabled
                    && client.allows_xml_fallback()
                    && provider_error_allows_xml_fallback(&err) =>
            {
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "fallback_decision",
                        "from": "native_tools",
                        "to": "xml_fallback",
                        "allowed": true,
                        "reason": eval_events::body_snippet(&err.to_string()),
                    }),
                );
                native_tools_enabled = false;
                session.native_tools_disabled = true;
                pending_feedback = Some(super::feedback::malformed_tool_call(&err.to_string()));
                continue;
            }
            Err(err) => {
                let message = err.to_string();
                if native_tools_enabled && client.allows_xml_fallback() {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "fallback_decision",
                            "from": "native_tools",
                            "to": "xml_fallback",
                            "allowed": false,
                            "reason": eval_events::body_snippet(&message),
                        }),
                    );
                }
                return Err(err);
            }
        };
        ui.publish_status(UiStatus::for_model_reply(
            config,
            &config.model,
            client.label(),
            reply.prompt_tokens,
            reply.completion_tokens,
        ));
        if ui.interrupted() {
            bail!("interrupted by user");
        }
        let tool_calls = reply.tool_calls.clone();
        tool_call_count += tool_calls.len();
        session.messages.push(ConversationMessage::assistant(
            reply.content.clone(),
            tool_calls.clone(),
        ));
        if tool_calls.is_empty() {
            let missing = missing_paths(&config.workspace_root, &required_paths);
            if !missing.is_empty() {
                session.messages.pop();
                last_blocking_reason =
                    Some(format!("missing required paths: {}", missing.join(", ")));
                pending_feedback = Some(super::feedback::missing_artifacts(&missing));
                continue;
            }
            if reply.content.trim().is_empty() && empty_feedbacks < 1 {
                empty_feedbacks += 1;
                session.messages.pop();
                last_blocking_reason = Some("empty assistant response".to_string());
                pending_feedback = Some(super::feedback::empty_response());
                continue;
            }
            if !write_or_edit_seen && looks_like_action_prompt(user_prompt) {
                if no_tool_feedbacks < 1 {
                    no_tool_feedbacks += 1;
                    session.messages.pop();
                    last_blocking_reason = Some("completion without write".to_string());
                    pending_feedback = Some(super::feedback::completion_without_write());
                    continue;
                }
                session.messages.pop();
                bail!("missing tool call for action prompt after feedback");
            }
            if !write_or_edit_seen
                && looks_like_progress_without_tool(&reply.content)
                && no_tool_feedbacks < 3
            {
                no_tool_feedbacks += 1;
                session.messages.pop();
                last_blocking_reason = Some("progress text without tool call".to_string());
                pending_feedback = Some(super::feedback::no_tool_progress());
                continue;
            }
            let mut import_scan_paths = changed_paths.clone();
            import_scan_paths.extend(required_paths.iter().cloned());
            let missing_imports =
                scan_relative_imports(&config.workspace_root, &import_scan_paths)?;
            if !missing_imports.is_empty() {
                session.messages.pop();
                last_blocking_reason = Some("missing relative imports".to_string());
                pending_feedback = Some(format_missing_import_feedback(&missing_imports));
                continue;
            }
            if let Some(contract) = completion_contract
                .as_ref()
                .filter(|contract| contract.has_verify())
            {
                if let Some(feedback) = handle_verify_repair_no_edit(
                    config.eval_events_path.as_deref(),
                    &mut verify_repair_state,
                )? {
                    session.messages.pop();
                    last_blocking_reason = Some("verify repair missing edit".to_string());
                    pending_feedback = Some(feedback);
                    continue;
                }
                match verify_completion_contract(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    contract,
                    &mut verify_attempts,
                    verify_repair_state.pending_signature.as_ref(),
                    true,
                ) {
                    Ok(None) => {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "loop_stop",
                                "reason": "completion_contract_satisfied",
                                "required_paths": required_paths,
                                "verify_attempts": verify_attempts,
                            }),
                        );
                        return Ok(RunSessionOutcome {
                            final_text: reply.content,
                            stop_reason: RunStopReason::CompletionContractSatisfied,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: Vec::new(),
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Ok(Some(feedback)) => {
                        session.messages.pop();
                        last_blocking_reason = Some("completion verify failed".to_string());
                        verify_repair_state.pending_signature = Some(feedback.signature);
                        verify_repair_state.no_edit_turns = 0;
                        pending_feedback = Some(feedback.feedback);
                        continue;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            return Ok(RunSessionOutcome {
                final_text: reply.content,
                stop_reason: RunStopReason::AssistantFinal,
                changed_paths,
                iterations: iteration + 1,
                tool_calls: tool_call_count,
                missing_required_paths: Vec::new(),
                verify_attempts,
                last_blocking_reason,
                last_provider_error,
            });
        }

        let context = ToolContext {
            root: config.workspace_root.clone(),
            mode: ExecutionMode::Act,
            auto_approve: config.yes,
            interactive_approval: false,
            offline: config.offline,
            workspace_policy: crate::tools::workspace_policy::WorkspacePolicy::for_task_request(),
        };
        let mut names_seen = BTreeSet::new();
        let mut batch_had_edit = false;
        let mut batch_non_edit_tools = 0usize;
        for call in tool_calls {
            if ui.interrupted() {
                bail!("interrupted by user");
            }
            let call_is_edit = matches!(call.name.as_str(), "Write" | "Edit");
            if call_is_edit {
                batch_had_edit = true;
            } else {
                batch_non_edit_tools += 1;
            }
            if !names_seen.insert(call.name.clone()) {
                // Multiple same-tool calls are fine; this keeps clippy from seeing unused state.
            }
            let shape = eval_events::argument_shape(&call.arguments);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "tool_call_raw",
                    "name": call.name.as_str(),
                    "arguments": shape,
                }),
            );
            let result = {
                let _guard = ui.before_tool_call(&call.name);
                registry.execute(&call.name, &call.arguments, &context)
            };
            let result = match result {
                Ok(result) => {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "ok",
                        }),
                    );
                    if matches!(call.name.as_str(), "Write" | "Edit") {
                        write_or_edit_seen = true;
                        if let Some(path) =
                            changed_path_from_call(&config.workspace_root, &call.arguments)
                            && !changed_paths.contains(&path)
                        {
                            changed_paths.push(path);
                        }
                    }
                    result
                }
                Err(err) if recoverable_tool_error(&err) => {
                    let kind = tool_error_kind(&err);
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_validation_error",
                            "name": call.name.as_str(),
                            "error_kind": kind,
                            "missing_arg": missing_arg_name(&err),
                        }),
                    );
                    recoverable_tool_feedback(&call.name, &err)
                }
                Err(err) => {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "error",
                            "error_kind": tool_error_kind(&err),
                        }),
                    );
                    return Err(err);
                }
            };
            session.messages.push(ConversationMessage::tool_result(
                call.name,
                Some(call.id),
                result,
            ));
        }
        if batch_had_edit {
            artifact_non_edit_streak = 0;
        } else {
            artifact_non_edit_streak += batch_non_edit_tools;
        }
        if required_paths_satisfied_after_tool(
            &config.workspace_root,
            &required_paths,
            &initially_missing_paths,
            write_or_edit_seen,
        ) {
            if let Some(contract) = completion_contract
                .as_ref()
                .filter(|contract| contract.has_verify())
            {
                if !batch_had_edit
                    && let Some(feedback) = handle_verify_repair_no_edit(
                        config.eval_events_path.as_deref(),
                        &mut verify_repair_state,
                    )?
                {
                    last_blocking_reason = Some("verify repair missing edit".to_string());
                    pending_feedback = Some(feedback);
                    continue;
                }
                match verify_completion_contract(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    contract,
                    &mut verify_attempts,
                    verify_repair_state.pending_signature.as_ref(),
                    batch_had_edit,
                ) {
                    Ok(None) => {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "loop_stop",
                                "reason": "completion_contract_satisfied",
                                "required_paths": required_paths,
                                "verify_attempts": verify_attempts,
                            }),
                        );
                        return Ok(RunSessionOutcome {
                            final_text: format!(
                                "completion contract satisfied: {}",
                                required_paths.join(", ")
                            ),
                            stop_reason: RunStopReason::CompletionContractSatisfied,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: Vec::new(),
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Ok(Some(feedback)) => {
                        last_blocking_reason = Some("completion verify failed".to_string());
                        verify_repair_state.pending_signature = Some(feedback.signature);
                        verify_repair_state.no_edit_turns = 0;
                        pending_feedback = Some(feedback.feedback);
                        continue;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "loop_stop",
                    "reason": "required_artifacts_satisfied_after_tool",
                    "required_paths": required_paths,
                }),
            );
            return Ok(RunSessionOutcome {
                final_text: format!(
                    "required artifacts satisfied: {}",
                    required_paths.join(", ")
                ),
                stop_reason: RunStopReason::RequiredArtifactsSatisfiedAfterTool,
                changed_paths,
                iterations: iteration + 1,
                tool_calls: tool_call_count,
                missing_required_paths: Vec::new(),
                verify_attempts,
                last_blocking_reason,
                last_provider_error,
            });
        }
        let missing = missing_paths(&config.workspace_root, &required_paths);
        if should_emit_artifact_recovery(
            artifact_recovery_enabled,
            artifact_non_edit_streak,
            &missing,
            completion_contract.as_ref(),
            &config.workspace_root,
        ) {
            artifact_recovery_attempts += 1;
            if artifact_recovery_attempts > ARTIFACT_RECOVERY_ATTEMPT_LIMIT {
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "loop_stop",
                        "reason": "artifact_recovery_exhausted",
                        "missing_paths": missing,
                        "non_edit_streak": artifact_non_edit_streak,
                        "attempts": artifact_recovery_attempts - 1,
                    }),
                );
                bail!("artifact recovery exhausted");
            }
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "artifact_stagnation_feedback",
                    "missing_paths": missing,
                    "attempt": artifact_recovery_attempts,
                    "attempt_limit": ARTIFACT_RECOVERY_ATTEMPT_LIMIT,
                    "non_edit_streak": artifact_non_edit_streak,
                }),
            );
            last_blocking_reason = Some("artifact creation stalled".to_string());
            pending_feedback = Some(super::feedback::artifact_stagnation(
                &missing,
                artifact_recovery_attempts,
                ARTIFACT_RECOVERY_ATTEMPT_LIMIT,
            ));
            artifact_non_edit_streak = 0;
            continue;
        }
    }
    let missing = missing_paths(&config.workspace_root, &required_paths);
    let reason = if missing.is_empty() {
        "max_iterations"
    } else {
        "required_artifacts_missing"
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": reason,
            "missing_paths": missing,
            "verify_attempts": verify_attempts,
            "last_blocking_reason": last_blocking_reason,
            "last_provider_error": last_provider_error.as_deref().map(eval_events::body_snippet),
        }),
    );
    bail!(
        "minimal loop reached max_iterations ({})",
        config.max_iterations
    )
}

fn missing_paths(root: &std::path::Path, required_paths: &[String]) -> Vec<String> {
    required_paths
        .iter()
        .filter(|path| resolve_existing(root, path).is_err())
        .cloned()
        .collect()
}

fn effective_required_paths(
    root: &Path,
    explicit: &[String],
    prompt: &str,
    contract_paths: &[String],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in explicit
        .iter()
        .cloned()
        .chain(extract_requested_artifact_paths(root, prompt))
        .chain(contract_paths.iter().cloned())
    {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn verify_completion_contract(
    root: &Path,
    eval_events_path: Option<&Path>,
    contract: &CompletionContract,
    verify_attempts: &mut usize,
    previous_signature: Option<&VerificationSignature>,
    had_edit: bool,
) -> anyhow::Result<Option<VerifyFailureFeedback>> {
    *verify_attempts += 1;
    let report = contract.verify(root);
    let ok = report.is_pass();
    let (signature, verdict) = classify_repair_progress(previous_signature, &report, had_edit);
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "completion_verify",
            "ok": ok,
            "attempt": *verify_attempts,
            "repair_cap": contract.verify_repair_cap,
            "missing_paths": report.missing_paths.clone(),
            "command_failures": report.command_failures.len(),
            "dependency_missing": report.dependency_missing.clone(),
            "primary_reason": eval_events::body_snippet(&report.primary_reason()),
            "failure_signature": signature.label(),
            "repair_progress": verdict.as_str(),
        }),
    );
    if ok {
        return Ok(None);
    }
    if previous_signature.is_some() {
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "verify_repair_progress",
                "verdict": verdict.as_str(),
                "previous_signature": previous_signature.map(VerificationSignature::label).unwrap_or_default(),
                "current_signature": signature.label(),
                "had_edit": had_edit,
            }),
        );
    }
    if *verify_attempts >= contract.verify_repair_cap {
        let stop_reason = terminal_verify_stop_reason(&signature, previous_signature, verdict);
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": stop_reason,
                "verify_attempts": *verify_attempts,
                "primary_reason": eval_events::body_snippet(&report.primary_reason()),
                "repair_progress": verdict.as_str(),
                "failure_signature": signature.label(),
            }),
        );
        bail!(
            "completion contract verify failed after {} attempts: {}",
            *verify_attempts,
            report.primary_reason()
        );
    }
    Ok(Some(VerifyFailureFeedback {
        feedback: format_verify_feedback(&report),
        signature,
    }))
}

fn handle_verify_repair_no_edit(
    eval_events_path: Option<&Path>,
    state: &mut VerifyRepairState,
) -> anyhow::Result<Option<String>> {
    let Some(signature) = state.pending_signature.as_ref() else {
        return Ok(None);
    };
    state.no_edit_turns += 1;
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "verify_repair_turn",
            "has_edit": false,
            "inspect_only": state.no_edit_turns == 1,
            "failure_signature": signature.label(),
            "no_edit_turns": state.no_edit_turns,
        }),
    );
    if state.no_edit_turns >= VERIFY_REPAIR_NO_EDIT_LIMIT {
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": "verify_repair_no_change",
                "failure_signature": signature.label(),
                "no_edit_turns": state.no_edit_turns,
            }),
        );
        bail!("completion contract verify repair made no file changes");
    }
    Ok(Some(super::feedback::verify_repair_edit_required(
        &signature.label(),
        state.no_edit_turns,
        VERIFY_REPAIR_NO_EDIT_LIMIT,
    )))
}

fn terminal_verify_stop_reason(
    signature: &VerificationSignature,
    previous_signature: Option<&VerificationSignature>,
    verdict: RepairProgressVerdict,
) -> String {
    if signature.has_test_discovery_failure() {
        return "test_discovery_failure".to_string();
    }
    if previous_signature.is_some() {
        match verdict {
            RepairProgressVerdict::Unchanged
            | RepairProgressVerdict::Regressed
            | RepairProgressVerdict::Invalid => {
                return format!("verify_repair_progress_{}", verdict.as_str());
            }
            RepairProgressVerdict::Passed | RepairProgressVerdict::Improved => {}
        }
    }
    "verify_repair_exhausted".to_string()
}

fn should_emit_artifact_recovery(
    enabled: bool,
    non_edit_streak: usize,
    missing_paths: &[String],
    contract: Option<&CompletionContract>,
    root: &Path,
) -> bool {
    enabled
        && !missing_paths.is_empty()
        && non_edit_streak >= ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD
        && !contract.is_some_and(|contract| contract.dependency_precondition_active(root))
}

fn provider_error_allows_xml_fallback(err: &anyhow::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains(" api failed:")
        || lower.contains("http")
        || lower.contains("status")
        || lower.contains("network")
        || lower.contains("timeout")
    {
        return false;
    }
    lower.contains("function_call")
        || lower.contains("tool call")
        || lower.contains("tool_call")
        || lower.contains("provider parse")
        || lower.contains("parse")
}

pub(crate) fn extract_requested_artifact_paths(root: &Path, prompt: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    let mut in_required_block = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_ascii_lowercase()
            .starts_with("required final artifacts")
        {
            in_required_block = true;
            continue;
        }
        if in_required_block {
            if trimmed.is_empty() {
                continue;
            }
            if !is_artifact_list_line(trimmed) && looks_like_section_boundary(trimmed) {
                in_required_block = false;
            } else if let Some(candidate) = artifact_candidate_from_line(trimmed)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
                continue;
            }
        }
        for candidate in backticked_candidates(trimmed) {
            if looks_like_artifact_path(&candidate)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
            }
        }
    }
    paths
}

fn requested_artifact_path_allowed(root: &Path, raw: &str) -> bool {
    if validate_workspace_relative(raw).is_err() {
        return false;
    }
    let path = Path::new(raw);
    let blocked = [".anvil", ".git", "target", "node_modules", ".next"];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| blocked.contains(&part))
    }) {
        return false;
    }
    resolve_optional_existing(root, raw).is_ok()
}

fn required_paths_satisfied_after_tool(
    root: &Path,
    required_paths: &[String],
    initially_missing_paths: &[String],
    write_or_edit_seen: bool,
) -> bool {
    if required_paths.is_empty() || !missing_paths(root, required_paths).is_empty() {
        return false;
    }
    write_or_edit_seen
        || initially_missing_paths
            .iter()
            .any(|path| resolve_existing(root, path).is_ok())
}

fn is_artifact_list_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- ")
        || line.starts_with("* ")
        || line.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn looks_like_section_boundary(line: &str) -> bool {
    line.ends_with(':') || line.starts_with('#')
}

fn artifact_candidate_from_line(line: &str) -> Option<String> {
    let mut value = line.trim();
    value = value
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim_start();
    if let Some((head, tail)) = value.split_once(". ")
        && head.chars().all(|ch| ch.is_ascii_digit())
    {
        value = tail.trim_start();
    }
    let first = value.split_whitespace().next().unwrap_or_default();
    let candidate = first
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches([',', ';']);
    if looks_like_artifact_path(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn backticked_candidates(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        out.push(after_start[..end].trim().to_string());
        rest = &after_start[end + 1..];
    }
    out
}

fn looks_like_artifact_path(value: &str) -> bool {
    if value.is_empty() || value.starts_with("http://") || value.starts_with("https://") {
        return false;
    }
    if value.contains('/') {
        return true;
    }
    matches!(
        value,
        "Cargo.toml"
            | "README.md"
            | "package.json"
            | "tsconfig.json"
            | "index.html"
            | "pyproject.toml"
    ) || Path::new(value).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "js" | "jsx"
                | "ts"
                | "tsx"
                | "rs"
                | "py"
                | "md"
                | "txt"
                | "csv"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "html"
                | "css"
        )
    })
}

fn looks_like_progress_without_tool(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("i will")
        || lower.contains("next")
        || lower.contains("作成します")
        || lower.contains("実装します")
        || lower.contains("進めます")
}

fn looks_like_action_prompt(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("create")
        || lower.contains("write")
        || lower.contains("edit")
        || lower.contains("fix")
        || lower.contains("implement")
        || lower.contains("add ")
        || lower.contains("build")
        || lower.contains("作成")
        || lower.contains("実装")
        || lower.contains("修正")
        || lower.contains("追加")
}

fn recoverable_tool_feedback(name: &str, err: &anyhow::Error) -> String {
    format!(
        "Tool call `{name}` was rejected with a recoverable validation error: {err}. Retry with the same tool or another available tool using a valid JSON object that matches the tool schema."
    )
}

fn changed_path_from_call(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let raw = arguments.get("path")?.as_str()?;
    let path = resolve_existing(root, raw).ok()?;
    let root = root.canonicalize().ok()?;
    Some(crate::tools::path_guard::relative_display(&root, &path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::AssistantReply;
    use crate::state::ToolCall;
    use serde_json::json;

    struct Fake {
        replies: Vec<anyhow::Result<AssistantReply>>,
    }

    impl ChatClient for Fake {
        fn label(&self) -> &str {
            "fake"
        }
        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }
        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.replies.remove(0)
        }
    }

    fn config(root: std::path::PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: std::path::PathBuf::from("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            eval_events_path: None,
            completion_contract_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    #[test]
    fn fake_write_then_final() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn completion_contract_without_verify_preserves_early_success() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.txt"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.txt","content":"ok"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create the file",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
    }

    #[test]
    fn minimal_loop_repairs_after_completion_verify_failure() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def fixed():\n    return 1\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractSatisfied
        );
        assert_eq!(outcome.verify_attempts, 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "def fixed():\n    return 1\n"
        );
        assert!(!session.messages.iter().any(|message| {
            message
                .content
                .contains("Deterministic completion verification failed")
        }));
    }

    #[test]
    fn minimal_loop_stops_with_verify_repair_exhausted_after_cap() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":1}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
    }

    #[test]
    fn artifact_stagnation_feedback_then_write_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Grep", json!({"pattern":"date"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Bash", json!({"command":"ls"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"date-helper.js","content":"module.exports = {};\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("date-helper.js").is_file());
    }

    #[test]
    fn artifact_recovery_exhausts_after_repeated_non_edit_tools() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.max_iterations = 10;
        let mut replies = Vec::new();
        for _ in 0..9 {
            replies.push(Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }));
        }
        let mut fake = Fake { replies };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("artifact recovery exhausted"));
    }

    #[test]
    fn verify_failure_requires_edit_after_repeated_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":3}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.max_iterations = 6;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"a.py"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"python3 -m py_compile a.py"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("verify repair made no file changes"));
    }

    #[test]
    fn verify_repair_progress_unchanged_after_edit_still_exhausts() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
    }

    #[test]
    fn run_session_string_wrapper_preserves_existing_cli_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply::text("plain final"))],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize workspace",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "plain final");
    }

    #[test]
    fn changed_paths_are_workspace_relative_after_tool_success() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":"export default function Page(){return null;}"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create page",
            &["src/app/page.tsx".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(outcome.changed_paths, vec!["src/app/page.tsx"]);
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
    }

    #[test]
    fn missing_tool_argument_feedback_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Grep", json!({}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.content.contains("recoverable validation error"))
        );
    }

    #[test]
    fn edit_anchor_mismatch_returns_recoverable_feedback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual content").unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Edit",
                        json!({"path":"a.txt","old_string":"missing anchor","new_string":"replacement"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize workspace",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(
            session.messages.iter().any(|message| message.role == "tool"
                && message.content.contains("edit_anchor_not_found"))
        );
    }

    #[test]
    fn prompt_requested_artifact_feedback_then_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Create the file.\n\nRequired final artifacts:\n- a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content == "done")
        );
    }

    #[test]
    fn completion_without_write_feedback_then_write_then_complete() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn empty_response_gets_one_retry_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("")),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize this workspace.",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content.is_empty())
        );
    }

    #[test]
    fn repeated_planned_action_without_tool_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("I will create it.")),
                Ok(AssistantReply::text("I will create it now.")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing tool call for action prompt"));
    }

    #[test]
    fn provider_http_error_does_not_enable_xml_fallback() {
        assert!(!provider_error_allows_xml_fallback(&anyhow::anyhow!(
            "OpenAI Responses API failed: 500 Internal Server Error"
        )));
        assert!(provider_error_allows_xml_fallback(&anyhow::anyhow!(
            "OpenAI function_call arguments are not valid JSON"
        )));
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_escape_and_metadata_paths() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "\
Required final artifacts:
- ../outside.txt
- /tmp/out.txt
- .anvil/session.json
- target/debug/app
- node_modules/pkg/index.js
- package.json
- src/app/page.tsx
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["package.json", "src/app/page.tsx"]);
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_backticked_escape() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Create `src/main.rs`, not `../main.rs` or `.anvil/log.json`.";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    #[cfg(unix)]
    fn requested_artifact_path_extraction_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
        let prompt = "\
Required final artifacts:
- out/file.txt
- safe/file.txt
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["safe/file.txt"]);
    }

    #[test]
    fn missing_relative_import_gets_repair_prompt_before_final() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/page.tsx","content":"import Widget from './Widget';\nexport default function Page(){return <Widget/>;}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/Widget.tsx","content":"export default function Widget(){return <div/>;}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "create a small Next.js page",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
        assert!(dir.path().join("src/Widget.tsx").is_file());
        assert_eq!(
            session
                .messages
                .iter()
                .filter(|message| message.role == "assistant" && message.content == "done")
                .count(),
            1
        );
    }

    #[test]
    fn dangerous_command_remains_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"rm -rf /"}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "run command",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("dangerous command blocked"));
    }
}
