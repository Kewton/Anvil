//! Agentic tool-use loop extracted from the main app module.
//!
//! Contains the multi-turn structured response execution loop and its
//! helpers.  These are `impl App` methods in a separate file for
//! maintainability — the same pattern used by `mock.rs`.

use crate::agent::subagent::{SubAgentError, SubAgentKind, SubAgentSession};
use crate::agent::{BasicAgentLoop, StructuredAssistantResponse};
use crate::contracts::{AppStateSnapshot, RuntimeState, ToolLogView};
use crate::provider::{ProviderClient, ProviderEvent};
use crate::session::{MessageRole, SessionMessage};
use crate::spinner::Spinner;
use crate::state::StateTransition;
use crate::tooling::{
    ExecutionMode, LocalToolExecutor, ToolCallRequest, ToolExecutionPayload, ToolExecutionPolicy,
    ToolExecutionRequest, ToolExecutionResult, ToolExecutionStatus, ToolInput,
    diff::generate_diff_preview, resolve_sandbox_path,
};
use crate::tui::Tui;
use std::sync::atomic::Ordering;

use super::policy::{OFFLINE_BLOCK_PAYLOAD, check_offline_blocked};
use super::{App, AppError};

/// Maximum number of parallel threads for tool execution.
const MAX_PARALLEL_THREADS: usize = 8;

/// Tool call execution group produced by [`group_by_execution_mode`].
#[derive(Debug)]
pub enum ExecutionGroup {
    /// A group of tools that can be executed in parallel.
    /// Uses parallel execution when 2+ items, sequential for 1 item.
    Parallel(Vec<(usize, ToolExecutionRequest)>),
    /// A single tool that must be executed sequentially.
    Sequential(usize, ToolExecutionRequest),
}

/// Group indexed tool execution requests by their [`ExecutionMode`].
///
/// Consecutive `ParallelSafe` requests are collected into a single
/// [`ExecutionGroup::Parallel`] group.  Each `SequentialOnly` request
/// flushes any accumulated parallel group and becomes its own
/// [`ExecutionGroup::Sequential`] item.
pub fn group_by_execution_mode(requests: &[(usize, ToolExecutionRequest)]) -> Vec<ExecutionGroup> {
    let mut groups = Vec::new();
    let mut current_parallel: Vec<(usize, ToolExecutionRequest)> = Vec::new();

    for (idx, request) in requests {
        if request.spec.execution_mode == ExecutionMode::ParallelSafe {
            current_parallel.push((*idx, request.clone()));
        } else {
            if !current_parallel.is_empty() {
                groups.push(ExecutionGroup::Parallel(std::mem::take(
                    &mut current_parallel,
                )));
            }
            groups.push(ExecutionGroup::Sequential(*idx, request.clone()));
        }
    }

    if !current_parallel.is_empty() {
        groups.push(ExecutionGroup::Parallel(current_parallel));
    }

    groups
}

/// Execute a group of tool requests in parallel using scoped threads.
///
/// This is a standalone function (not an `App` method) to avoid borrow
/// conflicts with `&mut self` in [`App::execute_structured_tool_calls`].
/// Each thread creates its own [`LocalToolExecutor`] instance.
fn execute_parallel_group_standalone(
    config: &crate::config::EffectiveConfig,
    shutdown_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: Vec<(usize, ToolExecutionRequest)>,
) -> Vec<(usize, ToolExecutionResult)> {
    let cwd = config.paths.cwd.clone();
    let runtime = &config.runtime;
    let mut all_results = Vec::new();

    for chunk in requests.chunks(MAX_PARALLEL_THREADS) {
        all_results.extend(std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(idx, request)| {
                    let cwd = cwd.clone();
                    let shutdown = shutdown_flag.clone();
                    let idx = *idx;
                    s.spawn(move || {
                        let mut executor =
                            LocalToolExecutor::new(cwd, runtime).with_shutdown_flag(shutdown);
                        let tool_call_id = request.tool_call_id.clone();
                        let tool_name = request.spec.name.clone();
                        let result = executor.execute(request.clone()).unwrap_or_else(|err| {
                            ToolExecutionResult {
                                tool_call_id,
                                tool_name,
                                status: ToolExecutionStatus::Failed,
                                summary: err.to_string(),
                                payload: ToolExecutionPayload::Text(err.to_string()),
                                artifacts: Vec::new(),
                                elapsed_ms: 0,
                            }
                        });
                        (idx, result)
                    })
                })
                .collect();

            let mut results = Vec::new();
            for handle in handles {
                match handle.join() {
                    Ok(indexed_result) => results.push(indexed_result),
                    Err(panic_payload) => {
                        let detail = panic_payload
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown");
                        tracing::error!("parallel tool thread panicked: {detail}");
                        results.push((
                            usize::MAX,
                            ToolExecutionResult {
                                tool_call_id: String::new(),
                                tool_name: String::new(),
                                status: ToolExecutionStatus::Failed,
                                summary: "parallel tool execution failed unexpectedly".to_string(),
                                payload: ToolExecutionPayload::None,
                                artifacts: Vec::new(),
                                elapsed_ms: 0,
                            },
                        ));
                    }
                }
            }
            results
        }));
    }

    all_results
}

