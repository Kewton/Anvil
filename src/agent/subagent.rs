//! Sub-agent execution loop.
//!
//! Provides [`SubAgentSession`] which runs an independent LLM loop with a
//! restricted tool set (read-only) within a sandboxed scope directory.

use crate::app::policy::{OFFLINE_BLOCK_PAYLOAD, check_offline_blocked};
use crate::config::EffectiveConfig;
use crate::contracts::{Finding, FixSliceProposal, SubAgentPayload, TerminationReason};
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

// ---------------------------------------------------------------------------
// FixSlice budget constants (Issue #291, DR1-004: code-level const)
// ---------------------------------------------------------------------------

/// Default value for the runtime `fixslice_max_iterations` knob (Issue #345).
///
/// Since Issue #345 the iteration cap is configurable. This const is kept so
/// tests and config defaults can reference the historical value in a single
/// place. Runtime callers should read
/// `EffectiveConfig::runtime::fixslice_max_iterations` instead.
pub const DEFAULT_FIXSLICE_MAX_ITERATIONS: u32 = 3;
/// Back-compat alias for `DEFAULT_FIXSLICE_MAX_ITERATIONS` (Issue #291 /
/// Issue #345). Kept so external callers that imported the old name continue
/// to compile; prefer the runtime config at the call site.
pub const FIXSLICE_MAX_ITERATIONS: u32 = DEFAULT_FIXSLICE_MAX_ITERATIONS;
/// Wall-clock timeout (seconds) for a FixSlice sub-agent run.
pub const FIXSLICE_TIMEOUT_SECS: u64 = 60;
/// Maximum allowed value for `max_lines` in agent.fix_slice input.
pub const MAX_FIXSLICE_LINES: u32 = 200;

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

// ---------------------------------------------------------------------------
// FixSlice sub-agent prompts (Issue #291, DR2-007)
// ---------------------------------------------------------------------------

const FIXSLICE_PROTOCOL_BASE: &str = r#"You are a sub-agent of Anvil, a local coding agent.

## Tool protocol
When you need to read files, respond using fenced blocks.

After you have analysed the target file, output your fix proposal as JSON inside:
```ANVIL_FINAL
{
  "target_path": "relative/path/to/file.rs",
  "start_line": 10,
  "end_line": 15,
  "replacement_content": "replacement lines here\n",
  "rationale": "Short explanation of the fix"
}
```

Rules:
- All paths must be relative (start with ./ or a directory name).
- Do not use any other tool syntax.
- Always include ANVIL_FINAL when you are done.
- You MUST output ANVIL_FINAL to signal completion.
- Output valid JSON in ANVIL_FINAL. The JSON must contain target_path, start_line, end_line, and replacement_content.
- start_line and end_line are 1-based, inclusive.
- Keep the replacement scope minimal — only the lines that need to change.

"#;

const FIXSLICE_ROLE_PROMPT: &str = r#"## Your role
You are a FixSlice sub-agent specializing in targeted, local code fixes.
- Use file.read to read the target file and understand the context.
- Identify the minimal set of lines that need to change.
- Produce a FixSliceProposal in ANVIL_FINAL with the exact line range and replacement.
- Keep the fix within the specified max_lines budget.
- You only have read-only access: file.read.
"#;

/// Options for sub-agent system prompt generation (Issue #162).
pub struct SubAgentPromptOptions<'a> {
    pub offline: bool,
    pub ui_language: Option<&'a str>,
}

