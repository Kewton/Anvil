//! Agentic tool-use loop extracted from the main app module.
//!
//! Contains the multi-turn structured response execution loop and its
//! helpers.  These are `impl App` methods in a separate file for
//! maintainability — the same pattern used by `mock.rs`.

use crate::agent::subagent::{SubAgentError, SubAgentKind, SubAgentOverrides, SubAgentSession};
use crate::agent::{BasicAgentLoop, StructuredAssistantResponse};
use crate::contracts::{AppStateSnapshot, RuntimeState, ToolLogView};
use crate::provider::{ProviderClient, ProviderEvent};
use crate::session::{MessageRole, SessionMessage};
use crate::spinner::Spinner;
use crate::state::StateTransition;
use crate::tooling::progress::{ToolProgressEntry, ToolProgressStatus};
use crate::tooling::{
    ExecutionMode, LocalToolExecutor, ToolCallRequest, ToolExecutionPayload, ToolExecutionPolicy,
    ToolExecutionRequest, ToolExecutionResult, ToolExecutionStatus, ToolInput, ToolKind,
    count_file_lines, diff::generate_diff_preview, resolve_sandbox_path,
};
use crate::tui::Tui;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::tooling::{PermissionClass, effective_permission_class};

use super::policy::{OFFLINE_BLOCK_PAYLOAD, check_offline_blocked};
use super::read_repeat_tracker::ReadRepeatAction;
use super::read_transition_guard::ReadTransitionAction;
use super::write_repeat_tracker::WriteRepeatAction;
use super::{App, AppError, CompactInfo, format_tool_counts};

/// Turn summary information for structured logging (Issue #206).
#[derive(Debug)]
pub struct TurnSummary<'a> {
    pub turn: u32,
    pub max_turns: u32,
    pub elapsed: std::time::Duration,
    pub tokens_used: usize,
    pub token_budget: usize,
    pub tool_calls: usize,
    pub tool_names: &'a [String],
    pub files_modified: usize,
    pub compact_info: Option<&'a CompactInfo>,
    pub phase: super::phase_estimator::Phase,
    /// Mutations executed this turn.
    pub mutations_this_turn: Option<u32>,
    /// Items advanced this turn.
    pub items_advanced_this_turn: Option<u32>,
}

/// Log a turn summary using structured tracing.
pub fn log_turn_summary(summary: &TurnSummary<'_>) {
    let compact_str = match summary.compact_info {
        None => "no".to_string(),
        Some(info) => match &info.sidecar_model {
            Some(model) => format!(
                "sidecar({}, {}->{}msgs)",
                model, info.before_messages, info.after_messages
            ),
            None => format!(
                "rule-based({}->{}msgs)",
                info.before_messages, info.after_messages
            ),
        },
    };
    let tool_summary = summarize_tool_names(summary.tool_names);
    tracing::info!(
        turn = summary.turn,
        max_turns = summary.max_turns,
        elapsed_s = format!("{:.1}", summary.elapsed.as_secs_f64()),
        tokens = summary.tokens_used,
        budget = summary.token_budget,
        tool_calls = summary.tool_calls,
        tools = %tool_summary,
        files_modified = summary.files_modified,
        compact = %compact_str,
        phase = %summary.phase,
        "turn completed"
    );
}

/// Summarize tool names by counting duplicates (e.g. `"file.read x3, file.edit"`).
pub fn summarize_tool_names(names: &[String]) -> String {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for name in names {
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    format_tool_counts(counts.into_iter())
}

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
    completed: Arc<AtomicUsize>,
    entries: Arc<std::sync::Mutex<Vec<ToolProgressEntry>>>,
    file_cache: Option<std::sync::Arc<std::sync::Mutex<crate::tooling::file_cache::FileReadCache>>>,
) -> Vec<(usize, ToolExecutionResult)> {
    let cwd = config.paths.cwd.clone();
    let runtime = &config.runtime;
    let mut all_results = Vec::new();

    let mut chunk_offset = 0usize;
    for chunk in requests.chunks(MAX_PARALLEL_THREADS) {
        let current_offset = chunk_offset;
        all_results.extend(std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .enumerate()
                .map(|(pos_in_chunk, (idx, request))| {
                    let cwd = cwd.clone();
                    let shutdown = shutdown_flag.clone();
                    let idx = *idx;
                    let completed = completed.clone();
                    let entries = entries.clone();
                    let entry_index = current_offset + pos_in_chunk;
                    let file_cache = file_cache.clone();
                    s.spawn(move || {
                        // Update started_at to actual execution start time
                        if let Ok(mut guard) = entries.lock()
                            && let Some(entry) = guard.get_mut(entry_index)
                        {
                            entry.started_at = std::time::Instant::now();
                        }
                        let mut executor = LocalToolExecutor::new(cwd, runtime, file_cache)
                            .with_shutdown_flag(shutdown);
                        let tool_call_id = request.tool_call_id.clone();
                        let tool_name = request.spec.name.clone();
                        let result = executor.execute(request.clone()).unwrap_or_else(|err| {
                            let (summary, payload) = match &err {
                                crate::tooling::ToolRuntimeError::EditNotFound {
                                    message,
                                    context_snippet,
                                } => {
                                    let payload_text = if let Some(ctx) = context_snippet {
                                        format!(
                                            "{message}\n\n--- File context (nearby lines) ---\n{ctx}"
                                        )
                                    } else {
                                        message.clone()
                                    };
                                    (
                                        message.clone(),
                                        ToolExecutionPayload::Text(payload_text),
                                    )
                                }
                                other => {
                                    let msg = other.to_string();
                                    (msg.clone(), ToolExecutionPayload::Text(msg))
                                }
                            };
                            ToolExecutionResult {
                                tool_call_id,
                                tool_name,
                                status: ToolExecutionStatus::Failed,
                                summary,
                                payload,
                                artifacts: Vec::new(),
                                elapsed_ms: 0,
                                diff_summary: None,
                                edit_detail: None,
                                rolled_back: false,
                            }
                        });
                        // Update progress entry
                        if let Ok(mut guard) = entries.lock()
                            && let Some(entry) = guard.get_mut(entry_index)
                        {
                            let elapsed =
                                entry.started_at.elapsed().as_millis().min(u64::MAX as u128)
                                    as u64;
                            entry.elapsed_ms = Some(elapsed);
                            entry.status =
                                if result.status == ToolExecutionStatus::Completed {
                                    ToolProgressStatus::Completed
                                } else {
                                    ToolProgressStatus::Failed
                                };
                        }
                        completed.fetch_add(1, Ordering::Relaxed);
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
                                diff_summary: None,
                                edit_detail: None,
                                rolled_back: false,
                            },
                        ));
                    }
                }
            }
            results
        }));
        chunk_offset += chunk.len();
    }

    all_results
}

/// Maximum number of ANVIL_FINAL guard retries (no-file-modification detection).
const MAX_FINAL_GUARD_RETRIES: u8 = 1;
const FILE_READ_RESULT_MAX_CHARS: usize = 2_000;
const SYNTHETIC_GUIDANCE_RESULT_MAX_CHARS: usize = 1_200;

/// Message sent to LLM when ANVIL_FINAL fires without file modifications.
const FINAL_GUARD_RETRY_MESSAGE: &str = "No file modifications detected (file.write/file.edit not called). \
     Please implement the changes rather than just planning them. \
     Use file.write or file.edit to make the necessary code changes.";

