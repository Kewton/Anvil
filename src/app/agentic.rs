//! Agentic tool-use loop extracted from the main app module.
//!
//! Contains the multi-turn structured response execution loop and its
//! helpers.  These are `impl App` methods in a separate file for
//! maintainability — the same pattern used by `mock.rs`.

use crate::agent::{BasicAgentLoop, StructuredAssistantResponse};
use crate::contracts::{AppStateSnapshot, RuntimeState, ToolLogView};
use crate::provider::{ProviderClient, ProviderEvent};
use crate::session::{MessageRole, SessionMessage};
use crate::spinner::Spinner;
use crate::state::StateTransition;
use crate::tooling::{
    LocalToolExecutor, ToolExecutionPayload, ToolExecutionPolicy, ToolExecutionResult,
};
use crate::tui::Tui;

use super::{App, AppError};

impl App {
    /// Execute tool calls and feed results back to the LLM in a loop.
    ///
    /// This implements the agentic tool-use loop:
    /// 1. Execute tool calls from the structured response
    /// 2. Record results (with full payload) into the session
    /// 3. Send updated session back to the LLM
    /// 4. If the LLM responds with more tool calls, repeat (up to MAX_AGENT_ITERATIONS)
    /// 5. When the LLM responds without tool calls, record as final answer
    pub(crate) fn complete_structured_response<C: ProviderClient>(
        &mut self,
        structured: StructuredAssistantResponse,
        status: &str,
        saved_status: &str,
        elapsed_ms: u128,
        tui: &Tui,
        provider_client: &C,
    ) -> Result<Vec<String>, AppError> {
        let max_iterations = self.config.runtime.max_agent_iterations;
        let mut current = structured;
        let mut frames = Vec::new();
        let mut total_tool_count = 0usize;
        let mut all_tool_log_views: Vec<ToolLogView> = Vec::new();

        for iteration in 0..max_iterations {
            // Show plan for this iteration
            let inferred_plan = infer_plan_from_structured_response(&current);
            let thinking = AppStateSnapshot::new(RuntimeState::Thinking)
                .with_status(format!(
                    "Prepared execution plan (iteration {})",
                    iteration + 1
                ))
                .with_plan(inferred_plan, Some(0))
                .with_reasoning_summary(vec![
                    "validated structured tool response".to_string(),
                    "ready to execute tool plan".to_string(),
                ]);
            // Use ResumeThinking when coming from Working state (iteration > 0)
            let transition = if self.state_machine.snapshot().state == RuntimeState::Working {
                StateTransition::ResumeThinking
            } else {
                StateTransition::StartThinking
            };
            let _ = self.transition_with_context(thinking, transition)?;
            frames.push(self.render_console(tui)?);

            // Execute tool calls and record results WITH payload
            let results = self.execute_structured_tool_calls(&current)?;
            total_tool_count += results.len();

            let tool_log_views: Vec<ToolLogView> = results
                .iter()
                .map(ToolExecutionResult::to_tool_log_view)
                .collect();
            all_tool_log_views.extend(tool_log_views.clone());

            let working = AppStateSnapshot::new(RuntimeState::Working)
                .with_status(format!(
                    "Executed {} tool call(s). Sending results to model...",
                    results.len()
                ))
                .with_tool_logs(tool_log_views)
                .with_elapsed_ms(elapsed_ms);
            let _ = self.transition_with_context(working, StateTransition::StartWorking)?;
            frames.push(self.render_console(tui)?);

            // Send tool results back to LLM for the next turn
            let spinner = Spinner::start(format!(
                "Analyzing results. model={} (iteration {})",
                self.config.runtime.model,
                iteration + 2
            ));

            let request = BasicAgentLoop::build_turn_request(
                self.config.runtime.model.clone(),
                &self.session,
                self.provider.capabilities.streaming && self.config.runtime.stream,
                self.config.runtime.context_window,
            );

            let mut next_token_buffer = String::new();
            let mut first_token = true;
            let mut spinner_opt = Some(spinner);

            let stream_result =
                provider_client.stream_turn(&request, &mut |event| {
                    if let Some(s) = spinner_opt.take() {
                        s.stop();
                    }
                    if let ProviderEvent::TokenDelta(delta) = &event {
                        next_token_buffer.push_str(delta);
                        if first_token {
                            first_token = false;
                        }
                        let _ = std::io::Write::write_fmt(
                            &mut std::io::stderr(),
                            format_args!("{delta}"),
                        );
                        let _ = std::io::Write::flush(&mut std::io::stderr());
                    }
                });

            if let Some(s) = spinner_opt.take() {
                s.stop();
            }
            if !first_token {
                let _ = std::io::Write::write_fmt(
                    &mut std::io::stderr(),
                    format_args!("\n"),
                );
            }

            stream_result.map_err(|err| match err {
                crate::provider::ProviderTurnError::Backend(msg) => {
                    AppError::ToolExecution(format!("agentic follow-up failed: {msg}"))
                }
                crate::provider::ProviderTurnError::Cancelled => AppError::ToolExecution(
                    "agentic follow-up cancelled".to_string(),
                ),
            })?;

            // Parse the follow-up response (retry once on parse failure)
            let next_structured = match BasicAgentLoop::parse_structured_response(
                &next_token_buffer,
            ) {
                Ok(parsed) => parsed,
                Err(first_err) => {
                    // LLMs occasionally produce malformed output; treat the
                    // raw text as a plain final answer rather than failing the
                    // entire turn.
                    let trimmed = next_token_buffer.trim();
                    if !trimmed.is_empty() {
                        StructuredAssistantResponse {
                            tool_calls: Vec::new(),
                            final_response: trimmed.to_string(),
                        }
                    } else {
                        return Err(AppError::ToolExecution(first_err));
                    }
                }
            };

            if next_structured.tool_calls.is_empty() {
                // No more tool calls — this is the final answer
                self.record_assistant_output(
                    self.next_message_id("assistant"),
                    next_structured.final_response,
                )?;
                break;
            }

            // More tool calls — continue the loop
            current = next_structured;
        }

        // Transition to Done
        let done = AppStateSnapshot::new(RuntimeState::Done)
            .with_status(status.to_string())
            .with_tool_logs(all_tool_log_views)
            .with_completion_summary(
                format!(
                    "Executed {} tool call(s) across agentic loop. {}",
                    total_tool_count, saved_status
                ),
                saved_status.to_string(),
            )
            .with_elapsed_ms(elapsed_ms);
        let _ = self.transition_with_context(done, StateTransition::Finish)?;
        frames.push(self.render_console(tui)?);
        Ok(frames)
    }

