//! Sub-agent execution loop.
//!
//! Provides [`SubAgentSession`] which runs an independent LLM loop with a
//! restricted tool set (read-only) within a sandboxed scope directory.

use crate::app::policy::{OFFLINE_BLOCK_PAYLOAD, check_offline_blocked};
use crate::config::EffectiveConfig;
use crate::contracts::{Finding, SubAgentPayload, TerminationReason};
use crate::provider::{ProviderClient, ProviderEvent, ProviderTurnError};
use crate::session::{MessageRole, SessionMessage, SessionRecord};
use crate::tooling::{
    LocalToolExecutor, ToolCallRequest, ToolExecutionPayload, ToolExecutionResult,
    ToolExecutionStatus, ToolInput, ToolRegistry,
};

use serde::Deserialize;

use super::BasicAgentLoop;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Sub-agent system prompt constants
// ---------------------------------------------------------------------------

const SUBAGENT_PROTOCOL_BASE: &str = r#"You are a sub-agent of Anvil, a local coding agent.

## Tool protocol
When you need to read files or search, respond using fenced blocks.

After you have gathered enough information, output your final answer as JSON inside:
```ANVIL_FINAL
{
  "found_files": ["path/to/file1.rs", "path/to/file2.rs"],
  "key_findings": [
    {
      "title": "Short title of finding",
      "detail": "Detailed explanation with evidence",
      "related_code": ["src/main.rs:10", "src/lib.rs:20"]
    }
  ],
  "raw_summary": "A concise overall summary of what you found.",
  "confidence": 0.8
}
```

Rules:
- All paths must be relative (start with ./ or a directory name).
- Do not use any other tool syntax.
- Always include ANVIL_FINAL when you are done.
- You MUST output ANVIL_FINAL to signal completion.
- Output valid JSON in ANVIL_FINAL. If you cannot produce JSON, plain text is acceptable as fallback.

"#;

const TOOL_DESC_FILE_READ: &str = r#"- file.read — read a file or list a directory:
```ANVIL_TOOL
{"id":"call_001","tool":"file.read","path":"./relative/path"}
```

"#;

pub(crate) const TOOL_DESC_FILE_SEARCH: &str = r#"- file.search — search for files by name or content (respects .gitignore, supports regex and context lines):
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

const TOOL_DESC_GIT_STATUS: &str = r#"- git.status — show working tree status:
```ANVIL_TOOL
{"id":"call_010","tool":"git.status"}
```

"#;

const TOOL_DESC_GIT_DIFF: &str = r#"- git.diff — show changes in working tree or between commits:
```ANVIL_TOOL
{"id":"call_011","tool":"git.diff","path":"src/main.rs","staged":true}
```

"#;

const TOOL_DESC_GIT_LOG: &str = r#"- git.log — show commit log (oneline format):
```ANVIL_TOOL
{"id":"call_012","tool":"git.log","count":20}
```

"#;

const EXPLORE_ROLE_PROMPT: &str = r#"## Your role
You are an Explore sub-agent specializing in codebase investigation and information gathering.
- Read files and search for patterns to understand the code structure.
- List discovered file paths in "found_files".
- Describe each significant finding in "key_findings" with title, detail, and related_code.
- Provide a concise overall summary in "raw_summary".
- Set "confidence" (0.0-1.0) based on how thoroughly you explored the topic.
- You only have read-only access: file.read and file.search.
"#;

const PLAN_ROLE_PROMPT: &str = r#"## Your role
You are a Plan sub-agent specializing in implementation planning.
- Read files and search the codebase to understand existing patterns.
- You may fetch web URLs for reference documentation.
- List relevant file paths in "found_files".
- Describe plan steps as "key_findings" with actionable details.
- Provide the overall plan summary in "raw_summary".
- Produce a detailed, actionable plan in ANVIL_FINAL as JSON.
- You only have read-only access: file.read, file.search, and web.fetch.
"#;