/// Build a system prompt for a sub-agent of the given kind.
///
/// When `opts.offline` is `true`, web-related tool descriptions and prompts
/// are excluded from the Plan sub-agent prompt.
pub fn build_subagent_system_prompt(
    kind: &SubAgentKind,
    opts: &SubAgentPromptOptions<'_>,
) -> String {
    use crate::config::{effective_ui_language_code, language_constraint_prompt};

    // FixSlice uses its own protocol base; Explore/Plan share SUBAGENT_PROTOCOL_BASE.
    let mut prompt = String::new();
    match kind {
        SubAgentKind::Explore => {
            prompt.push_str(SUBAGENT_PROTOCOL_BASE);
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(TOOL_DESC_FILE_SEARCH);
            prompt.push_str(TOOL_DESC_GIT_STATUS);
            prompt.push_str(TOOL_DESC_GIT_DIFF);
            prompt.push_str(TOOL_DESC_GIT_LOG);
            prompt.push_str(EXPLORE_ROLE_PROMPT);
        }
        SubAgentKind::Plan => {
            prompt.push_str(SUBAGENT_PROTOCOL_BASE);
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(TOOL_DESC_FILE_SEARCH);
            if !opts.offline {
                prompt.push_str(TOOL_DESC_WEB_FETCH);
            }
            prompt.push_str(TOOL_DESC_GIT_STATUS);
            prompt.push_str(if opts.offline {
                PLAN_ROLE_PROMPT_OFFLINE
            } else {
                PLAN_ROLE_PROMPT
            });
        }
        SubAgentKind::FixSlice => {
            prompt.push_str(FIXSLICE_PROTOCOL_BASE);
            prompt.push_str(TOOL_DESC_FILE_READ);
            prompt.push_str(FIXSLICE_ROLE_PROMPT);
        }
    }

    // Language constraint (Issue #162)
    let lang = effective_ui_language_code(opts.ui_language);
    prompt.push_str(&language_constraint_prompt(lang));

    prompt
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Sub-agent kind (Explore, Plan, or FixSlice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentKind {
    Explore,
    Plan,
    /// Microtask fix-slice worker (Issue #291).
    FixSlice,
}

impl SubAgentKind {
    /// Convert a [`ToolInput`] variant to a [`SubAgentKind`], if applicable.
    pub fn from_tool_input(input: &ToolInput) -> Option<SubAgentKind> {
        match input {
            ToolInput::AgentExplore { .. } => Some(SubAgentKind::Explore),
            ToolInput::AgentPlan { .. } => Some(SubAgentKind::Plan),
            ToolInput::AgentFixSlice { .. } => Some(SubAgentKind::FixSlice),
            _ => None,
        }
    }
}

/// Why a FixSlice sub-agent finished without a usable `FixSliceProposal`
/// (Issue #345).
///
/// Surfaces the parse outcome to `handle_fixslice_result` so it can record a
/// granular `FixSliceFailureReason` instead of lumping everything into
/// `NoProposal`. `None` on `SubAgentResult::fix_proposal_failure` means the
/// worker either produced a usable proposal (possibly salvaged) or the result
/// came from a non-FixSlice kind where this field is irrelevant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixProposalParseFailure {
    /// Final response was empty (or whitespace-only) after trimming.
    EmptyFinal,
    /// Final response was non-empty but could not be parsed into a
    /// `FixSliceProposal`, even after the salvage pass that extracts the
    /// first embedded JSON object.
    ParseFailed,
    /// Worker terminated without emitting a final response of its own — the
    /// result was built from a partial/max-iterations exit path. Treated as
    /// distinct from `EmptyFinal` because it reflects loop exhaustion rather
    /// than a blank model output.
    NoFinal,
    /// The sub-agent hit the same-target `file.read` oscillation guard and
    /// aborted early (Issue #351, B1 shape). The worker kept re-reading the
    /// same target path without producing a proposal; the runtime aborts
    /// before the iteration budget is exhausted so the parent can classify
    /// the session as pack_gate_invalid.
    RepeatedReadLoop,
}

/// Outcome of `try_parse_fix_proposal` (Issue #345).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixProposalParseOutcome {
    /// The full `final_response` parsed as strict JSON.
    Parsed(FixSliceProposal),
    /// The first balanced `{...}` substring parsed as JSON after the strict
    /// parse failed.
    Salvaged(FixSliceProposal),
    /// `final_response` was blank after trimming.
    EmptyFinal,
    /// `final_response` was non-empty but no JSON object could be extracted.
    ParseFailed,
}

