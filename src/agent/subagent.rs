//! Sub-agent execution loop.
//!
//! Provides [`SubAgentSession`] which runs an independent LLM loop with a
//! restricted tool set (read-only) within a sandboxed scope directory.

use crate::app::policy::{OFFLINE_BLOCK_PAYLOAD, check_offline_blocked};
use crate::config::EffectiveConfig;
use crate::provider::{ProviderClient, ProviderEvent, ProviderTurnError};
use crate::session::{MessageRole, SessionMessage, SessionRecord};
use crate::tooling::{
    LocalToolExecutor, ToolCallRequest, ToolExecutionPayload, ToolExecutionResult,
    ToolExecutionStatus, ToolInput, ToolRegistry,
};

use super::BasicAgentLoop;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Maximum number of LLM turns a sub-agent may perform.
const MAX_SUBAGENT_ITERATIONS: u32 = 10;

/// Wall-clock timeout for the entire sub-agent run.
const SUBAGENT_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Sub-agent system prompt constants
// ---------------------------------------------------------------------------

const SUBAGENT_PROTOCOL_BASE: &str = r#"You are a sub-agent of Anvil, a local coding agent.

## Tool protocol
When you need to read files or search, respond using fenced blocks.

After you have gathered enough information, output your final answer inside:
```ANVIL_FINAL
Your summary here.
```

Rules:
- All paths must be relative (start with ./ or a directory name).
- Do not use any other tool syntax.
- Always include ANVIL_FINAL when you are done.
- You MUST output ANVIL_FINAL to signal completion.

"#;

const TOOL_DESC_FILE_READ: &str = r#"- file.read — read a file or list a directory:
```ANVIL_TOOL
{"id":"call_001","tool":"file.read","path":"./relative/path"}
```

"#;

pub(crate) const TOOL_DESC_FILE_SEARCH: &str = r#"- file.search — search for files by name or content (supports regex and context lines):
```ANVIL_TOOL
{"id":"call_002","tool":"file.search","root":".","pattern":"search term"}
```
  Optional parameters:
  - "regex": true — interpret pattern as a regular expression (default: false)
  - "context_lines": N — show N lines before/after each match, max 10 (default: 0)
  Example with regex and context:
```ANVIL_TOOL
{"id":"call_010","tool":"file.search","root":".","pattern":"fn\\s+main","regex":true,"context_lines":3}
```

"#;

const TOOL_DESC_WEB_FETCH: &str = r#"- web.fetch — fetch the contents of a URL:
```ANVIL_TOOL
{"id":"call_003","tool":"web.fetch","url":"https://example.com"}
```

"#;

const EXPLORE_ROLE_PROMPT: &str = r#"## Your role
You are an Explore sub-agent. Your task is to investigate the codebase and gather information.
- Read files and search for patterns to understand the code structure.
- Summarize your findings clearly in ANVIL_FINAL.
- You only have read-only access: file.read and file.search.
"#;

const PLAN_ROLE_PROMPT: &str = r#"## Your role
You are a Plan sub-agent. Your task is to create an implementation plan.
- Read files and search the codebase to understand existing patterns.
- You may fetch web URLs for reference documentation.
- Produce a detailed, actionable plan in ANVIL_FINAL.
- You only have read-only access: file.read, file.search, and web.fetch.
"#;

const PLAN_ROLE_PROMPT_OFFLINE: &str = r#"## Your role
You are a Plan sub-agent. Your task is to create an implementation plan.
- Read files and search the codebase to understand existing patterns.
- Produce a detailed, actionable plan in ANVIL_FINAL.
- You only have read-only access: file.read and file.search.
- Note: Offline mode is active. Web access is unavailable.
"#;