    pub(crate) fn execute_structured_tool_calls(
        &mut self,
        structured: &StructuredAssistantResponse,
    ) -> Result<Vec<ToolExecutionResult>, AppError> {
        let executor = LocalToolExecutor::new(self.config.paths.cwd.clone());
        let mut results = Vec::new();
        for call in &structured.tool_calls {
            let validated = match self.tools.validate(call.clone()) {
                Ok(v) => v,
                Err(err) => {
                    let error_result = ToolExecutionResult {
                        tool_call_id: call.tool_call_id.clone(),
                        tool_name: call.tool_name.clone(),
                        status: crate::tooling::ToolExecutionStatus::Failed,
                        summary: format!("validation failed: {err:?}"),
                        payload: crate::tooling::ToolExecutionPayload::None,
                        artifacts: Vec::new(),
                        elapsed_ms: 0,
                    };
                    self.session.push_message(
                        SessionMessage::new(
                            MessageRole::Tool,
                            "tool",
                            format_tool_result_message(
                                &error_result,
                                self.config.runtime.tool_result_max_chars,
                            ),
                        )
                        .with_id(self.next_message_id("tool")),
                    );
                    results.push(error_result);
                    continue;
                }
            };
            // Check if this tool needs approval in the current mode
            if self.config.mode.approval_required
                && validated.approval_required(true).is_some()
            {
                let summary = match &call.input {
                    crate::tooling::ToolInput::ShellExec { command } => {
                        format!("{}: {command}", call.tool_name)
                    }
                    crate::tooling::ToolInput::FileWrite { path, .. } => {
                        format!("{}: {path}", call.tool_name)
                    }
                    _ => call.tool_name.clone(),
                };
                // Ask user inline via stderr/stdin
                let approved = prompt_inline_approval(&summary);
                if !approved {
                    let denied_result = ToolExecutionResult {
                        tool_call_id: call.tool_call_id.clone(),
                        tool_name: call.tool_name.clone(),
                        status: crate::tooling::ToolExecutionStatus::Failed,
                        summary: "denied by user".to_string(),
                        payload: crate::tooling::ToolExecutionPayload::None,
                        artifacts: Vec::new(),
                        elapsed_ms: 0,
                    };
                    self.session.push_message(
                        SessionMessage::new(
                            MessageRole::Tool,
                            "tool",
                            format_tool_result_message(
                                &denied_result,
                                self.config.runtime.tool_result_max_chars,
                            ),
                        )
                        .with_id(self.next_message_id("tool")),
                    );
                    results.push(denied_result);
                    continue;
                }
            }
            let request = match validated.approve().into_execution_request(ToolExecutionPolicy {
                approval_required: false,
                allow_restricted: true,
                plan_mode: false,
                plan_scope_granted: true,
            }) {
                Ok(r) => r,
                Err(err) => {
                    let error_result = ToolExecutionResult {
                        tool_call_id: call.tool_call_id.clone(),
                        tool_name: call.tool_name.clone(),
                        status: crate::tooling::ToolExecutionStatus::Failed,
                        summary: format!("{err:?}"),
                        payload: crate::tooling::ToolExecutionPayload::None,
                        artifacts: Vec::new(),
                        elapsed_ms: 0,
                    };
                    self.session.push_message(
                        SessionMessage::new(
                            MessageRole::Tool,
                            "tool",
                            format_tool_result_message(
                                &error_result,
                                self.config.runtime.tool_result_max_chars,
                            ),
                        )
                        .with_id(self.next_message_id("tool")),
                    );
                    results.push(error_result);
                    continue;
                }
            };
            let result = match executor.execute(request) {
                Ok(r) => r,
                Err(err) => ToolExecutionResult {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    status: crate::tooling::ToolExecutionStatus::Failed,
                    summary: err.to_string(),
                    payload: crate::tooling::ToolExecutionPayload::Text(err.to_string()),
                    artifacts: Vec::new(),
                    elapsed_ms: 0,
                },
            };
            // Record tool result WITH actual payload so the LLM can see it
            self.session.push_message(
                SessionMessage::new(
                    MessageRole::Tool,
                    "tool",
                    format_tool_result_message(&result, self.config.runtime.tool_result_max_chars),
                )
                .with_id(self.next_message_id("tool")),
            );
            results.push(result);
        }
        self.persist_session(crate::contracts::AppEvent::SessionSaved)?;
        Ok(results)
    }