const PLAN_ROLE_PROMPT_OFFLINE: &str = r#"## Your role
You are a Plan sub-agent specializing in implementation planning.
- Read files and search the codebase to understand existing patterns.
- List relevant file paths in "found_files".
- Describe plan steps as "key_findings" with actionable details.
- Provide the overall plan summary in "raw_summary".
- Produce a detailed, actionable plan in ANVIL_FINAL as JSON.
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
            prompt.push_str(TOOL_DESC_GIT_STATUS);
            prompt.push_str(TOOL_DESC_GIT_DIFF);
            prompt.push_str(TOOL_DESC_GIT_LOG);
            prompt.push_str(EXPLORE_ROLE_PROMPT);
        }
        SubAgentKind::Plan => {
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(TOOL_DESC_FILE_SEARCH);
            if !offline {
                prompt.push_str(TOOL_DESC_WEB_FETCH);
            }
            prompt.push_str(TOOL_DESC_GIT_STATUS);
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
    /// 構造化ペイロード
    pub payload: SubAgentPayload,
    /// 推定トークン数
    pub estimated_tokens: usize,
    /// 使用したイテレーション数
    pub iterations_used: u32,
}

impl SubAgentResult {
    /// Convert into a [`ToolExecutionResult`] for integration with the main
    /// agent's tool result recording flow.
    pub fn into_tool_execution_result(self, call: &ToolCallRequest) -> ToolExecutionResult {
        let reason = self.payload.termination_reason;
        let iterations = self.iterations_used;
        let summary = format!("sub-agent {reason} in {iterations} iteration(s)");
        let json = serde_json::to_string(&self.payload)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialize failed: {e}\"}}"));

        ToolExecutionResult {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            status: ToolExecutionStatus::Completed,
            summary,
            payload: ToolExecutionPayload::Text(json),
            artifacts: Vec::new(),
            elapsed_ms: 0,
        }
    }
}