/// Build a system prompt for a sub-agent of the given kind.
///
/// When `offline` is `true`, web-related tool descriptions and prompts
/// are excluded from the Plan sub-agent prompt.
pub fn build_subagent_system_prompt(kind: &SubAgentKind, offline: bool) -> String {
    let mut prompt = String::new();
    prompt.push_str(SUBAGENT_PROTOCOL_BASE);
    match kind {
        SubAgentKind::Explore => {
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(TOOL_DESC_FILE_SEARCH);
            prompt.push_str(EXPLORE_ROLE_PROMPT);
        }
        SubAgentKind::Plan => {
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(TOOL_DESC_FILE_SEARCH);
            if !offline {
                prompt.push_str(TOOL_DESC_WEB_FETCH);
            }
            prompt.push_str(if offline {
                PLAN_ROLE_PROMPT_OFFLINE
            } else {
                PLAN_ROLE_PROMPT
            });
        }
    }
    prompt
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Sub-agent kind (Explore or Plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentKind {
    Explore,
    Plan,
}

impl SubAgentKind {
    /// Convert a [`ToolInput`] variant to a [`SubAgentKind`], if applicable.
    pub fn from_tool_input(input: &ToolInput) -> Option<SubAgentKind> {
        match input {
            ToolInput::AgentExplore { .. } => Some(SubAgentKind::Explore),
            ToolInput::AgentPlan { .. } => Some(SubAgentKind::Plan),
            _ => None,
        }
    }
}

/// Result returned by a successful sub-agent run.
pub struct SubAgentResult {
    pub summary: String,
    pub estimated_tokens: usize,
    pub iterations_used: u32,
    pub timed_out: bool,
}

impl SubAgentResult {
    /// Convert into a [`ToolExecutionResult`] for integration with the main
    /// agent's tool result recording flow.
    pub fn into_tool_execution_result(self, call: &ToolCallRequest) -> ToolExecutionResult {
        // Both timed_out and normal completion are Completed status;
        // timed_out flag is preserved in the SubAgentResult for reporting.
        let status = ToolExecutionStatus::Completed;
        ToolExecutionResult {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            status,
            summary: format!(
                "sub-agent completed in {} iteration(s)",
                self.iterations_used
            ),
            payload: ToolExecutionPayload::Text(self.summary),
            artifacts: Vec::new(),
            elapsed_ms: 0,
        }
    }
}

/// Errors that can occur during sub-agent execution.
#[derive(Debug)]
pub enum SubAgentError {
    /// LLM communication error.
    Provider(ProviderTurnError),
    /// Tool execution error within the sub-agent.
    ToolExecution(String),
    /// Wall-clock timeout exceeded.
    Timeout,
    /// Iteration limit reached.
    MaxIterations,
    /// Scope path failed sandbox validation.
    SandboxViolation(String),
}

impl std::fmt::Display for SubAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubAgentError::Provider(e) => write!(f, "SubAgent provider error: {e}"),
            SubAgentError::ToolExecution(msg) => {
                write!(f, "SubAgent tool execution error: {msg}")
            }
            SubAgentError::Timeout => write!(f, "SubAgent timed out"),
            SubAgentError::MaxIterations => write!(f, "SubAgent reached max iterations"),
            SubAgentError::SandboxViolation(path) => {
                write!(f, "SubAgent sandbox violation: {path}")
            }
        }
    }
}

impl std::error::Error for SubAgentError {}