/// Maximum number of sub-agent calls allowed in a single turn (SR4-006).
const MAX_SUBAGENT_CALLS_PER_TURN: usize = 3;

impl App {
    /// Extract sub-agent tool calls, execute them, and return results along
    /// with the remaining normal tool calls (DR1-003).
    fn extract_and_run_subagent_calls<C: ProviderClient>(
        &mut self,
        tool_calls: &[ToolCallRequest],
        provider_client: &C,
    ) -> (Vec<ToolExecutionResult>, Vec<ToolCallRequest>) {
        let (agent_calls, normal_calls): (Vec<_>, Vec<_>) = tool_calls
            .iter()
            .cloned()
            .partition(|tc| SubAgentKind::from_tool_input(&tc.input).is_some());

        let mut agent_results = Vec::new();
        for (index, call) in agent_calls.iter().enumerate() {
            // SR4-006: limit sub-agent calls per turn
            if index >= MAX_SUBAGENT_CALLS_PER_TURN {
                agent_results.push(ToolExecutionResult {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    status: ToolExecutionStatus::Failed,
                    summary: "too many subagent calls in a single turn".to_string(),
                    payload: ToolExecutionPayload::Text(
                        "too many subagent calls in a single turn".to_string(),
                    ),
                    artifacts: Vec::new(),
                    elapsed_ms: 0,
                });
                continue;
            }

            // IR3-004: check shutdown flag
            if self.shutdown_flag.load(Ordering::Relaxed) {
                break;
            }

            let kind = SubAgentKind::from_tool_input(&call.input).unwrap();
            let (prompt, scope) = match &call.input {
                ToolInput::AgentExplore { prompt, scope } => (prompt.as_str(), scope.as_deref()),
                ToolInput::AgentPlan { prompt, scope } => (prompt.as_str(), scope.as_deref()),
                _ => unreachable!(),
            };

            // SR4-001/SR4-008: scope validation
            let scope_path = if let Some(scope_str) = scope {
                // SR4-008: NUL byte / control character check
                if scope_str.chars().any(|c| c.is_control()) {
                    agent_results.push(
                        SubAgentError::SandboxViolation(format!(
                            "scope contains control characters: {:?}",
                            scope_str
                        ))
                        .into_tool_execution_result(call),
                    );
                    continue;
                }
                // SR4-001: path traversal check
                match resolve_sandbox_path(&self.config.paths.cwd, scope_str) {
                    Ok(resolved) => resolved,
                    Err(_) => {
                        agent_results.push(
                            SubAgentError::SandboxViolation(format!(
                                "scope path traversal detected: {}",
                                scope_str
                            ))
                            .into_tool_execution_result(call),
                        );
                        continue;
                    }
                }
            } else {
                self.config.paths.cwd.clone()
            };

            let session = SubAgentSession::new(
                kind,
                prompt,
                &scope_path,
                provider_client,
                &self.config,
                self.shutdown_flag(),
            );
            let result = session.run();
            agent_results.push(match result {
                Ok(r) => r.into_tool_execution_result(call),
                Err(e) => e.into_tool_execution_result(call),
            });
        }

        (agent_results, normal_calls)
    }

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
            // Check shutdown flag before tool execution
            if self.is_shutdown_requested() {
                break;
            }

            // Step 1: Extract and run sub-agent calls (IR3-001)
            let (agent_results, normal_calls) =
                self.extract_and_run_subagent_calls(&current.tool_calls, provider_client);