/// Extract a `FixSliceProposal` from a worker's final response (Issue #345).
///
/// Tries a strict parse first, then falls back to extracting the first
/// balanced `{...}` substring from the text. The depth scanner tracks JSON
/// string state so braces inside string literals (e.g. Rust code in
/// `replacement_content`) don't throw off the bracket count.
pub fn try_parse_fix_proposal(final_response: &str) -> FixProposalParseOutcome {
    let trimmed = final_response.trim();
    if trimmed.is_empty() {
        return FixProposalParseOutcome::EmptyFinal;
    }
    if let Ok(parsed) = serde_json::from_str::<FixSliceProposal>(trimmed) {
        return FixProposalParseOutcome::Parsed(parsed);
    }
    if let Some(slice) = extract_first_json_object(trimmed)
        && let Ok(parsed) = serde_json::from_str::<FixSliceProposal>(slice)
    {
        return FixProposalParseOutcome::Salvaged(parsed);
    }
    FixProposalParseOutcome::ParseFailed
}

/// Return the first balanced `{...}` substring from `text`, respecting JSON
/// string state so braces inside string literals don't unbalance the scan.
fn extract_first_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start: Option<usize> = None;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => {
                in_string = true;
            }
            b'{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        let s = start?;
                        return Some(&text[s..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Result returned by a successful sub-agent run.
// TODO(DR1-001): When a 4th sub-agent kind is added, refactor SubAgentResult
// into an enum to avoid accumulating Option fields.
pub struct SubAgentResult {
    /// 構造化ペイロード
    pub payload: SubAgentPayload,
    /// 推定トークン数
    pub estimated_tokens: usize,
    /// 使用したイテレーション数
    pub iterations_used: u32,
    /// FixSlice proposal extracted from ANVIL_FINAL (Issue #291).
    pub fix_proposal: Option<FixSliceProposal>,
    /// Why `fix_proposal` is `None` for FixSlice runs (Issue #345).
    ///
    /// `None` when the result produced a usable proposal or the run was not
    /// a FixSlice sub-agent. Populated by `run_turn` / `build_partial_result`
    /// for the FixSlice kind so `handle_fixslice_result` can pick a precise
    /// `FixSliceFailureReason` classification.
    pub fix_proposal_failure: Option<FixProposalParseFailure>,
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
            diff_summary: None,
            edit_detail: None,
            rolled_back: false,
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
            diff_summary: None,
            edit_detail: None,
            rolled_back: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Outcome of a single sub-agent turn.
enum TurnOutcome {
    /// The sub-agent produced a final answer.
    Finished(Box<SubAgentResult>),
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
    /// Most recent `file.read` target path observed in a FixSlice iteration
    /// (Issue #351). Used together with `same_target_read_count` to detect
    /// same-path read oscillation inside the sub-agent loop.
    last_file_read_target: Option<String>,
    /// Number of consecutive FixSlice iterations whose primary `file.read`
    /// call targeted `last_file_read_target` (Issue #351).
    same_target_read_count: u32,
}

/// Default threshold (consecutive iterations) for the same-target
/// `file.read` oscillation guard inside the FixSlice sub-agent loop
/// (Issue #351 / Issue #353).
///
/// Runtime callers should read
/// `EffectiveConfig::runtime::fixslice_no_progress_detector` instead —
/// this constant only pins the default used when nothing else is set.
///
/// Issue #353: cycle-11 traces showed the previous default (`2`) killed
/// legitimate A1/A2 exploration — qwen3.5:122b frequently needs more than
/// two same-path reads to reach a proposal. Raised to `5` so the guard
/// still catches the B1 runaway shape but leaves normal exploration
/// intact. A value of `0` disables the guard entirely.
pub const DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD: u32 = 5;

/// Back-compat alias for [`DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD`]
/// (Issue #351 / Issue #353). Kept so existing test imports continue to
/// compile; new code should read the runtime config at the call site.
pub const FIXSLICE_REPEATED_READ_THRESHOLD: u32 = DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD;

/// Pure helper for the same-target `file.read` oscillation guard (Issue
/// #351). Returns `true` when `current_target` matches `last_target` and
/// the resulting consecutive count reaches `threshold`.
///
/// Exposed as a pure function so the guard can be unit-tested without
/// standing up a provider / tool-executor stack.
pub fn is_repeated_read_loop(
    current_target: Option<&str>,
    last_target: Option<&str>,
    prior_consecutive_count: u32,
    threshold: u32,
) -> bool {
    match (current_target, last_target) {
        (Some(c), Some(l)) if c == l => prior_consecutive_count + 1 >= threshold,
        _ => false,
    }
}

/// Return the first `file.read` target path from a slice of tool calls, if
/// any (Issue #351). The FixSlice sub-agent is limited to `file.read`, so
/// "first match" is the simplest stable fingerprint for the iteration's
/// dominant target.
pub fn first_file_read_target(tool_calls: &[ToolCallRequest]) -> Option<String> {
    tool_calls.iter().find_map(|call| match &call.input {
        ToolInput::FileRead { path } => Some(path.clone()),
        _ => None,
    })
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
            SubAgentKind::FixSlice => registry.register_fixslice_tools(),
        }

        // 3. Dedicated system prompt
        let prompt_opts = SubAgentPromptOptions {
            offline: config.mode.offline,
            ui_language: config.runtime.ui_language.as_deref(),
        };
        let system_prompt = build_subagent_system_prompt(&kind, &prompt_opts);

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
            last_file_read_target: None,
            same_target_read_count: 0,
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
        let mut request = BasicAgentLoop::build_turn_request(
            self.effective_model(),
            &self.session,
            true,
            self.effective_context_window(),
            &self.system_prompt,
            self.config.runtime.context_budget,
        );
        request.max_output_tokens = self.config.runtime.max_output_tokens;

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
        // Note: ANVIL_FINAL file-modification guard is NOT applied to sub-agents.
        // Sub-agents (Explore/Plan) are read-only and not expected to modify files.
        // The guard is only enforced in the main agent loop (src/app/agentic.rs).
        if structured.tool_calls.is_empty() {
            let tokens = crate::contracts::tokens::estimate_tokens(
                &token_buffer,
                crate::contracts::tokens::ContentKind::Text,
            );

            // FixSlice: parse ANVIL_FINAL as FixSliceProposal.
            // Issue #291, DR1-007: strict JSON parse.
            // Issue #345: fall back to a salvage pass and report the parse
            // outcome so the parent can classify `no_proposal` sub-classes.
            if self.kind == SubAgentKind::FixSlice {
                let outcome = try_parse_fix_proposal(&structured.final_response);
                let (fix_proposal, fix_proposal_failure, summary) = match outcome {
                    FixProposalParseOutcome::Parsed(p) => (
                        Some(p),
                        None,
                        "fix-slice proposal parsed successfully".to_string(),
                    ),
                    FixProposalParseOutcome::Salvaged(p) => (
                        Some(p),
                        None,
                        "fix-slice proposal salvaged from embedded JSON".to_string(),
                    ),
                    FixProposalParseOutcome::EmptyFinal => (
                        None,
                        Some(FixProposalParseFailure::EmptyFinal),
                        "fix-slice proposal missing: empty final response".to_string(),
                    ),
                    FixProposalParseOutcome::ParseFailed => (
                        None,
                        Some(FixProposalParseFailure::ParseFailed),
                        format!(
                            "fix-slice proposal parse failed: {}",
                            &structured
                                .final_response
                                .chars()
                                .take(200)
                                .collect::<String>()
                        ),
                    ),
                };
                let payload = SubAgentPayload::fallback(summary, TerminationReason::Completed);
                return Ok(TurnOutcome::Finished(Box::new(SubAgentResult {
                    payload,
                    estimated_tokens: tokens,
                    iterations_used: self.iterations_used,
                    fix_proposal,
                    fix_proposal_failure,
                })));
            }

            let payload = parse_final_response_to_payload(&structured.final_response);
            return Ok(TurnOutcome::Finished(Box::new(SubAgentResult {
                payload,
                estimated_tokens: tokens,
                iterations_used: self.iterations_used,
                fix_proposal: None,
                fix_proposal_failure: None,
            })));
        }

        // Issue #351: same-target `file.read` oscillation guard (FixSlice only).
        // Issue #353: threshold is now configurable via
        // `runtime.fixslice_no_progress_detector` (0 = disabled).
        //
        // Cycle-10 traces showed FixSlice workers burning the full iteration
        // budget while re-reading the exact same target path without ever
        // producing a proposal. Detect that shape here, before the executor
        // runs another round of IO against the same file, and abort with a
        // dedicated `RepeatedReadLoop` classification so the parent can
        // classify the session as pack_gate_invalid instead of waiting for
        // the runner timeout.
        if self.kind == SubAgentKind::FixSlice {
            let detector_threshold = self.config.runtime.fixslice_no_progress_detector;
            let current_target = first_file_read_target(&structured.tool_calls);
            let triggered = detector_threshold > 0
                && is_repeated_read_loop(
                    current_target.as_deref(),
                    self.last_file_read_target.as_deref(),
                    self.same_target_read_count,
                    detector_threshold,
                );
            match current_target.as_deref() {
                Some(path) if self.last_file_read_target.as_deref() == Some(path) => {
                    self.same_target_read_count += 1;
                }
                Some(path) => {
                    self.last_file_read_target = Some(path.to_string());
                    self.same_target_read_count = 1;
                }
                None => {
                    self.last_file_read_target = None;
                    self.same_target_read_count = 0;
                }
            }
            if triggered {
                let tokens = crate::contracts::tokens::estimate_tokens(
                    &token_buffer,
                    crate::contracts::tokens::ContentKind::Text,
                );
                let target = self.last_file_read_target.clone().unwrap_or_default();
                eprintln!(
                    "[subagent:fix_slice] repeated-read loop detected (path={target}); \
                     aborting after {} iteration(s)",
                    self.iterations_used
                );
                let summary = format!(
                    "fix-slice worker aborted: repeated-read loop on target '{target}' \
                     after {} iteration(s)",
                    self.iterations_used
                );
                let payload = SubAgentPayload::fallback(summary, TerminationReason::LoopDetected);
                return Ok(TurnOutcome::Finished(Box::new(SubAgentResult {
                    payload,
                    estimated_tokens: tokens,
                    iterations_used: self.iterations_used,
                    fix_proposal: None,
                    fix_proposal_failure: Some(FixProposalParseFailure::RepeatedReadLoop),
                })));
            }
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
                    diff_summary: None,
                    edit_detail: None,
                    rolled_back: false,
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
            SubAgentKind::FixSlice => "fix_slice",
        };
        eprintln!("[subagent:{kind_label}] Starting...");
        let start = Instant::now();
        let (max_iterations, timeout) = if self.kind == SubAgentKind::FixSlice {
            // Issue #345: runtime-configurable iteration cap.
            (
                self.config.runtime.fixslice_max_iterations,
                Duration::from_secs(FIXSLICE_TIMEOUT_SECS),
            )
        } else {
            (
                self.config.runtime.subagent_max_iterations,
                Duration::from_secs(self.config.runtime.subagent_timeout_secs),
            )
        };

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
                TurnOutcome::Finished(result) => return Ok(*result),
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

        // Issue #345: attempt to salvage a proposal from the last assistant
        // message even on a partial exit path. Timeout / max-iterations paths
        // sometimes have a valid-looking ANVIL_FINAL in the last turn that
        // the strict main path never got to parse.
        let (fix_proposal, fix_proposal_failure) = if self.kind == SubAgentKind::FixSlice {
            match try_parse_fix_proposal(&raw_summary) {
                FixProposalParseOutcome::Parsed(p) | FixProposalParseOutcome::Salvaged(p) => {
                    (Some(p), None)
                }
                FixProposalParseOutcome::EmptyFinal => {
                    (None, Some(FixProposalParseFailure::NoFinal))
                }
                FixProposalParseOutcome::ParseFailed => {
                    (None, Some(FixProposalParseFailure::ParseFailed))
                }
            }
        } else {
            (None, None)
        };

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
            fix_proposal,
            fix_proposal_failure,
        }
    }
}