    pub(crate) fn handle_structured_done<C: ProviderClient>(
        &mut self,
        event: &crate::agent::AgentEvent,
        tui: &Tui,
        provider_client: &C,
    ) -> Result<Option<Vec<String>>, AppError> {
        let crate::agent::AgentEvent::Done {
            status,
            assistant_message,
            completion_summary: _,
            saved_status,
            tool_logs: _,
            elapsed_ms,
        } = event
        else {
            return Ok(None);
        };

        let structured = BasicAgentLoop::parse_structured_response(assistant_message)
            .map_err(AppError::ToolExecution)?;
        if structured.tool_calls.is_empty() {
            return Ok(None);
        }

        Ok(Some(self.complete_structured_response(
            structured,
            status,
            saved_status,
            *elapsed_ms,
            tui,
            provider_client,
        )?))
    }
}

pub(crate) fn infer_plan_from_structured_response(
    structured: &StructuredAssistantResponse,
) -> Vec<String> {
    let mut plan = vec!["validate requested output scope".to_string()];
    for call in &structured.tool_calls {
        let item = match &call.input {
            crate::tooling::ToolInput::FileWrite { path, .. } => format!("write {path}"),
            crate::tooling::ToolInput::FileRead { path } => format!("read {path}"),
            crate::tooling::ToolInput::FileSearch { pattern, .. } => {
                format!("search for {pattern}")
            }
            crate::tooling::ToolInput::ShellExec { command } => {
                format!("run shell command: {command}")
            }
        };
        plan.push(item);
    }
    plan.push("review generated result and summarize".to_string());
    plan
}

/// Format a tool execution result into a message that the LLM can interpret.
///
/// Includes the actual payload (file content, search matches) so the LLM
/// can reason about the results in subsequent turns.
pub(crate) fn format_tool_result_message(
    result: &ToolExecutionResult,
    max_chars: usize,
) -> String {
    match &result.payload {
        ToolExecutionPayload::None => {
            format!("[tool result: {}] {}", result.tool_name, result.summary)
        }
        ToolExecutionPayload::Text(content) => {
            let truncated = if content.len() > max_chars {
                format!(
                    "{}...\n[truncated, {} bytes total]",
                    &content[..max_chars],
                    content.len()
                )
            } else {
                content.clone()
            };
            format!(
                "[tool result: {}] {}\n{}",
                result.tool_name, result.summary, truncated
            )
        }
        ToolExecutionPayload::Paths(paths) => {
            let listing = paths.join("\n");
            format!(
                "[tool result: {}] {} — {} match(es)\n{}",
                result.tool_name,
                result.summary,
                paths.len(),
                listing
            )
        }
    }
}

/// Prompt the user for inline approval via stderr/stdin.
/// Returns `true` if the user approves, `false` otherwise.
fn prompt_inline_approval(summary: &str) -> bool {
    use std::io::{BufRead, Write};
    let _ = write!(std::io::stderr(), "\n  Allow {summary}? [y/n] ");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if std::io::stdin().lock().read_line(&mut input).is_ok() {
        let answer = input.trim().to_ascii_lowercase();
        matches!(answer.as_str(), "y" | "yes")
    } else {
        false
    }
}