            // Record sub-agent results first (IR3-002)
            for result in &agent_results {
                self.record_tool_result(result);
            }
            total_tool_count += agent_results.len();

            // Rebuild structured response with only normal calls
            let current_normal = StructuredAssistantResponse {
                tool_calls: normal_calls,
                final_response: current.final_response.clone(),
            };

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
            // Skip intermediate Thinking frames — the user already sees
            // live streaming output on stderr (Issue #1).

            // Execute normal tool calls and record results WITH payload
            let results = self.execute_structured_tool_calls(&current_normal)?;
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
            // Skip intermediate Working frames — tool execution output
            // is already shown on stderr (Issue #1).

            // Check shutdown flag before LLM call
            if self.is_shutdown_requested() {
                break;
            }

            // Send tool results back to LLM for the next turn
            let spinner = Spinner::start(
                format!(
                    "Analyzing results. model={} (iteration {})",
                    self.config.runtime.model,
                    iteration + 2
                ),
                self.config.mode.interactive,
            );

            let request = BasicAgentLoop::build_turn_request(
                self.config.runtime.model.clone(),
                &self.session,
                self.provider.capabilities.streaming && self.config.runtime.stream,
                self.config.runtime.context_window,
                &self.system_prompt,
            );

            let mut next_token_buffer = String::new();
            let mut first_token = true;
            let mut spinner_opt = Some(spinner);

            let stream_result = provider_client.stream_turn(&request, &mut |event| {
                if let Some(s) = spinner_opt.take() {
                    s.stop();
                }
                if let ProviderEvent::TokenDelta(delta) = &event {
                    next_token_buffer.push_str(delta);
                    if first_token {
                        first_token = false;
                    }
                    let _ =
                        std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("{delta}"));
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
            });