/// Synthetic guidance tools that must be shown to the model before accepting
/// an ANVIL_FINAL termination.
const GUIDANCE_TOOL_NAMES: &[&str] = &[
    "system.read_guard",
    "system.loop_detector",
    "system.phase_estimator",
];

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
                    diff_summary: None,
                    edit_detail: None,
                    rolled_back: false,
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

            let overrides = SubAgentOverrides {
                model: self.active_model.clone(),
                context_window: self.active_context_window,
            };
            let session = SubAgentSession::new(
                kind,
                prompt,
                &scope_path,
                provider_client,
                &self.config,
                self.shutdown_flag(),
                overrides,
            );
            let result = session.run();
            agent_results.push(match result {
                Ok(r) => r.into_tool_execution_result(call),
                Err(e) => e.into_tool_execution_result(call),
            });
        }

        (agent_results, normal_calls)
    }

    /// Check if the ANVIL_FINAL guard should activate.
    /// Returns true if no file modifications were detected and retries remain.
    fn should_activate_final_guard(&self, retries: u8) -> bool {
        retries < MAX_FINAL_GUARD_RETRIES && self.session.working_memory.touched_files.is_empty()
    }

    /// Check whether synthetic guidance was injected in the current turn and
    /// should be shown to the model before honoring ANVIL_FINAL.
    fn should_activate_guidance_retry(
        &self,
        already_used: bool,
        results: &[ToolExecutionResult],
    ) -> bool {
        !already_used
            && self.session.working_memory.touched_files.is_empty()
            && results
                .iter()
                .any(|r| GUIDANCE_TOOL_NAMES.contains(&r.tool_name.as_str()))
    }

    /// Inject a retry message into the session to prompt the LLM for actual implementation.
    /// See also: PROMPT_TOOL_RULES in src/agent/mod.rs for preventive guidance.
    fn inject_final_guard_retry(&mut self) {
        tracing::warn!("ANVIL_FINAL guard: no file modifications detected, retrying");
        let retry_msg = SessionMessage::new(
            MessageRole::Tool,
            "system",
            FINAL_GUARD_RETRY_MESSAGE.to_string(),
        )
        .with_id(self.next_message_id("tool"));
        self.session.push_message(retry_msg);
    }

    /// Execute tool calls and feed results back to the LLM in a loop.
    ///
    /// This implements the agentic tool-use loop:
    /// 1. Execute tool calls from the structured response
    /// 2. Record results (with full payload) into the session
    /// 3. Send updated session back to the LLM
    /// 4. If the LLM responds with more tool calls, repeat (up to MAX_AGENT_ITERATIONS)
    /// 5. When the LLM responds without tool calls, record as final answer
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete_structured_response<C: ProviderClient>(
        &mut self,
        structured: StructuredAssistantResponse,
        status: &str,
        saved_status: &str,
        elapsed_ms: u128,
        inference_performance: Option<crate::contracts::InferencePerformanceView>,
        tui: &Tui,
        provider_client: &C,
        anvil_final_already: bool,
    ) -> Result<Vec<String>, AppError> {
        let max_iterations = self.config.runtime.max_agent_iterations;
        let mut current = structured;
        let mut frames = Vec::new();
        let mut total_tool_count = 0usize;
        let mut all_tool_log_views: Vec<ToolLogView> = Vec::new();
        let mut final_guard_retries: u8 = 0;
        let mut fallback_completed = false;
        let mut guidance_retry_used = false;
        let mut awaiting_guidance_followup = false;
        // Issue #173: Track whether ANVIL_FINAL has been seen in this session.
        // When true, the loop will terminate after executing the current batch
        // of tool calls (no further LLM round-trips).
        let mut anvil_final_seen = anvil_final_already || current.anvil_final_detected;

        // Reset loop detector at the start of each top-level turn (Issue #145)
        self.loop_detector.reset();
        // Reset alternating loop detector per-turn (Issue #172)
        self.alternating_loop_detector.reset();
        // Reset phase estimator per-turn counters (Issue #159)
        self.phase_estimator.reset();
        // Reset read transition guard per-turn counters (Issue #216)
        self.read_transition_guard.reset();
        // Reset execution plan per user-turn (Issue #249)
        self.reset_execution_plan();

        // Issue #249: Detect ANVIL_PLAN from initial response
        self.try_register_plan(&current.raw_content);

        // Session note extraction bookkeeping (Issue #241)
        let msg_count_before = self.session.messages.len();
        let tokens_before = self.session.estimated_token_count();

        for iteration in 0..max_iterations {
            let iteration_started = std::time::Instant::now();

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
                anvil_final_detected: current.anvil_final_detected,
                raw_content: current.raw_content.clone(),
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
            let (results, loop_action) = self.execute_structured_tool_calls(&current_normal)?;
            total_tool_count += results.len();

            // Update plan item status from tool results; capture telemetry.
            let (turn_mutations, turn_items_advanced) = self.update_plan_from_results(&results);

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

            // Handle loop detection Break action (Issue #145)
            // Results are already recorded and visible above; now terminate the loop.
            if let super::loop_detector::LoopAction::Break(ref msg) = loop_action {
                tracing::warn!(reason = "loop_detected", message = %msg, "agentic loop terminated by loop detector");
                break;
            }

            // Issue #173: If ANVIL_FINAL was already seen, terminate after
            // executing the current tool batch (no further LLM round-trips).
            if anvil_final_seen {
                // Issue #249: Plan-aware ANVIL_FINAL gate — suppress if plan is incomplete
                if self.check_plan_final_gate() {
                    tracing::info!("ANVIL_FINAL suppressed by plan gate; continuing execution");
                    anvil_final_seen = false;
                } else if self.should_activate_guidance_retry(guidance_retry_used, &results) {
                    tracing::info!(
                        "ANVIL_FINAL delayed: synthetic guidance injected, sending one follow-up turn"
                    );
                    guidance_retry_used = true;
                    awaiting_guidance_followup = true;
                    anvil_final_seen = false;
                } else {
                    tracing::info!("ANVIL_FINAL detected; terminating after tool execution");
                    self.phase_estimator.accept_anvil_final();
                    break;
                }
            }

            // Max tool calls check (Issue #172)
            if total_tool_count >= self.config.runtime.max_tool_calls {
                tracing::warn!(
                    "Max tool calls reached ({}/{}). Terminating agentic loop.",
                    total_tool_count,
                    self.config.runtime.max_tool_calls
                );
                break;
            }

            // Check shutdown flag before LLM call
            if self.is_shutdown_requested() {
                break;
            }

            // Issue #249: Inject plan turn guidance before follow-up LLM call
            self.inject_plan_turn_guidance();

            // Send tool results back to LLM for the next turn
            let spinner = Spinner::start(
                format!(
                    "Analyzing results. model={} (iteration {})",
                    self.effective_model(),
                    iteration + 2
                ),
                self.config.mode.interactive,
            );

            let (system_prompt, calibration_ratio) = self.prepare_turn_context();
            let (mut request, used_tokens) = BasicAgentLoop::build_turn_request_calibrated(
                self.effective_model().to_string(),
                &self.session,
                self.provider.capabilities.streaming && self.config.runtime.stream,
                self.effective_context_window(),
                &system_prompt,
                calibration_ratio,
                self.config.runtime.context_budget,
            );
            request.max_output_tokens = self.config.runtime.max_output_tokens;

            // Budget pressure WARN (Issue #206 D-2)
            let token_budget = self.effective_token_budget();
            if token_budget > 0 {
                let usage_ratio = used_tokens as f64 / token_budget as f64;
                if usage_ratio >= 0.9 {
                    tracing::warn!(
                        used = used_tokens,
                        budget = token_budget,
                        ratio = format!("{:.1}%", usage_ratio * 100.0),
                        "budget pressure"
                    );
                }
            }

            // Collect tool names for this iteration's turn summary (before LLM call)
            let turn_tool_names: Vec<String> =
                results.iter().map(|r| r.tool_name.clone()).collect();
            let turn_files_modified = results
                .iter()
                .filter(|r| r.diff_summary.is_some() && !r.rolled_back)
                .count();
            let turn_tool_count = results.len() + agent_results.len();

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
                Err(
                    ref err @ crate::provider::ProviderTurnError::ConnectionRefused(_)
                    | ref err @ crate::provider::ProviderTurnError::DnsFailure(_)
                    | ref err @ crate::provider::ProviderTurnError::AuthenticationFailed { .. }
                    | ref err @ crate::provider::ProviderTurnError::ModelNotFound { .. },
                ) => {
                    return Err(AppError::ProviderTurn(err.clone()));
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
            let next_structured = match BasicAgentLoop::parse_structured_response_with_registry(
                &next_token_buffer,
                &self.tools,
            ) {
                Ok(parsed) => parsed,
                Err(first_err) => {
                    // LLMs occasionally produce malformed output; treat the
                    // raw text as a plain final answer rather than failing the
                    // entire turn.
                    let trimmed = next_token_buffer.trim();
                    if !trimmed.is_empty() {
                        StructuredAssistantResponse::empty(trimmed.to_string())
                    } else {
                        return Err(AppError::ToolExecution(first_err));
                    }
                }
            };

            // Record turn stats AFTER the full iteration completes (Issue #206 CB-001)
            self.session_stats.record_turn();
            self.agent_telemetry
                .record_turn_metrics(turn_mutations, turn_items_advanced, 0, 0);
            log_turn_summary(&TurnSummary {
                turn: self.session_stats.total_turns,
                max_turns: max_iterations as u32,
                elapsed: iteration_started.elapsed(),
                tokens_used: used_tokens,
                token_budget,
                tool_calls: turn_tool_count,
                tool_names: &turn_tool_names,
                files_modified: turn_files_modified,
                compact_info: self.last_compact_info.as_ref(),
                phase: self.phase_estimator.current_phase(),
                mutations_this_turn: Some(turn_mutations),
                items_advanced_this_turn: Some(turn_items_advanced),
            });
            // Reset last_compact_info after it's been consumed by the turn summary
            self.last_compact_info = None;

            // Issue #249: Detect ANVIL_PLAN / ANVIL_PLAN_UPDATE from follow-up responses.
            // Scan the raw token buffer since ANVIL_PLAN may be outside the ANVIL_FINAL block.
            self.try_register_plan(&next_token_buffer);
            self.try_update_plan(&next_token_buffer);

            // Issue #173: Update ANVIL_FINAL tracking from the new response
            if next_structured.anvil_final_detected {
                anvil_final_seen = true;
                self.phase_estimator.observe_anvil_final();
            }

            if next_structured.tool_calls.is_empty() {
                if awaiting_guidance_followup {
                    awaiting_guidance_followup = false;
                    if self.should_activate_final_guard(final_guard_retries) {
                        tracing::info!(
                            "Guidance follow-up ended without edits; escalating to final guard retry"
                        );
                        self.inject_final_guard_retry();
                        final_guard_retries += 1;
                        current = next_structured;
                        continue;
                    }
                    tracing::info!("Guidance follow-up completed; accepting response");
                    self.record_assistant_output(
                        self.next_message_id("assistant"),
                        next_structured.final_response,
                    )?;
                    break;
                }
                // ANVIL_FINAL guard: check if any file modifications were made
                if self.should_activate_final_guard(final_guard_retries) {
                    self.inject_final_guard_retry();
                    final_guard_retries += 1;
                    current = next_structured;
                    continue;
                }

                // Fallback completion detection (Issue #159):
                // When ANVIL_FINAL was never observed, check if tool patterns
                // indicate the agent has finished (write succeeded + verification reads + empty response).
                if let super::phase_estimator::PhaseAction::FallbackComplete =
                    self.phase_estimator.check_empty_response()
                {
                    tracing::info!("Phase estimator: fallback completion detected");
                    self.record_assistant_output(
                        self.next_message_id("assistant"),
                        next_structured.final_response,
                    )?;
                    fallback_completed = true;
                    break;
                }

                // Issue #249: Plan gate — suppress termination if plan is incomplete
                if self.check_plan_final_gate() {
                    tracing::info!("Plan gate: suppressing termination with empty tool calls");
                    current = next_structured;
                    continue;
                }

                // No more tool calls — this is the final answer
                // Issue #261 Task 0.4: Mark as accepted (not suppressed)
                if anvil_final_seen {
                    self.phase_estimator.accept_anvil_final();
                }
                self.record_assistant_output(
                    self.next_message_id("assistant"),
                    next_structured.final_response,
                )?;
                break;
            }

            // More tool calls — continue the loop
            if awaiting_guidance_followup {
                awaiting_guidance_followup = false;
            }
            current = next_structured;
        }

        // Issue #255: Classify completion kind based on plan state.
        {
            let budget_exhausted = total_tool_count >= self.config.runtime.max_tool_calls;
            let completion_kind = crate::contracts::CompletionKind::classify(
                &self.execution_plan,
                None, // verify not yet implemented (Stage 3)
                budget_exhausted,
            );
            self.agent_telemetry.completion_kind = Some(completion_kind);
            tracing::info!(
                completion_kind = %completion_kind,
                plan_items = self.execution_plan.items.len(),
                plan_finished = self.execution_plan.finished_count(),
                "agentic loop completion classified"
            );
        }

        // Transition to Done
        let mut done = AppStateSnapshot::new(RuntimeState::Done)
            .with_status(status.to_string())
            .with_tool_logs(all_tool_log_views)
            .with_completion_summary(
                if fallback_completed {
                    format!(
                        "Executed {} tool call(s) across agentic loop (fallback completion). {}",
                        total_tool_count, saved_status
                    )
                } else {
                    format!(
                        "Executed {} tool call(s) across agentic loop. {}",
                        total_tool_count, saved_status
                    )
                },
                saved_status.to_string(),
            )
            .with_elapsed_ms(elapsed_ms);
        if let Some(perf) = inference_performance {
            done = done.with_inference_performance(perf);
        }
        let mut done_snapshot = self.transition_with_context(done, StateTransition::Finish)?;
        self.evaluate_context_warning(&mut done_snapshot);
        frames.push(self.render_console(tui)?);

        // Session note extraction (Issue #241)
        let tokens_after = self.session.estimated_token_count();
        let context_window = self.effective_context_window() as usize;
        let token_delta = tokens_after.saturating_sub(tokens_before);
        if total_tool_count >= 5 || token_delta >= context_window / 10 {
            let turn_messages = &self.session.messages[msg_count_before..];
            let notes = crate::session::extract_session_notes(turn_messages);
            for note in &notes {
                tracing::info!(
                    kind = %note.kind,
                    files = ?note.files,
                    "session_note: {}",
                    note.summary
                );
            }
        }

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
                        diff_summary: None,
                        edit_detail: None,
                        rolled_back: false,
                    },
                ));
                continue;
            }

            if self.config.mode.approval_required && validated.approval_required(true).is_some() {
                let effective_perm = effective_permission_class(&call.input, &validated.spec);
                if is_trusted(
                    &call.tool_name,
                    validated.spec.kind,
                    effective_perm,
                    self.trust_all,
                    &self.trusted_tools,
                ) {
                    let summary = tool_call_approval_summary(call);
                    let _ = std::io::Write::write_fmt(
                        &mut std::io::stderr(),
                        format_args!("\n  [trusted] {summary}\n"),
                    );
                } else {
                    let summary = tool_call_approval_summary(call);
                    let diff_options =
                        crate::tooling::diff::DiffOptions::from_runtime(&self.config.runtime);
                    let diff_preview =
                        generate_diff_preview(&self.config.paths.cwd, &call.input, &diff_options);
                    let approved = prompt_inline_approval(&summary, diff_preview.as_deref());
                    if !approved {
                        failed_results
                            .push((idx, build_failed_result(call, "denied by user".to_string())));
                        continue;
                    }
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

        // Capture checkpoint before file-mutating tools (Issue #68)
        let cwd = self.config.paths.cwd.clone();
        let checkpoint_idx = self
            .capture_checkpoint_if_needed(&request, &cwd)
            .map(|entry| self.checkpoint_stack.push(entry));

        // Built-in tools: delegate to LocalToolExecutor
        let mut executor = LocalToolExecutor::new(
            cwd,
            &self.config.runtime,
            Some(self.file_read_cache.clone()),
        )
        .with_shutdown_flag(self.shutdown_flag());

        let result = executor
            .execute(request)
            .unwrap_or_else(|err| ToolExecutionResult {
                tool_call_id: String::new(),
                tool_name: String::new(),
                status: ToolExecutionStatus::Failed,
                summary: err.to_string(),
                payload: ToolExecutionPayload::Text(err.to_string()),
                artifacts: Vec::new(),
                elapsed_ms: 0,
                diff_summary: None,
                edit_detail: None,
                rolled_back: false,
            });

        // Remove checkpoint if tool execution failed.
        // During a transaction, skip individual removal — rollback_to_mark()
        // will handle bulk cleanup.
        if result.status == ToolExecutionStatus::Failed
            && let Some(idx) = checkpoint_idx
            && !self.checkpoint_stack.is_in_transaction()
        {
            self.checkpoint_stack.remove(idx);
        }

        result
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
                diff_summary: None,
                edit_detail: None,
                rolled_back: false,
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
            diff_summary: None,
            edit_detail: None,
            rolled_back: false,
        }
    }

    pub(crate) fn execute_structured_tool_calls(
        &mut self,
        structured: &StructuredAssistantResponse,
    ) -> Result<(Vec<ToolExecutionResult>, super::loop_detector::LoopAction), AppError> {
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
                                    diff_summary: None,
                                    edit_detail: None,
                                    rolled_back: false,
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

        // Loop detection: record each validated tool call and check for repetition (Issue #145, #172)
        let mut worst_loop_action = super::loop_detector::LoopAction::Continue;
        for (_, req) in &validated_requests {
            if let Some((tool_name, tool_input)) = tool_input_map.get(&req.tool_call_id) {
                // LoopDetector: same-call repetition
                let action = self.loop_detector.record_and_check(tool_name, tool_input);
                worst_loop_action = worst_loop_action.merge(action);
                // AlternatingLoopDetector: cyclic pattern detection (Issue #172)
                let alt_action = self
                    .alternating_loop_detector
                    .record_and_check(tool_name, tool_input);
                worst_loop_action = worst_loop_action.merge(alt_action);
            }
        }

        // If Break: skip tool execution entirely
        if matches!(
            worst_loop_action,
            super::loop_detector::LoopAction::Break(_)
        ) {
            // Return failed results with the Break action
            failed_results.sort_by_key(|(idx, _)| *idx);
            let results: Vec<ToolExecutionResult> =
                failed_results.into_iter().map(|(_, r)| r).collect();
            return Ok((results, worst_loop_action));
        }

        // For Warn/StrongWarn: create synthetic tool result to include in results
        let synthetic_warning = match &worst_loop_action {
            super::loop_detector::LoopAction::Warn(msg)
            | super::loop_detector::LoopAction::StrongWarn(msg) => Some(ToolExecutionResult {
                tool_call_id: "loop_detector_warning".to_string(),
                tool_name: "system.loop_detector".to_string(),
                status: ToolExecutionStatus::Completed,
                summary: msg.clone(),
                payload: ToolExecutionPayload::Text(msg.clone()),
                artifacts: vec![],
                elapsed_ms: 0,
                diff_summary: None,
                edit_detail: None,
                rolled_back: false,
            }),
            _ => None,
        };

        // Phase 2: Grouping
        let groups = group_by_execution_mode(&validated_requests);

        // Transaction begin: mark the checkpoint stack position before execution.
        let mark = self.checkpoint_stack.mark();

        // Build tool_call_id → ToolKind map for rollback judgment.
        let tool_kind_map: std::collections::HashMap<String, ToolKind> = validated_requests
            .iter()
            .map(|(_, req)| (req.tool_call_id.clone(), req.spec.kind))
            .collect();

        // Phase 3: Execution
        let total_tools = validated_requests.len();
        let interactive = self.config.mode.interactive;
        let mut indexed_results: Vec<(usize, ToolExecutionResult)> = failed_results;
        let mut seq_counter = 0usize;
        for group in groups {
            match group {
                ExecutionGroup::Parallel(requests) if requests.len() >= 2 => {
                    let completed = Arc::new(AtomicUsize::new(0));
                    let progress_entries: Vec<ToolProgressEntry> = requests
                        .iter()
                        .map(|(_, req)| ToolProgressEntry {
                            tool_name: req.spec.name.clone(),
                            status: ToolProgressStatus::Running,
                            started_at: std::time::Instant::now(),
                            elapsed_ms: None,
                        })
                        .collect();
                    let entries = Arc::new(std::sync::Mutex::new(progress_entries));
                    let spinner = Spinner::start_parallel_detailed(
                        entries.clone(),
                        completed.clone(),
                        interactive,
                    );
                    let parallel_results = execute_parallel_group_standalone(
                        &self.config,
                        self.shutdown_flag(),
                        requests,
                        completed,
                        entries,
                        Some(self.file_read_cache.clone()),
                    );
                    spinner.stop();
                    seq_counter += parallel_results.len();
                    indexed_results.extend(parallel_results);
                }
                ExecutionGroup::Parallel(requests) => {
                    // Single ParallelSafe item — execute sequentially
                    for (idx, req) in requests {
                        seq_counter += 1;
                        let spinner = Spinner::start_tool(
                            &req.spec.name,
                            total_tools,
                            seq_counter,
                            interactive,
                        );
                        if req.spec.kind.produces_stderr() {
                            spinner.pause();
                        }
                        let result = self.execute_single(req);
                        spinner.stop();
                        indexed_results.push((idx, result));
                    }
                }
                ExecutionGroup::Sequential(idx, req) => {
                    seq_counter += 1;
                    let spinner =
                        Spinner::start_tool(&req.spec.name, total_tools, seq_counter, interactive);
                    if req.spec.kind.produces_stderr() {
                        spinner.pause();
                    }
                    let result = self.execute_single(req);
                    spinner.stop();
                    indexed_results.push((idx, result));
                }
            }
        }

        // Helper: check whether a tool_call_id refers to a file-mutating tool.
        let is_file_mutation = |tool_call_id: &str| -> bool {
            tool_kind_map.get(tool_call_id).is_some_and(|kind| {
                matches!(
                    kind,
                    ToolKind::FileWrite | ToolKind::FileEdit | ToolKind::FileEditAnchor
                )
            })
        };

        // Transaction check: determine if any file-mutating tool failed.
        let has_file_mutation_failure = indexed_results.iter().any(|(_, r)| {
            r.status == ToolExecutionStatus::Failed && is_file_mutation(&r.tool_call_id)
        });

        let rolled_back_ids: std::collections::HashSet<String> = if has_file_mutation_failure {
            let entries = self.checkpoint_stack.rollback_to_mark(mark);
            let restore_results: Vec<_> = entries.iter().map(|e| e.restore()).collect();
            let restored_count = restore_results
                .iter()
                .filter(|r| r.action != crate::tooling::RestoreAction::Skipped)
                .count();
            let failed_restore_count = restore_results.len() - restored_count;

            // Count failed file-mutating tools for the notification message
            let failed_count = indexed_results
                .iter()
                .filter(|(_, r)| {
                    r.status == ToolExecutionStatus::Failed && is_file_mutation(&r.tool_call_id)
                })
                .count();

            // Build set of tool_call_ids that were involved in file mutations
            // (both successful and failed) -- all are rolled back
            let rb_ids: std::collections::HashSet<String> = indexed_results
                .iter()
                .filter(|(_, r)| is_file_mutation(&r.tool_call_id))
                .map(|(_, r)| r.tool_call_id.clone())
                .collect();

            // Annotate successful file-mutating results with rollback info (Issue #259)
            for (_, r) in &mut indexed_results {
                if r.status == ToolExecutionStatus::Completed && rb_ids.contains(&r.tool_call_id) {
                    r.summary = format!("{} [rolled back: atomic transaction failed]", r.summary);
                    r.rolled_back = true;
                }
            }

            // Record system message for LLM awareness
            let rollback_msg = format!(
                "[System] Atomic transaction rolled back. {} file(s) restored to pre-transaction state \
                 ({} restore failures). Reason: {} file tool(s) failed. \
                 All file changes in this turn have been reverted.",
                restored_count, failed_restore_count, failed_count
            );
            self.session.push_message(
                SessionMessage::new(MessageRole::System, "system", rollback_msg)
                    .with_id(self.next_message_id("system")),
            );

            rb_ids
        } else {
            self.checkpoint_stack.commit_mark();
            std::collections::HashSet::new()
        };

        // Phase 4: Sort by index, record results, run PostToolUse hooks (DR2-004)
        indexed_results.sort_by_key(|(idx, _)| *idx);
        let mut results: Vec<ToolExecutionResult> = Vec::with_capacity(indexed_results.len());
        let mut phase_action = super::phase_estimator::PhaseAction::Continue;
        let mut read_transition_message: Option<String> = None;
        for (_, result) in indexed_results {
            self.record_tool_result(&result);
            // Phase estimator: record tool call pattern (Issue #159)
            let success = result.status == ToolExecutionStatus::Completed;
            let pa = self
                .phase_estimator
                .record_tool_call(&result.tool_name, success);
            if !matches!(pa, super::phase_estimator::PhaseAction::Continue) {
                phase_action = pa;
            }
            // Issue #265: pass shell command to read_transition_guard so
            // grep/sed/cat are counted as exploration calls.
            let shell_cmd: Option<String> = if result.tool_name == "shell.exec" {
                tool_input_map
                    .get(&result.tool_call_id)
                    .and_then(|(_, v)| v.get("command").and_then(|c| c.as_str()).map(String::from))
            } else {
                None
            };
            let transition_action = self.read_transition_guard.record_tool_call_ex(
                &result.tool_name,
                success,
                shell_cmd.as_deref(),
            );
            if let ReadTransitionAction::Inject(msg) = transition_action {
                read_transition_message = Some(msg);
            }

            // Emit folded tool result to stderr for interactive sessions
            if interactive {
                let output_text = match &result.payload {
                    ToolExecutionPayload::Text(content) => content.as_str(),
                    ToolExecutionPayload::Paths(_) => "",
                    _ => "",
                };
                let display = crate::app::render::format_tool_result_for_display(
                    &result.tool_name,
                    &result.summary,
                    output_text,
                    result.elapsed_ms.min(u64::MAX as u128) as u64,
                );
                let _ =
                    std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("{display}\n"));
            }

            // PostToolUse hook (soft-fail) — skip for rolled-back results
            if rolled_back_ids.contains(&result.tool_call_id) {
                results.push(result);
                continue;
            }
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

        // Append synthetic loop detection warning if present
        if let Some(warning_result) = synthetic_warning {
            self.record_tool_result(&warning_result);
            results.push(warning_result);
        }

        if let Some(ref msg) = read_transition_message {
            let transition_result = ToolExecutionResult {
                tool_call_id: "read_transition_guard".to_string(),
                tool_name: "system.read_guard".to_string(),
                status: ToolExecutionStatus::Completed,
                summary: msg.clone(),
                payload: ToolExecutionPayload::Text(msg.clone()),
                artifacts: vec![],
                elapsed_ms: 0,
                diff_summary: None,
                edit_detail: None,
                rolled_back: false,
            };
            self.record_tool_result(&transition_result);
            results.push(transition_result);
        }

        // Phase estimator: inject force-transition message if needed (Issue #159).
        // Skip if LoopDetector or read transition guard already issued a warning.
        if matches!(
            worst_loop_action,
            super::loop_detector::LoopAction::Continue
        ) && read_transition_message.is_none()
            && let super::phase_estimator::PhaseAction::ForceTransition(msg) = phase_action
        {
            // Context window protection: skip if usage >= 90%
            let usage = crate::contracts::ContextUsageView {
                estimated_tokens: self.session.estimated_token_count(),
                max_tokens: self.effective_context_window(),
            };
            if usage.usage_ratio() < 0.9 {
                let transition_result = ToolExecutionResult {
                    tool_call_id: "phase_estimator_transition".to_string(),
                    tool_name: "system.phase_estimator".to_string(),
                    status: ToolExecutionStatus::Completed,
                    summary: msg.clone(),
                    payload: ToolExecutionPayload::Text(msg),
                    artifacts: vec![],
                    elapsed_ms: 0,
                    diff_summary: None,
                    edit_detail: None,
                    rolled_back: false,
                };
                self.record_tool_result(&transition_result);
                results.push(transition_result);
            }
        }

        self.persist_session(crate::contracts::AppEvent::SessionSaved)?;
        Ok((results, worst_loop_action))
    }

    /// Push a tool execution result into the session as a tool message.
    fn record_tool_result(&mut self, result: &ToolExecutionResult) {
        // Session stats: record tool call (Issue #206 C-3)
        self.session_stats.record_tool_call(&result.tool_name);

        // Session stats: record file change line counts from diff_summary (Issue #206 C-3, #259)
        // Skip rolled-back results to avoid counting reverted changes.
        if !result.rolled_back
            && let Some(ref diff) = result.diff_summary
        {
            let (added, deleted) = super::count_diff_lines(diff);
            self.session_stats.record_file_change(added, deleted);
            for artifact in &result.artifacts {
                self.session_stats.files_modified.insert(artifact.clone());
            }
        }

        // Track tool usage for dynamic system prompt generation (Issue #73)
        self.session.used_tools.insert(result.tool_name.clone());

        // Working memory: track touched files (file-mutating tools only) (Issue #130, #157)
        let is_file_tool = matches!(
            result.tool_name.as_str(),
            "file.write" | "file.edit" | "file.edit_anchor"
        );
        if is_file_tool
            && result.status == ToolExecutionStatus::Completed
            && !result.summary.contains("[rolled back]")
        {
            for artifact in &result.artifacts {
                let path = std::path::Path::new(artifact);
                if let Some(rel) = self.relative_path_for_working_memory(path) {
                    self.session.working_memory.update_touched_files(&rel);
                } else {
                    tracing::warn!("skip touched_files update for non-relative artifact");
                }
            }

            // Update recent_diffs with diff_summary (Issue #157)
            if let Some(ref diff) = result.diff_summary {
                let current = self
                    .session
                    .working_memory
                    .recent_diffs
                    .clone()
                    .unwrap_or_default();
                let updated = if current.is_empty() {
                    diff.clone()
                } else {
                    format!("{}\n{}", current, diff)
                };
                self.session.working_memory.set_recent_diffs(Some(updated));
            }
        }

        // Edit fail tracker: track consecutive file.edit/file.edit_anchor failures (Issue #143, #158)
        let mut edit_hint: Option<String> = None;
        if result.tool_name == "file.edit" || result.tool_name == "file.edit_anchor" {
            if result.status == ToolExecutionStatus::Failed {
                if let Some(raw_path) = extract_edit_path_from_summary(&result.summary) {
                    let path = resolve_edit_tracker_path(&raw_path);
                    let action = self.edit_fail_tracker.record_failure(&path);
                    let count = self.edit_fail_tracker.failure_count(&path);
                    match action {
                        crate::app::edit_fail_tracker::EditFallbackAction::Continue => {}
                        crate::app::edit_fail_tracker::EditFallbackAction::ReRead => {
                            tracing::warn!(
                                tool = "file.edit",
                                path = %path,
                                count = count,
                                "repeated tool failure"
                            );
                            edit_hint = Some(format!(
                                "\n\n[Anvil hint] file.edit has failed {count} consecutive \
                                 times for '{path}'. Use file.read to get the current file \
                                 content, then retry file.edit with the correct old_string."
                            ));
                        }
                        crate::app::edit_fail_tracker::EditFallbackAction::WriteFallback => {
                            tracing::warn!(
                                tool = "file.edit",
                                path = %path,
                                count = count,
                                "repeated tool failure - suggesting write fallback"
                            );
                            self.prepare_write_fallback();
                            let max_lines = self.config.runtime.safe_write_max_lines;
                            let line_count = if max_lines > 0 {
                                resolve_sandbox_path(&self.config.paths.cwd, &path)
                                    .ok()
                                    .and_then(|resolved| count_file_lines(&resolved).ok())
                            } else {
                                None
                            };
                            let is_large = line_count.is_some_and(|lines| lines > max_lines);
                            let line_count_check_failed = max_lines > 0 && line_count.is_none();

                            if is_large || line_count_check_failed {
                                edit_hint = Some(format!(
                                    "\n\n[Anvil hint] file.edit has failed {count} consecutive \
                                     times for '{path}'. file.write is not available or could not be \
                                     safely validated for this path. Instead: \
                                     (1) Use file.read to get the current content. \
                                     (2) Identify the exact section to change. \
                                     (3) Retry file.edit with a smaller, precise old_string."
                                ));
                            } else {
                                edit_hint = Some(format!(
                                    "\n\n[Anvil hint] file.edit has failed {count} consecutive \
                                     times for '{path}'. Consider using file.read to get the \
                                     current content, then file.write to replace the entire file \
                                     with the corrected version."
                                ));
                            }
                        }
                    }
                }
            } else if result.status == ToolExecutionStatus::Completed {
                // Reset on success — use consistent path resolution
                if let Some(artifact) = result.artifacts.first() {
                    let path_str = resolve_edit_tracker_path(artifact);
                    self.edit_fail_tracker.record_success(&path_str);
                    self.write_repeat_tracker.reset_for_path(&path_str);
                }
            }
        }

        // Write repeat tracker: reset on file.read success for the same path
        if result.tool_name == "file.read"
            && result.status == ToolExecutionStatus::Completed
            && let Some(artifact) = result.artifacts.first()
        {
            let path = resolve_edit_tracker_path(artifact);
            self.write_repeat_tracker.reset_for_path(&path);
        }

        // Write fail tracker: track consecutive file.write failures (Issue #161)
        let write_hint = self.update_write_trackers(result);

        // Read repeat tracker: track repeated file.read calls (Issue #185)
        let mut read_hint: Option<String> = None;
        if result.tool_name == "file.read"
            && result.status == ToolExecutionStatus::Completed
            && let Some(path) = result.artifacts.first()
        {
            let action = self.read_repeat_tracker.record_read(path);
            match action {
                ReadRepeatAction::Warn(count) => {
                    tracing::warn!(
                        path = %path,
                        count = count,
                        "same file read repeatedly"
                    );
                    read_hint = Some(format!(
                        "\n\n[Anvil hint] You have read {} {} times. \
                         The file content is cached and unchanged — \
                         refer to the content already in your context \
                         instead of re-reading.",
                        path, count
                    ));
                }
                ReadRepeatAction::StrongWarn(count) => {
                    tracing::warn!(
                        path = %path,
                        count = count,
                        "same file read repeatedly (strong)"
                    );
                    read_hint = Some(format!(
                        "\n\n[Anvil hint] You have read {} {} times, \
                         wasting iterations. Refer to the content \
                         already in your context instead of re-reading.",
                        path, count
                    ));
                }
                ReadRepeatAction::Continue => {}
            }
        }

        // Reset read repeat tracker on any file-mutation success (Issue #185)
        if is_file_tool
            && result.status == ToolExecutionStatus::Completed
            && let Some(raw_path) = result.artifacts.first()
        {
            let path = resolve_edit_tracker_path(raw_path);
            self.read_repeat_tracker.reset(&path);
        }

        // Working memory: track errors (Issue #130)
        if result.status == ToolExecutionStatus::Failed {
            let sanitized_error = if result.tool_name == "shell.exec" {
                "shell.exec: command failed (details redacted)".to_string()
            } else {
                format!(
                    "{}: {}",
                    result.tool_name,
                    crate::session::sanitize_for_prompt_entry(&result.summary)
                )
            };
            self.session.working_memory.add_error(sanitized_error);
        }

        let is_error = result.status == ToolExecutionStatus::Failed;
        let mut formatted =
            format_tool_result_message(result, self.config.runtime.tool_result_max_chars);
        // Append edit recovery hint if consecutive failures detected
        if let Some(hint) = edit_hint {
            formatted.push_str(&hint);
        }
        // Append write recovery hint if consecutive failures detected
        if let Some(hint) = write_hint {
            formatted.push_str(&hint);
        }
        // Append read repeat hint if repeated reads detected (Issue #185)
        if let Some(hint) = read_hint {
            formatted.push_str(&hint);
        }
        // Issue #265: Inject hint when shell.exec is used for file reading
        // (grep/sed/cat), which bypasses the read transition guard.
        if result.tool_name == "shell.exec"
            && result.status == ToolExecutionStatus::Completed
            && let Some(cmd) = result.summary.strip_prefix("shell.exec completed: ")
            && crate::tooling::shell_policy::is_file_read_shell_command(cmd)
        {
            formatted.push_str(
                "\n\n[Anvil hint] Using grep/sed/cat to read file contents counts as \
                 exploration. You already have enough context — proceed to implement \
                 changes using file.edit or file.write instead of reading more.",
            );
        }
        let mut msg = SessionMessage::new(MessageRole::Tool, &result.tool_name, formatted)
            .with_id(self.next_message_id("tool"));
        msg.is_error = is_error;

        // Attach image paths for Image payloads so the agent layer can
        // resolve them to base64 when building the provider request.
        if let ToolExecutionPayload::Image { source_path, .. } = &result.payload {
            msg = msg.with_image_paths(vec![source_path.clone()]);
        }
        self.session.push_message(msg);
    }

    /// Track consecutive file.write failures and repeated successful writes,
    /// returning a hint if either threshold is reached.
    fn update_write_trackers(&mut self, result: &ToolExecutionResult) -> Option<String> {
        if result.tool_name != "file.write" {
            return None;
        }
        if result.status == ToolExecutionStatus::Failed {
            if let Some(path) = extract_write_path_from_summary(&result.summary)
                .filter(|p| self.write_fail_tracker.record_failure(p))
            {
                let count = self.write_fail_tracker.failure_count(&path);
                tracing::warn!(
                    tool = "file.write",
                    path = %path,
                    count = count,
                    "repeated write failure"
                );
                let safe_path = crate::session::sanitize_for_prompt_entry(&path);
                return Some(format!(
                    "\n\n[Anvil hint] file.write has failed {count} consecutive \
                     times for '{safe_path}'. Please check the error message carefully. \
                     Consider: (1) verifying the file path is correct, \
                     (2) splitting the content into smaller files, \
                     (3) using file.edit for partial modifications instead."
                ));
            }
        } else if result.status == ToolExecutionStatus::Completed
            && let Some(artifact) = result.artifacts.first()
        {
            let path = resolve_edit_tracker_path(artifact);
            self.write_fail_tracker.record_success(&path);
            let repeat_action = self.write_repeat_tracker.record_write(&path);
            if matches!(
                repeat_action,
                WriteRepeatAction::Warn | WriteRepeatAction::StrongWarn
            ) {
                let count = self.write_repeat_tracker.write_count(&path);
                tracing::warn!(
                    path = %path,
                    count = count,
                    "same file written repeatedly"
                );
                let safe_path = crate::session::sanitize_for_prompt_entry(&path);
                let detail = if repeat_action == WriteRepeatAction::StrongWarn {
                    ". This appears to be a loop. Consider: \
                     (1) Stop rewriting this file entirely, \
                     (2) Use file.read to verify the current state, \
                     (3) Use file.edit for targeted changes to specific sections."
                } else {
                    ". Consider: \
                     (1) Use file.read to review the current content, \
                     (2) Use file.edit to modify only the specific section \
                     that needs changing, \
                     (3) Avoid rewriting the entire file repeatedly."
                };
                return Some(format!(
                    "\n\n[Anvil hint] file.write has been called {count} times \
                     for '{safe_path}'{detail}"
                ));
            }
        }
        None
    }

    /// Execute a single retry LLM turn after ANVIL_FINAL guard activation.
    /// This is a simplified version of the agentic loop that processes exactly one LLM response.
    /// After the retry, the response is accepted unconditionally (no further guard check).
    ///
    /// CB-005: The stream callback only processes `TokenDelta` events, consistent
    /// with `complete_structured_response`. `ProviderEvent::Agent(Done)` is not
    /// handled here because the retry response is parsed from the accumulated
    /// token buffer directly (the same pattern used by the main agentic loop).
    #[allow(clippy::too_many_arguments)]
    fn run_guarded_retry_turn<C: ProviderClient>(
        &mut self,
        status: &str,
        saved_status: &str,
        elapsed_ms: u128,
        inference_performance: Option<crate::contracts::InferencePerformanceView>,
        tui: &Tui,
        provider_client: &C,
    ) -> Result<Vec<String>, AppError> {
        // Build request and call LLM for one more turn
        let (system_prompt, calibration_ratio) = self.prepare_turn_context();
        let (mut request, _) = BasicAgentLoop::build_turn_request_calibrated(
            self.effective_model().to_string(),
            &self.session,
            self.provider.capabilities.streaming && self.config.runtime.stream,
            self.effective_context_window(),
            &system_prompt,
            calibration_ratio,
            self.config.runtime.context_budget,
        );
        request.max_output_tokens = self.config.runtime.max_output_tokens;

        let spinner = Spinner::start(
            format!("ANVIL_FINAL guard retry. model={}", self.effective_model()),
            self.config.mode.interactive,
        );

        let mut token_buffer = String::new();
        let mut first_token = true;
        let mut spinner_opt = Some(spinner);

        let stream_result = provider_client.stream_turn(&request, &mut |event| {
            if let Some(s) = spinner_opt.take() {
                s.stop();
            }
            if let ProviderEvent::TokenDelta(delta) = &event {
                token_buffer.push_str(delta);
                if first_token {
                    first_token = false;
                }
                let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("{delta}"));
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
        });

        if let Some(s) = spinner_opt.take() {
            s.stop();
        }
        if !first_token {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("\n"));
        }

        stream_result.map_err(|err| match err {
            crate::provider::ProviderTurnError::Cancelled => {
                AppError::ToolExecution("guarded retry cancelled".to_string())
            }
            other => AppError::ToolExecution(format!("guarded retry failed: {other}")),
        })?;

        // Parse the retry response
        let retry_structured = match BasicAgentLoop::parse_structured_response_with_registry(
            &token_buffer,
            &self.tools,
        ) {
            Ok(parsed) => parsed,
            Err(_) => {
                let trimmed = token_buffer.trim();
                StructuredAssistantResponse::empty(if trimmed.is_empty() {
                    "Guard retry produced empty response.".to_string()
                } else {
                    trimmed.to_string()
                })
            }
        };

        // If response has tool_calls, delegate to complete_structured_response.
        // Issue #173: Pass anvil_final_already=true since Guard Retry was triggered
        // by ANVIL_FINAL detection.
        if !retry_structured.tool_calls.is_empty() {
            return self.complete_structured_response(
                retry_structured,
                status,
                saved_status,
                elapsed_ms,
                inference_performance,
                tui,
                provider_client,
                true, // anvil_final_already: Guard Retry origin was ANVIL_FINAL
            );
        }

        // No tool calls — accept as final answer (no further guard check)
        self.record_assistant_output(
            self.next_message_id("assistant"),
            retry_structured.final_response,
        )?;

        // Transition to Done
        let mut done = AppStateSnapshot::new(RuntimeState::Done)
            .with_status(status.to_string())
            .with_completion_summary(
                format!("Guard retry completed. {saved_status}"),
                saved_status.to_string(),
            )
            .with_elapsed_ms(elapsed_ms);
        if let Some(perf) = inference_performance {
            done = done.with_inference_performance(perf);
        }
        let mut done_snapshot = self.transition_with_context(done, StateTransition::Finish)?;
        self.evaluate_context_warning(&mut done_snapshot);
        let frames = vec![self.render_console(tui)?];
        Ok(frames)
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
            inference_performance,
        } = event
        else {
            return Ok(None);
        };

        let structured =
            BasicAgentLoop::parse_structured_response_with_registry(assistant_message, &self.tools)
                .map_err(AppError::ToolExecution)?;
        if structured.tool_calls.is_empty() {
            // ANVIL_FINAL guard: only activate when the message contains a
            // structured ANVIL_FINAL block (not plain-text Done messages).
            // Inject retry message and re-invoke LLM directly (not via
            // complete_structured_response, which would wastefully execute the
            // empty tool_calls pipeline).
            //
            // CB-002: Retry count is always 0 here because this path handles
            // the first-turn Done event (before any agentic loop iteration).
            // MAX_FINAL_GUARD_RETRIES = 1 ensures at most one retry, so the
            // hardcoded 0 is correct and sufficient for the current design.
            // Issue #173: Use lenient detection for Done path (response is complete)
            if BasicAgentLoop::is_complete_structured_response_lenient(assistant_message)
                && self.should_activate_final_guard(0)
            {
                // ANVIL_FINAL detected → record observation (Issue #159)
                self.phase_estimator.observe_anvil_final();
                // Issue #253: Apply plan gate even on zero-tool-call Done path.
                // Use require_plan variant so NoPlan also suppresses.
                if self.check_plan_final_gate_require_plan() {
                    self.record_assistant_output(
                        self.next_message_id("assistant"),
                        assistant_message,
                    )?;
                    return Ok(Some(self.run_guarded_retry_turn(
                        status,
                        saved_status,
                        *elapsed_ms,
                        inference_performance.clone(),
                        tui,
                        provider_client,
                    )?));
                }
                self.inject_final_guard_retry();
                // Record the assistant message that triggered the guard
                self.record_assistant_output(self.next_message_id("assistant"), assistant_message)?;
                // Re-invoke LLM and process the response
                return Ok(Some(self.run_guarded_retry_turn(
                    status,
                    saved_status,
                    *elapsed_ms,
                    inference_performance.clone(),
                    tui,
                    provider_client,
                )?));
            }
            // CB-002: Record turn stats for non-tool turns
            self.session_stats.record_turn();
            let token_budget = self.effective_token_budget();
            log_turn_summary(&TurnSummary {
                turn: self.session_stats.total_turns,
                max_turns: self.config.runtime.max_agent_iterations as u32,
                elapsed: std::time::Duration::from_millis(*elapsed_ms as u64),
                tokens_used: 0,
                token_budget,
                tool_calls: 0,
                tool_names: &[],
                files_modified: 0,
                compact_info: None,
                phase: self.phase_estimator.current_phase(),
                mutations_this_turn: None,
                items_advanced_this_turn: None,
            });
            return Ok(None);
        }

        // Issue #173: Pass anvil_final_detected flag from the parsed response
        let anvil_final = structured.anvil_final_detected;
        Ok(Some(self.complete_structured_response(
            structured,
            status,
            saved_status,
            *elapsed_ms,
            inference_performance.clone(),
            tui,
            provider_client,
            anvil_final,
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
            crate::tooling::ToolInput::FileSearch { pattern, regex, .. } => {
                if *regex {
                    format!("regex search for {pattern}")
                } else {
                    format!("search for {pattern}")
                }
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
            crate::tooling::ToolInput::GitStatus {} => "git status".to_string(),
            crate::tooling::ToolInput::GitDiff { path, .. } => {
                if let Some(p) = path {
                    format!("git diff {p}")
                } else {
                    "git diff".to_string()
                }
            }
            crate::tooling::ToolInput::GitLog { count, path } => {
                let count_str = count.map_or("10".to_string(), |c| c.to_string());
                if let Some(p) = path {
                    format!("git log -{count_str} {p}")
                } else {
                    format!("git log -{count_str}")
                }
            }
            crate::tooling::ToolInput::FileEditAnchor { path, .. } => {
                format!("edit (anchor) {path}")
            }
        };
        plan.push(item);
    }
    plan.push("review generated result and summarize".to_string());
    plan
}

/// Head+Tail truncation: 先頭と末尾を保持し、中間を省略する。
/// head_pct: headに割り当てるパーセンテージ (0〜100)
/// max_chars: コンテンツの文字数予算（marker文字列は含まない）
/// 最終出力長: max_chars + marker文字列長（約60文字）
pub fn truncate_with_head_tail(content: &str, max_chars: usize, head_pct: usize) -> String {
    let total_chars = content.chars().count(); // O(n), 1回だけ呼ぶ
    if total_chars <= max_chars {
        return content.to_string();
    }
    if max_chars == 0 {
        return format!("... [{} chars total, all truncated] ...", total_chars);
    }

    let head_pct = head_pct.min(100); // clamp to prevent underflow
    let head_chars = max_chars / 100 * head_pct + (max_chars % 100) * head_pct / 100; // overflow-safe
    let tail_chars = max_chars - head_chars;
    let omitted = total_chars - head_chars - tail_chars;

    // head: 先頭 head_chars 文字（UTF-8安全）
    let head: String = content.chars().take(head_chars).collect();

    // tail: 末尾 tail_chars 文字（UTF-8安全）
    let tail: String = {
        let skip_count = total_chars - tail_chars;
        content.chars().skip(skip_count).collect()
    };

    format!(
        "{}\n\n... [{} chars truncated, {} chars total] ...\n\n{}",
        head, omitted, total_chars, tail
    )
}

/// Extract the file path from a file.edit error summary.
/// Looks for patterns like "... in {path}. ..." or "... in {path},"
/// Normalize a file path for consistent EditFailTracker key usage (Issue #158).
/// Used by both failure (from summary) and success (from artifact) paths.
fn resolve_edit_tracker_path(raw_path: &str) -> String {
    raw_path.trim_start_matches("./").to_string()
}

fn extract_edit_path_from_summary(summary: &str) -> Option<String> {
    // Pattern: "file.edit: ... in {path}. ..."
    // or "file.edit: ... in {path}, ..."
    let in_idx = summary.find(" in ")?;
    let after_in = &summary[in_idx + 4..];
    let end = after_in
        .find(". ")
        .or_else(|| after_in.find(", "))
        .unwrap_or(after_in.len());
    let path = after_in[..end].trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Extract the file path from a file.write error summary.
/// Looks for patterns like "file.write failed for {path}: {err}"
/// or "file.write failed for {path} (parent creation failed for {parent}): {err}"
fn extract_write_path_from_summary(summary: &str) -> Option<String> {
    let prefix = "file.write failed for ";
    let rest = summary.strip_prefix(prefix)?;

    // Priority 1: check for the parent-creation fixed marker first
    let parent_marker = " (parent creation failed for ";
    if let Some(marker_pos) = rest.find(parent_marker) {
        return Some(rest[..marker_pos].to_string());
    }

    // Priority 2: normal format — use rsplit_once to find the LAST ": " boundary,
    // which avoids mis-splitting on paths containing ": " (rare but legal on Unix).
    // The error portion after the last ": " is the OS I/O error message.
    rest.rsplit_once(": ").map(|(path, _err)| path.to_string())
}

/// Format a tool execution result into a message that the LLM can interpret.
///
/// Includes the actual payload (file content, search matches) so the LLM
/// can reason about the results in subsequent turns.
pub fn format_tool_result_message(result: &ToolExecutionResult, max_chars: usize) -> String {
    let max_chars = effective_tool_result_max_chars(result.tool_name.as_str(), max_chars);
    match &result.payload {
        ToolExecutionPayload::None => {
            format!("[tool result: {}] {}", result.tool_name, result.summary)
        }
        ToolExecutionPayload::Text(content) => {
            let head_pct = match &result.status {
                ToolExecutionStatus::Completed => 80,
                ToolExecutionStatus::Failed | ToolExecutionStatus::Interrupted => 20,
            };
            let truncated = truncate_with_head_tail(content, max_chars, head_pct);
            if result.tool_name == "file.read"
                && result.status == ToolExecutionStatus::Completed
                && let Some(path) = result.artifacts.first()
            {
                return format!(
                    "[tool result: {}] {}\nPath: {}\n{}",
                    result.tool_name, result.summary, path, truncated
                );
            }
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

fn effective_tool_result_max_chars(tool_name: &str, default_max_chars: usize) -> usize {
    match tool_name {
        "file.read" => default_max_chars.min(FILE_READ_RESULT_MAX_CHARS),
        name if name.starts_with("system.") => {
            default_max_chars.min(SYNTHETIC_GUIDANCE_RESULT_MAX_CHARS)
        }
        _ => default_max_chars,
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
        diff_summary: None,
        edit_detail: None,
        rolled_back: false,
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

/// Determine whether a tool call should be auto-approved based on trust settings.
///
/// This is a pure function for testability; it does not access `App` state directly.
pub(crate) fn is_trusted(
    tool_name: &str,
    kind: ToolKind,
    permission_class: PermissionClass,
    trust_all: bool,
    trusted_tools: &HashSet<String>,
) -> bool {
    if permission_class == PermissionClass::Restricted {
        return false;
    }
    if trust_all && kind != ToolKind::Mcp {
        return true;
    }
    trusted_tools.contains(tool_name)
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    #[test]
    fn trust_all_auto_approves_confirm_tools() {
        let trusted_tools = HashSet::new();
        assert!(is_trusted(
            "shell.exec",
            ToolKind::ShellExec,
            PermissionClass::Confirm,
            true,
            &trusted_tools,
        ));
    }

    #[test]
    fn trust_all_blocks_restricted_tools() {
        let trusted_tools = HashSet::new();
        assert!(!is_trusted(
            "some.restricted",
            ToolKind::ShellExec,
            PermissionClass::Restricted,
            true,
            &trusted_tools,
        ));
    }

    #[test]
    fn trust_all_blocks_mcp_tools() {
        let trusted_tools = HashSet::new();
        assert!(!is_trusted(
            "mcp__github__create_issue",
            ToolKind::Mcp,
            PermissionClass::Confirm,
            true,
            &trusted_tools,
        ));
    }

    #[test]
    fn individual_trust_approves_specific_tool() {
        let mut trusted_tools = HashSet::new();
        trusted_tools.insert("file.edit".to_string());
        assert!(is_trusted(
            "file.edit",
            ToolKind::FileEdit,
            PermissionClass::Confirm,
            false,
            &trusted_tools,
        ));
    }

    #[test]
    fn individual_trust_allows_mcp() {
        let mut trusted_tools = HashSet::new();
        trusted_tools.insert("mcp__github__create_issue".to_string());
        assert!(is_trusted(
            "mcp__github__create_issue",
            ToolKind::Mcp,
            PermissionClass::Confirm,
            false,
            &trusted_tools,
        ));
    }

    #[test]
    fn untrusted_tool_returns_false() {
        let trusted_tools = HashSet::new();
        assert!(!is_trusted(
            "file.edit",
            ToolKind::FileEdit,
            PermissionClass::Confirm,
            false,
            &trusted_tools,
        ));
    }

    #[test]
    fn test_extract_write_path_from_summary() {
        let summary = "file.write failed for /tmp/foo.rs: Permission denied";
        assert_eq!(
            extract_write_path_from_summary(summary),
            Some("/tmp/foo.rs".to_string())
        );
    }

    #[test]
    fn test_extract_write_path_parent_fail() {
        let summary = "file.write failed for /tmp/dir/foo.rs (parent creation failed for /tmp/dir): No such file or directory";
        assert_eq!(
            extract_write_path_from_summary(summary),
            Some("/tmp/dir/foo.rs".to_string())
        );
    }

    #[test]
    fn test_extract_write_path_invalid() {
        assert_eq!(extract_write_path_from_summary("some random error"), None);
        assert_eq!(
            extract_write_path_from_summary("file.edit: something in foo.rs. blah"),
            None
        );
    }

    // --- summarize_tool_names tests (Issue #206 B-3) ---

    #[test]
    fn summarize_tool_names_empty() {
        let names: Vec<String> = vec![];
        assert_eq!(summarize_tool_names(&names), "");
    }

    #[test]
    fn summarize_tool_names_single() {
        let names = vec!["file.read".to_string()];
        assert_eq!(summarize_tool_names(&names), "file.read");
    }

    #[test]
    fn summarize_tool_names_duplicates() {
        let names = vec![
            "file.read".to_string(),
            "file.read".to_string(),
            "file.edit".to_string(),
            "file.read".to_string(),
        ];
        let result = summarize_tool_names(&names);
        // file.read x3 should come first (higher count)
        assert!(result.starts_with("file.read x3"));
        assert!(result.contains("file.edit"));
    }

    #[test]
    fn summarize_tool_names_all_unique() {
        let names = vec![
            "file.read".to_string(),
            "file.edit".to_string(),
            "shell.exec".to_string(),
        ];
        let result = summarize_tool_names(&names);
        // All have count 1, no "x" suffix
        assert!(!result.contains(" x"));
        assert!(result.contains("file.read"));
        assert!(result.contains("file.edit"));
        assert!(result.contains("shell.exec"));
    }

    #[test]
    fn format_tool_result_message_caps_file_read_payload_more_aggressively() {
        let result = ToolExecutionResult {
            tool_call_id: "call_001".to_string(),
            tool_name: "file.read".to_string(),
            status: ToolExecutionStatus::Completed,
            summary: "read ok".to_string(),
            payload: ToolExecutionPayload::Text("A".repeat(3_000)),
            artifacts: vec!["./src/main.rs".to_string()],
            elapsed_ms: 0,
            diff_summary: None,
            edit_detail: None,
            rolled_back: false,
        };

        let formatted = format_tool_result_message(&result, 8_000);

        assert!(formatted.contains("Path: ./src/main.rs"));
        assert!(formatted.contains("[1000 chars truncated, 3000 chars total]"));
        assert!(
            !formatted.contains("[0 chars truncated"),
            "file.read should use the tighter per-tool cap"
        );
    }
}