impl SubAgentError {
    /// Convert this error into a [`ToolExecutionResult`].
    ///
    /// Conversion policy:
    /// - Timeout / MaxIterations -> Completed (partial result may exist)
    /// - Provider / ToolExecution / SandboxViolation -> Failed
    pub fn into_tool_execution_result(self, call: &ToolCallRequest) -> ToolExecutionResult {
        let (status, output) = match &self {
            SubAgentError::Timeout | SubAgentError::MaxIterations => {
                (ToolExecutionStatus::Completed, self.to_string())
            }
            _ => (ToolExecutionStatus::Failed, self.to_string()),
        };
        ToolExecutionResult {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            status,
            summary: output.clone(),
            payload: ToolExecutionPayload::Text(output),
            artifacts: Vec::new(),
            elapsed_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Outcome of a single sub-agent turn.
enum TurnOutcome {
    /// The sub-agent produced a final answer.
    Finished(SubAgentResult),
    /// The sub-agent wants to continue (tool calls were executed).
    Continue,
}

// ---------------------------------------------------------------------------
// SubAgentSession
// ---------------------------------------------------------------------------

/// An independent sub-agent session that runs within a restricted scope.
///
/// Responsibilities:
/// - `new()`:      initialise session, registry, system prompt
/// - `run_turn()`: one LLM turn (build request -> stream -> parse -> validate -> execute -> record)
/// - `run()`:      loop control (iteration limit, timeout, shutdown flag)
pub struct SubAgentSession<'a, C: ProviderClient> {
    kind: SubAgentKind,
    session: SessionRecord,
    registry: ToolRegistry,
    system_prompt: String,
    provider_client: &'a C,
    config: &'a EffectiveConfig,
    shutdown_flag: Arc<AtomicBool>,
    /// Sandbox root for LocalToolExecutor (set to scope path, SR4-002).
    scope_path: std::path::PathBuf,
    /// Running count of iterations used (for result reporting).
    iterations_used: u32,
}

impl<'a, C: ProviderClient> SubAgentSession<'a, C> {
    /// Create a new sub-agent session.
    ///
    /// The `scope` path is used as the sandbox root for all tool execution
    /// (SR4-002), restricting file access to that directory tree.
    pub fn new(
        kind: SubAgentKind,
        prompt: &str,
        scope: &Path,
        provider_client: &'a C,
        config: &'a EffectiveConfig,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Self {
        // 1. Independent session with scope as cwd
        let mut session = SessionRecord::new(scope.to_path_buf());
        session.push_message(SessionMessage::new(MessageRole::User, "subagent", prompt));

        // 2. Restricted tool registry (no agent.explore / agent.plan -> SR4-005)
        let mut registry = ToolRegistry::new();
        match kind {
            SubAgentKind::Explore => registry.register_explore_tools(),
            SubAgentKind::Plan => registry.register_plan_tools(),
        }

        // 3. Dedicated system prompt
        let system_prompt = build_subagent_system_prompt(&kind, config.mode.offline);

        SubAgentSession {
            kind,
            session,
            registry,
            system_prompt,
            provider_client,
            config,
            shutdown_flag,
            scope_path: scope.to_path_buf(),
            iterations_used: 0,
        }
    }

    /// Execute one LLM turn: request -> stream -> parse -> validate -> execute -> record.
    fn run_turn(&mut self) -> Result<TurnOutcome, SubAgentError> {
        // Build the provider request
        let request = BasicAgentLoop::build_turn_request(
            &self.config.runtime.model,
            &self.session,
            true,
            self.config.runtime.context_window,
            &self.system_prompt,
        );

        // Stream the LLM response, collecting token deltas
        let mut token_buffer = String::new();
        self.provider_client
            .stream_turn(&request, &mut |event| {
                if let ProviderEvent::TokenDelta(delta) = &event {
                    token_buffer.push_str(delta);
                    // Progress output on stderr (IR3-004)
                    let _ =
                        std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("{delta}"));
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
            })
            .map_err(SubAgentError::Provider)?;

        let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("\n"));

        // Parse structured response
        let structured = BasicAgentLoop::parse_structured_response(&token_buffer)
            .map_err(SubAgentError::ToolExecution)?;

        // Record assistant output in the sub-agent session
        self.session.push_message(SessionMessage::new(
            MessageRole::Assistant,
            "subagent",
            &token_buffer,
        ));

        // If no tool calls, this is the final response
        if structured.tool_calls.is_empty() {
            let tokens = crate::contracts::tokens::estimate_tokens(
                &token_buffer,
                crate::contracts::tokens::ContentKind::Text,
            );
            return Ok(TurnOutcome::Finished(SubAgentResult {
                summary: structured.final_response,
                estimated_tokens: tokens,
                iterations_used: self.iterations_used + 1,
                timed_out: false,
            }));
        }

        // Validate and execute tool calls
        // SR4-003: validate() is mandatory before execution
        let mut executor = LocalToolExecutor::new(self.scope_path.clone(), &self.config.runtime)
            .with_shutdown_flag(self.shutdown_flag.clone());

        for call in &structured.tool_calls {
            // Validate against restricted registry
            let validated = match self.registry.validate(call.clone()) {
                Ok(v) => v,
                Err(err) => {
                    // Record the validation error as a tool result
                    let error_msg = format!("tool validation failed: {err:?}");
                    self.session.push_message(SessionMessage::new(
                        MessageRole::Tool,
                        "tool",
                        format!("[tool result: {}] {}", call.tool_name, error_msg),
                    ));
                    continue;
                }
            };

            // Offline policy check (validate succeeded, check before approve)
            if let Some(summary) = check_offline_blocked(self.config, call) {
                self.session.push_message(SessionMessage::new(
                    MessageRole::Tool,
                    "tool",
                    format!(
                        "[tool result: {}] {}\n{}",
                        call.tool_name, summary, OFFLINE_BLOCK_PAYLOAD
                    ),
                ));
                continue;
            }

            // Auto-approve (sub-agent tools are all Safe)
            let approved = validated.approve();
            let exec_request =
                match approved.into_execution_request(crate::tooling::ToolExecutionPolicy {
                    approval_required: false,
                    allow_restricted: true,
                    plan_mode: false,
                    plan_scope_granted: true,
                }) {
                    Ok(r) => r,
                    Err(err) => {
                        let error_msg = format!("tool execution policy error: {err:?}");
                        self.session.push_message(SessionMessage::new(
                            MessageRole::Tool,
                            "tool",
                            format!("[tool result: {}] {}", call.tool_name, error_msg),
                        ));
                        continue;
                    }
                };

            // Execute
            let result = executor
                .execute(exec_request)
                .unwrap_or_else(|err| ToolExecutionResult {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    status: ToolExecutionStatus::Failed,
                    summary: err.to_string(),
                    payload: ToolExecutionPayload::Text(err.to_string()),
                    artifacts: Vec::new(),
                    elapsed_ms: 0,
                });

            // Record tool result in the sub-agent session
            // Apply tool_result_max_chars (IR3-005)
            let formatted = crate::app::agentic::format_tool_result_message(
                &result,
                self.config.runtime.tool_result_max_chars,
            );
            self.session
                .push_message(SessionMessage::new(MessageRole::Tool, "tool", formatted));
        }

        Ok(TurnOutcome::Continue)
    }

    /// Run the sub-agent loop to completion.
    ///
    /// Enforces iteration limit, wall-clock timeout, and shutdown flag.
    pub fn run(mut self) -> Result<SubAgentResult, SubAgentError> {
        let kind_label = match self.kind {
            SubAgentKind::Explore => "explore",
            SubAgentKind::Plan => "plan",
        };
        eprintln!("[subagent:{kind_label}] Starting...");
        let start = Instant::now();

        for iteration in 0..MAX_SUBAGENT_ITERATIONS {
            if start.elapsed() > SUBAGENT_TIMEOUT {
                return Err(SubAgentError::Timeout);
            }
            if self.shutdown_flag.load(Ordering::Relaxed) {
                return Err(SubAgentError::Timeout);
            }

            self.iterations_used = iteration + 1;
            eprintln!(
                "[subagent:{kind_label}] iteration {}/{}...",
                iteration + 1,
                MAX_SUBAGENT_ITERATIONS
            );

            match self.run_turn()? {
                TurnOutcome::Finished(result) => return Ok(result),
                TurnOutcome::Continue => continue,
            }
        }

        Err(SubAgentError::MaxIterations)
    }
}