            if let Some(s) = spinner_opt.take() {
                s.stop();
            }
            if !first_token {
                let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("\n"));
            }

            match stream_result {
                Err(crate::provider::ProviderTurnError::Cancelled)
                    if self.is_shutdown_requested() =>
                {
                    break;
                }
                other => {
                    other.map_err(|err| match err {
                        crate::provider::ProviderTurnError::Cancelled => {
                            AppError::ToolExecution("agentic follow-up cancelled".to_string())
                        }
                        other => {
                            AppError::ToolExecution(format!("agentic follow-up failed: {other}"))
                        }
                    })?;
                }
            }

            // Parse the follow-up response (retry once on parse failure)
            let next_structured =
                match BasicAgentLoop::parse_structured_response(&next_token_buffer) {
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
        let mut done_snapshot = self.transition_with_context(done, StateTransition::Finish)?;
        self.evaluate_context_warning(&mut done_snapshot);
        frames.push(self.render_console(tui)?);
        Ok(frames)
    }

    /// Phase 1 helper: validate and approve all tool calls.
    ///
    /// Returns a tuple of (successful requests with index, failed results with index).
    #[allow(clippy::type_complexity)]
    fn validate_and_approve_all(
        &mut self,
        tool_calls: &[crate::tooling::ToolCallRequest],
    ) -> (
        Vec<(usize, ToolExecutionRequest)>,
        Vec<(usize, ToolExecutionResult)>,
    ) {
        let mut requests = Vec::new();
        let mut failed_results = Vec::new();

        for (idx, call) in tool_calls.iter().enumerate() {
            let validated = match self.tools.validate(call.clone()) {
                Ok(v) => v,
                Err(err) => {
                    failed_results.push((
                        idx,
                        build_failed_result(call, format!("validation failed: {err:?}")),
                    ));
                    continue;
                }
            };

            // Offline policy check: block network tools before approval
            if let Some(summary) = check_offline_blocked(&self.config, call) {
                failed_results.push((
                    idx,
                    ToolExecutionResult {
                        tool_call_id: call.tool_call_id.clone(),
                        tool_name: call.tool_name.clone(),
                        status: ToolExecutionStatus::Failed,
                        summary,
                        payload: ToolExecutionPayload::Text(OFFLINE_BLOCK_PAYLOAD.to_string()),
                        artifacts: Vec::new(),
                        elapsed_ms: 0,
                    },
                ));
                continue;
            }

            if self.config.mode.approval_required && validated.approval_required(true).is_some() {
                let summary = tool_call_approval_summary(call);
                let diff_preview = generate_diff_preview(&self.config.paths.cwd, &call.input);
                let approved = prompt_inline_approval(&summary, diff_preview.as_deref());
                if !approved {
                    failed_results
                        .push((idx, build_failed_result(call, "denied by user".to_string())));
                    continue;
                }
            }
            match validated
                .approve()
                .into_execution_request(ToolExecutionPolicy {
                    approval_required: false,
                    allow_restricted: true,
                    plan_mode: false,
                    plan_scope_granted: true,
                }) {
                Ok(request) => requests.push((idx, request)),
                Err(err) => {
                    failed_results.push((idx, build_failed_result(call, format!("{err:?}"))));
                }
            }
        }

        (requests, failed_results)
    }

    /// Phase 3 helper: execute a single tool request.
    ///
    /// [D2-008] MCP tools are dispatched here before reaching LocalToolExecutor.
    /// MCP tools have ExecutionMode::SequentialOnly so they never enter
    /// execute_parallel_group_standalone().
    fn execute_single(&mut self, request: ToolExecutionRequest) -> ToolExecutionResult {
        // [D2-008] MCP tool branch -- dispatch via McpManager directly
        if let crate::tooling::ToolInput::Mcp {
            ref server,
            ref tool,
            ref arguments,
        } = request.input
        {
            return self.execute_mcp_tool(
                request.tool_call_id.clone(),
                request.spec.name.clone(),
                server,
                tool,
                arguments.clone(),
            );
        }

        // Built-in tools: delegate to LocalToolExecutor
        let mut executor =
            LocalToolExecutor::new(self.config.paths.cwd.clone(), &self.config.runtime)
                .with_shutdown_flag(self.shutdown_flag());

        executor
            .execute(request)
            .unwrap_or_else(|err| ToolExecutionResult {
                tool_call_id: String::new(),
                tool_name: String::new(),
                status: ToolExecutionStatus::Failed,
                summary: err.to_string(),
                payload: ToolExecutionPayload::Text(err.to_string()),
                artifacts: Vec::new(),
                elapsed_ms: 0,
            })
    }

    /// Execute an MCP tool call via the McpManager.
    fn execute_mcp_tool(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> ToolExecutionResult {
        let Some(ref mut manager) = self.mcp_manager else {
            return ToolExecutionResult {
                tool_call_id,
                tool_name,
                status: ToolExecutionStatus::Failed,
                summary: "MCP manager not available".to_string(),
                payload: ToolExecutionPayload::None,
                artifacts: Vec::new(),
                elapsed_ms: 0,
            };
        };

        let started = std::time::Instant::now();
        let (status, summary, payload) = match manager.call_tool(server, tool, arguments) {
            Ok(result) => (
                ToolExecutionStatus::Completed,
                "MCP tool call succeeded".to_string(),
                ToolExecutionPayload::Text(result),
            ),
            Err(e) => (
                ToolExecutionStatus::Failed,
                format!("{e}"),
                ToolExecutionPayload::Text(format!("{e:?}")),
            ),
        };

        ToolExecutionResult {
            tool_call_id,
            tool_name,
            status,
            summary,
            payload,
            artifacts: Vec::new(),
            elapsed_ms: started.elapsed().as_millis(),
        }
    }

    pub(crate) fn execute_structured_tool_calls(
        &mut self,
        structured: &StructuredAssistantResponse,
    ) -> Result<Vec<ToolExecutionResult>, AppError> {
        // Phase 1: Validation + Approval
        let (validated_requests, mut failed_results) =
            self.validate_and_approve_all(&structured.tool_calls);

        // Phase 1.5: PreToolUse hooks (DR design judgment #1)
        // Build tool_input_map before grouping (DR3-003, DR3-005)
        let tool_input_map: std::collections::HashMap<String, (String, serde_json::Value)> =
            validated_requests
                .iter()
                .map(|(_, r)| {
                    (
                        r.tool_call_id.clone(),
                        (
                            r.spec.name.clone(),
                            serde_json::to_value(&r.input).unwrap_or_default(),
                        ),
                    )
                })
                .collect();

        // Run PreToolUse hooks and filter blocked requests
        let validated_requests = if let Some(ref engine) = self.hooks_engine {
            let mut remaining = Vec::new();
            for (idx, request) in validated_requests {
                if let Some((tool_name, tool_input)) = tool_input_map.get(&request.tool_call_id) {
                    let event = crate::hooks::PreToolUseEvent {
                        hook_point: "PreToolUse",
                        tool_name: tool_name.clone(),
                        tool_input: tool_input.clone(),
                        tool_call_id: request.tool_call_id.clone(),
                    };
                    match engine.run_pre_tool_use(event) {
                        Ok(crate::hooks::PreToolUseOutcome::Continue) => {
                            remaining.push((idx, request));
                        }
                        Ok(crate::hooks::PreToolUseOutcome::Block { reason, .. }) => {
                            tracing::info!(
                                tool = %tool_name,
                                reason = %reason,
                                "PreToolUse hook blocked tool call"
                            );
                            failed_results.push((
                                idx,
                                ToolExecutionResult {
                                    tool_call_id: request.tool_call_id.clone(),
                                    tool_name: request.spec.name.clone(),
                                    status: ToolExecutionStatus::Failed,
                                    summary: format!("blocked by hook: {reason}"),
                                    payload: crate::tooling::ToolExecutionPayload::None,
                                    artifacts: Vec::new(),
                                    elapsed_ms: 0,
                                },
                            ));
                        }
                        Err(crate::hooks::HookError::Shutdown) => {
                            // Propagate shutdown
                            remaining.push((idx, request));
                            break;
                        }
                        Err(err) => {
                            // Soft-fail: continue
                            tracing::warn!("PreToolUse hook error: {err}");
                            remaining.push((idx, request));
                        }
                    }
                } else {
                    remaining.push((idx, request));
                }
            }
            remaining
        } else {
            validated_requests
        };

        // Phase 2: Grouping
        let groups = group_by_execution_mode(&validated_requests);

        // Phase 3: Execution
        let mut indexed_results: Vec<(usize, ToolExecutionResult)> = failed_results;
        for group in groups {
            match group {
                ExecutionGroup::Parallel(requests) if requests.len() >= 2 => {
                    let parallel_results = execute_parallel_group_standalone(
                        &self.config,
                        self.shutdown_flag(),
                        requests,
                    );
                    indexed_results.extend(parallel_results);
                }
                ExecutionGroup::Parallel(requests) => {
                    // Single ParallelSafe item — execute sequentially
                    for (idx, req) in requests {
                        let result = self.execute_single(req);
                        indexed_results.push((idx, result));
                    }
                }
                ExecutionGroup::Sequential(idx, req) => {
                    let result = self.execute_single(req);
                    indexed_results.push((idx, result));
                }
            }
        }

        // Phase 4: Sort by index, record results, run PostToolUse hooks (DR2-004)
        indexed_results.sort_by_key(|(idx, _)| *idx);
        let mut results: Vec<ToolExecutionResult> = Vec::with_capacity(indexed_results.len());
        for (_, result) in indexed_results {
            self.record_tool_result(&result);

            // PostToolUse hook (soft-fail)
            if let Some(ref engine) = self.hooks_engine
                && let Some((tool_name, tool_input)) = tool_input_map.get(&result.tool_call_id)
            {
                let status_str = match result.status {
                    ToolExecutionStatus::Completed => "completed",
                    ToolExecutionStatus::Failed | ToolExecutionStatus::Interrupted => "failed",
                };
                let event = crate::hooks::PostToolUseEvent {
                    hook_point: "PostToolUse",
                    tool_name: tool_name.clone(),
                    tool_input: tool_input.clone(),
                    tool_call_id: result.tool_call_id.clone(),
                    tool_result: crate::hooks::HookToolResult {
                        status: status_str.to_string(),
                        summary: result.summary.clone(),
                    },
                };
                if let Err(err) = engine.run_post_tool_use(event) {
                    tracing::warn!("PostToolUse hook error: {err}");
                }
            }

            results.push(result);
        }

        self.persist_session(crate::contracts::AppEvent::SessionSaved)?;
        Ok(results)
    }

    /// Push a tool execution result into the session as a tool message.
    fn record_tool_result(&mut self, result: &ToolExecutionResult) {
        let is_error = result.status == ToolExecutionStatus::Failed;
        let mut msg = SessionMessage::new(
            MessageRole::Tool,
            "tool",
            format_tool_result_message(result, self.config.runtime.tool_result_max_chars),
        )
        .with_id(self.next_message_id("tool"));
        msg.is_error = is_error;

        // Attach image paths for Image payloads so the agent layer can
        // resolve them to base64 when building the provider request.
        if let ToolExecutionPayload::Image { source_path, .. } = &result.payload {
            msg = msg.with_image_paths(vec![source_path.clone()]);
        }
        self.session.push_message(msg);
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
            crate::tooling::ToolInput::FileEdit { path, .. } => format!("edit {path}"),
            crate::tooling::ToolInput::FileRead { path } => format!("read {path}"),
            crate::tooling::ToolInput::FileSearch { pattern, .. } => {
                format!("search for {pattern}")
            }
            crate::tooling::ToolInput::ShellExec { command } => {
                format!("run shell command: {command}")
            }
            crate::tooling::ToolInput::WebFetch { url } => format!("fetch {url}"),
            crate::tooling::ToolInput::WebSearch { query } => {
                format!("web search: {query}")
            }
            crate::tooling::ToolInput::Mcp { server, tool, .. } => {
                format!("mcp call {server}/{tool}")
            }
            crate::tooling::ToolInput::AgentExplore { prompt, .. } => {
                let truncated = truncate_chars(prompt, 50);
                format!("explore: {truncated}")
            }
            crate::tooling::ToolInput::AgentPlan { prompt, .. } => {
                let truncated = truncate_chars(prompt, 50);
                format!("plan: {truncated}")
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
pub fn format_tool_result_message(result: &ToolExecutionResult, max_chars: usize) -> String {
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
        ToolExecutionPayload::Image { source_path, .. } => {
            format!(
                "[tool result: {}] [画像: {}]",
                result.tool_name, source_path
            )
        }
    }
}

/// Build a failed [`ToolExecutionResult`] with no payload.
fn build_failed_result(
    call: &crate::tooling::ToolCallRequest,
    summary: String,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_call_id: call.tool_call_id.clone(),
        tool_name: call.tool_name.clone(),
        status: crate::tooling::ToolExecutionStatus::Failed,
        summary,
        payload: crate::tooling::ToolExecutionPayload::None,
        artifacts: Vec::new(),
        elapsed_ms: 0,
    }
}

/// Produce a human-readable summary of a tool call for the approval prompt.
fn tool_call_approval_summary(call: &crate::tooling::ToolCallRequest) -> String {
    match &call.input {
        crate::tooling::ToolInput::ShellExec { command } => {
            format!("{}: {command}", call.tool_name)
        }
        crate::tooling::ToolInput::FileWrite { path, .. } => {
            format!("{}: {path}", call.tool_name)
        }
        crate::tooling::ToolInput::FileEdit { path, .. } => {
            format!("{}: {path}", call.tool_name)
        }
        crate::tooling::ToolInput::WebFetch { url } => {
            format!("{}: {url}", call.tool_name)
        }
        crate::tooling::ToolInput::WebSearch { query } => {
            format!("Web search: {query}")
        }
        crate::tooling::ToolInput::Mcp {
            server,
            tool,
            arguments,
        } => {
            let args_preview = {
                let formatted = serde_json::to_string_pretty(arguments)
                    .unwrap_or_else(|_| arguments.to_string());
                truncate_chars(&formatted, 500)
            };
            format!("MCP {server}/{tool}\n  arguments: {args_preview}")
        }
        crate::tooling::ToolInput::AgentExplore { prompt, scope, .. } => {
            let scope_info = scope.as_deref().unwrap_or("(project root)");
            let truncated = truncate_chars(prompt, 100);
            format!("agent.explore [scope: {scope_info}]: {truncated}")
        }
        crate::tooling::ToolInput::AgentPlan { prompt, scope, .. } => {
            let scope_info = scope.as_deref().unwrap_or("(project root)");
            let truncated = truncate_chars(prompt, 100);
            format!("agent.plan [scope: {scope_info}]: {truncated}")
        }
        _ => call.tool_name.clone(),
    }
}

/// Prompt the user for inline approval via stderr/stdin.
/// Returns `true` if the user approves, `false` otherwise.
fn prompt_inline_approval(summary: &str, diff_preview: Option<&str>) -> bool {
    use std::io::{BufRead, Write};
    if let Some(diff) = diff_preview {
        let _ = write!(std::io::stderr(), "\n{}\n", crate::tui::colorize_diff(diff));
    }
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

/// Truncate a string to at most `max_chars` Unicode characters, appending
/// "..." if truncation occurred.  This is safe for multi-byte UTF-8 strings
/// unlike byte-index slicing (`&s[..n]`), which can panic mid-character.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}