/// Errors that can occur during sub-agent execution.
///
/// Note: Timeout and MaxIterations have been moved to the Ok path (Issue #129).
/// They are now represented as `SubAgentResult` with `TerminationReason::Timeout`
/// or `TerminationReason::MaxIterations`.
#[derive(Debug)]
pub enum SubAgentError {
    /// LLM communication error.
    Provider(ProviderTurnError),
    /// Tool execution error within the sub-agent.
    ToolExecution(String),
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
    /// All remaining error variants map to Failed status (Issue #129).
    pub fn into_tool_execution_result(self, call: &ToolCallRequest) -> ToolExecutionResult {
        let output = self.to_string();
        ToolExecutionResult {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            status: ToolExecutionStatus::Failed,
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

/// LLM出力専用DTO。システム管理フィールド (termination_reason, error) は含めない。
#[derive(Debug, Deserialize)]
struct SubAgentPayloadInput {
    #[serde(default)]
    found_files: Vec<String>,
    #[serde(default)]
    key_findings: Vec<Finding>,
    #[serde(default)]
    raw_summary: String,
    #[serde(default)]
    confidence: Option<f32>,
}

/// Size limit constants for sub-agent payload fields.
const MAX_FOUND_FILES: usize = 50;
const MAX_KEY_FINDINGS: usize = 20;
const MAX_RELATED_CODE_PER_FINDING: usize = 20;
const MAX_FINDING_TITLE_CHARS: usize = 200;
const MAX_FINDING_DETAIL_CHARS: usize = 2000;
const MAX_RAW_SUMMARY_CHARS: usize = 4000;

/// Parse ANVIL_FINAL content as JSON into a SubAgentPayload.
/// Falls back to plain text in raw_summary on parse failure.
fn parse_final_response_to_payload(final_response: &str) -> SubAgentPayload {
    match serde_json::from_str::<SubAgentPayloadInput>(final_response) {
        Ok(input) => {
            let found_files: Vec<String> = input
                .found_files
                .into_iter()
                .take(MAX_FOUND_FILES)
                .collect();
            let key_findings: Vec<Finding> = input
                .key_findings
                .into_iter()
                .take(MAX_KEY_FINDINGS)
                .map(|finding| Finding {
                    title: finding
                        .title
                        .chars()
                        .take(MAX_FINDING_TITLE_CHARS)
                        .collect(),
                    detail: finding
                        .detail
                        .chars()
                        .take(MAX_FINDING_DETAIL_CHARS)
                        .collect(),
                    related_code: finding
                        .related_code
                        .into_iter()
                        .take(MAX_RELATED_CODE_PER_FINDING)
                        .collect(),
                })
                .collect();

            SubAgentPayload {
                found_files,
                key_findings,
                raw_summary: input
                    .raw_summary
                    .chars()
                    .take(MAX_RAW_SUMMARY_CHARS)
                    .collect(),
                confidence: input.confidence.map(|c| c.clamp(0.0, 1.0)),
                termination_reason: TerminationReason::Completed,
                error: None,
            }
        }
        Err(_) => {
            // Fallback: plain text to raw_summary
            SubAgentPayload::fallback(
                final_response.chars().take(MAX_RAW_SUMMARY_CHARS).collect(),
                TerminationReason::Completed,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// SubAgentSession
// ---------------------------------------------------------------------------

/// Override settings for sub-agent model/context_window (Issue #77).
#[derive(Debug, Clone, Default)]
pub struct SubAgentOverrides {
    pub model: Option<String>,
    pub context_window: Option<u32>,
}

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
    /// Model/context_window overrides from the parent App (Issue #77).
    overrides: SubAgentOverrides,
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
        overrides: SubAgentOverrides,
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
            overrides,
        }
    }

    /// Return the effective model for this sub-agent session.
    fn effective_model(&self) -> &str {
        self.overrides
            .model
            .as_deref()
            .unwrap_or(&self.config.runtime.model)
    }

    /// Return the effective context window for this sub-agent session.
    fn effective_context_window(&self) -> u32 {
        self.overrides
            .context_window
            .unwrap_or(self.config.runtime.context_window)
    }

    /// Execute one LLM turn: request -> stream -> parse -> validate -> execute -> record.
    fn run_turn(&mut self) -> Result<TurnOutcome, SubAgentError> {
        // Build the provider request
        let request = BasicAgentLoop::build_turn_request(
            self.effective_model(),
            &self.session,
            true,
            self.effective_context_window(),
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
            let payload = parse_final_response_to_payload(&structured.final_response);
            return Ok(TurnOutcome::Finished(SubAgentResult {
                payload,
                estimated_tokens: tokens,
                iterations_used: self.iterations_used,
            }));
        }

        // Validate and execute tool calls
        // SR4-003: validate() is mandatory before execution
        let mut executor =
            LocalToolExecutor::new(self.scope_path.clone(), &self.config.runtime, None)
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
    /// Timeout and MaxIterations are returned as Ok with partial results (Issue #129).
    pub fn run(mut self) -> Result<SubAgentResult, SubAgentError> {
        let kind_label = match self.kind {
            SubAgentKind::Explore => "explore",
            SubAgentKind::Plan => "plan",
        };
        eprintln!("[subagent:{kind_label}] Starting...");
        let start = Instant::now();
        let max_iterations = self.config.runtime.subagent_max_iterations;
        let timeout = Duration::from_secs(self.config.runtime.subagent_timeout_secs);

        for iteration in 0..max_iterations {
            // Wall-clock timeout -> partial result with Ok
            if start.elapsed() > timeout {
                eprintln!(
                    "[subagent:{kind_label}] Timed out after {:?}",
                    start.elapsed()
                );
                return Ok(self.build_partial_result(TerminationReason::Timeout, iteration + 1));
            }
            // Shutdown flag -> partial result with Ok
            if self.shutdown_flag.load(Ordering::Relaxed) {
                eprintln!("[subagent:{kind_label}] Shutdown requested");
                return Ok(self.build_partial_result(TerminationReason::Timeout, iteration + 1));
            }

            self.iterations_used = iteration + 1;
            eprintln!(
                "[subagent:{kind_label}] iteration {}/{}...",
                iteration + 1,
                max_iterations
            );

            match self.run_turn()? {
                TurnOutcome::Finished(result) => return Ok(result),
                TurnOutcome::Continue => continue,
            }
        }

        // MaxIterations -> partial result with Ok
        eprintln!("[subagent:{kind_label}] Reached max iterations ({max_iterations})");
        Ok(self.build_partial_result(TerminationReason::MaxIterations, max_iterations))
    }

    /// Build a partial result from the session history when interrupted by
    /// timeout or max iterations.
    fn build_partial_result(&self, reason: TerminationReason, iterations: u32) -> SubAgentResult {
        // Extract the last assistant message as raw_summary
        let raw_summary = self
            .session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        SubAgentResult {
            payload: SubAgentPayload {
                found_files: vec![],
                key_findings: vec![],
                raw_summary,
                confidence: None,
                termination_reason: reason,
                error: None,
            },
            estimated_tokens: 0,
            iterations_used: iterations,
        }
    }
}
