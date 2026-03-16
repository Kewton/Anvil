//! Tool registry, validation, and local execution.
//!
//! Tools are declared as [`ToolSpec`] entries in a [`ToolRegistry`], validated
//! through a permission and plan-mode pipeline, and executed by
//! [`LocalToolExecutor`] within a sandboxed workspace root.

use crate::config::{RuntimeConfig, WebSearchProvider};
use crate::contracts::ToolLogView;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionClass {
    Safe,
    Confirm,
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    ParallelSafe,
    SequentialOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionClass {
    ReadOnly,
    Mutating,
    Network,
    Interactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanModePolicy {
    Allowed,
    AllowedWithScope,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackPolicy {
    None,
    CheckpointBeforeWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    FileRead,
    FileWrite,
    FileSearch,
    ShellExec,
    WebFetch,
    WebSearch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolInput {
    FileRead { path: String },
    FileWrite { path: String, content: String },
    FileSearch { root: String, pattern: String },
    ShellExec { command: String },
    WebFetch { url: String },
    WebSearch { query: String },
}

impl ToolInput {
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::FileRead { .. } => ToolKind::FileRead,
            Self::FileWrite { .. } => ToolKind::FileWrite,
            Self::FileSearch { .. } => ToolKind::FileSearch,
            Self::ShellExec { .. } => ToolKind::ShellExec,
            Self::WebFetch { .. } => ToolKind::WebFetch,
            Self::WebSearch { .. } => ToolKind::WebSearch,
        }
    }

    /// Parse a JSON value into a `ToolInput` given a tool name.
    ///
    /// This centralises all field-name knowledge in the `ToolInput` enum
    /// definition, keeping the agent parser thin.
    pub fn from_json(tool_name: &str, value: &serde_json::Value) -> Result<ToolInput, String> {
        match tool_name {
            "file.write" => Ok(ToolInput::FileWrite {
                path: value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing path in file.write tool block".to_string())?
                    .to_string(),
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing content in file.write tool block".to_string())?
                    .to_string(),
            }),
            "file.read" => Ok(ToolInput::FileRead {
                path: value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing path in file.read tool block".to_string())?
                    .to_string(),
            }),
            "file.search" => Ok(ToolInput::FileSearch {
                root: value
                    .get("root")
                    .or_else(|| value.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing root in file.search tool block".to_string())?
                    .to_string(),
                pattern: value
                    .get("pattern")
                    .or_else(|| value.get("content"))
                    .or_else(|| value.get("query"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing pattern in file.search tool block".to_string())?
                    .to_string(),
            }),
            "shell.exec" | "shell" => Ok(ToolInput::ShellExec {
                command: value
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing command in shell.exec tool block".to_string())?
                    .to_string(),
            }),
            "web.fetch" => Ok(ToolInput::WebFetch {
                url: value
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing url in web.fetch tool block".to_string())?
                    .to_string(),
            }),
            "web.search" => Ok(ToolInput::WebSearch {
                query: value
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing query in web.search tool block".to_string())?
                    .to_string(),
            }),
            other => Err(format!("unsupported tool in ANVIL_TOOL block: {other}")),
        }
    }

    /// Attempt to repair a malformed JSON block into a `ToolInput`.
    ///
    /// Uses simple string extraction as a fallback when JSON parsing fails.
    pub fn repair_from_block(
        tool_name: &str,
        block: &str,
        extract_simple: fn(&str, &str) -> Option<String>,
        extract_trailing: fn(&str, &str) -> Option<String>,
    ) -> Option<ToolInput> {
        match tool_name {
            "file.write" => Some(ToolInput::FileWrite {
                path: extract_simple(block, "path")?,
                content: extract_trailing(block, "content")?,
            }),
            "file.read" => Some(ToolInput::FileRead {
                path: extract_simple(block, "path")?,
            }),
            "file.search" => Some(ToolInput::FileSearch {
                root: extract_simple(block, "root").or_else(|| extract_simple(block, "path"))?,
                pattern: extract_simple(block, "pattern")
                    .or_else(|| extract_simple(block, "content"))
                    .or_else(|| extract_simple(block, "query"))?,
            }),
            "shell.exec" | "shell" => Some(ToolInput::ShellExec {
                command: extract_simple(block, "command")?,
            }),
            "web.fetch" => Some(ToolInput::WebFetch {
                url: extract_simple(block, "url")?,
            }),
            "web.search" => Some(ToolInput::WebSearch {
                query: extract_simple(block, "query")?,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: ToolInput,
}

impl ToolCallRequest {
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: ToolInput,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub version: u32,
    pub name: String,
    pub kind: ToolKind,
    pub execution_class: ExecutionClass,
    pub permission_class: PermissionClass,
    pub execution_mode: ExecutionMode,
    pub plan_mode: PlanModePolicy,
    pub rollback_policy: RollbackPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolExecutionPolicy {
    pub approval_required: bool,
    pub allow_restricted: bool,
    pub plan_mode: bool,
    pub plan_scope_granted: bool,
}

impl Default for ToolExecutionPolicy {
    fn default() -> Self {
        Self {
            approval_required: true,
            allow_restricted: false,
            plan_mode: false,
            plan_scope_granted: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub tool_call_id: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedToolCall {
    pub spec: ToolSpec,
    pub request: ToolCallRequest,
    pub approved: bool,
}

impl ValidatedToolCall {
    pub fn approval_required(&self, approval_mode: bool) -> Option<ApprovalRequest> {
        if !approval_mode {
            return None;
        }

        let effective = effective_permission_class(&self.request.input, &self.spec);
        if effective == PermissionClass::Safe {
            return None;
        }

        Some(ApprovalRequest {
            tool_call_id: self.request.tool_call_id.clone(),
            tool_name: self.spec.name.clone(),
        })
    }

    pub fn approve(mut self) -> Self {
        self.approved = true;
        self
    }

    pub fn into_execution_request(
        self,
        policy: ToolExecutionPolicy,
    ) -> Result<ToolExecutionRequest, ToolExecutionError> {
        if policy.plan_mode {
            match self.spec.plan_mode {
                PlanModePolicy::Allowed => {}
                PlanModePolicy::AllowedWithScope if policy.plan_scope_granted => {}
                PlanModePolicy::AllowedWithScope => {
                    return Err(ToolExecutionError::PlanModeScopeRequired(
                        self.spec.name.clone(),
                    ));
                }
                PlanModePolicy::Blocked => {
                    return Err(ToolExecutionError::PlanModeBlocked(self.spec.name.clone()));
                }
            }
        }

        if self.spec.permission_class == PermissionClass::Restricted && !policy.allow_restricted {
            return Err(ToolExecutionError::RestrictedTool(self.spec.name.clone()));
        }

        if self.approval_required(policy.approval_required).is_some() && !self.approved {
            return Err(ToolExecutionError::ApprovalRequired(
                self.request.tool_call_id.clone(),
            ));
        }

        Ok(ToolExecutionRequest {
            tool_call_id: self.request.tool_call_id.clone(),
            spec: self.spec,
            input: self.request.input,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionRequest {
    pub tool_call_id: String,
    pub spec: ToolSpec,
    pub input: ToolInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionPayload {
    None,
    Text(String),
    Paths(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolExecutionStatus,
    pub summary: String,
    pub payload: ToolExecutionPayload,
    pub artifacts: Vec<String>,
    pub elapsed_ms: u128,
}

impl ToolExecutionResult {
    pub fn to_tool_log_view(&self) -> ToolLogView {
        let action = match self.status {
            ToolExecutionStatus::Completed => "completed",
            ToolExecutionStatus::Failed => "failed",
            ToolExecutionStatus::Interrupted => "interrupted",
        };

        ToolLogView {
            tool_name: self.tool_name.clone(),
            action: action.to_string(),
            target: self.summary.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionError {
    ApprovalRequired(String),
    RestrictedTool(String),
    PlanModeBlocked(String),
    PlanModeScopeRequired(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolValidationError {
    UnknownTool,
    InputKindMismatch,
    MissingRequiredField(String),
    InvalidFieldValue { field: String, reason: String },
    DangerousCommand { command: String, reason: String },
}

#[derive(Debug, Default)]
pub struct ToolRegistry {
    specs: HashMap<String, ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            specs: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.get(name)
    }

    pub fn register(&mut self, spec: ToolSpec) {
        self.specs.insert(spec.name.clone(), spec);
    }

    pub fn register_file_read(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "file.read".to_string(),
            kind: ToolKind::FileRead,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_file_write(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "file.write".to_string(),
            kind: ToolKind::FileWrite,
            execution_class: ExecutionClass::Mutating,
            permission_class: PermissionClass::Confirm,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::AllowedWithScope,
            rollback_policy: RollbackPolicy::CheckpointBeforeWrite,
        });
    }

    pub fn register_file_search(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "file.search".to_string(),
            kind: ToolKind::FileSearch,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_shell_exec(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "shell.exec".to_string(),
            kind: ToolKind::ShellExec,
            execution_class: ExecutionClass::Interactive,
            permission_class: PermissionClass::Confirm,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::AllowedWithScope,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_web_fetch(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "web.fetch".to_string(),
            kind: ToolKind::WebFetch,
            execution_class: ExecutionClass::Network,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_web_search(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "web.search".to_string(),
            kind: ToolKind::WebSearch,
            execution_class: ExecutionClass::Network,
            permission_class: PermissionClass::Confirm,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_standard_tools(&mut self) {
        self.register_file_read();
        self.register_file_write();
        self.register_file_search();
        self.register_shell_exec();
        self.register_web_fetch();
        self.register_web_search();
    }

    pub fn validate(
        &self,
        request: ToolCallRequest,
    ) -> Result<ValidatedToolCall, ToolValidationError> {
        let spec = self
            .specs
            .get(&request.tool_name)
            .cloned()
            .ok_or(ToolValidationError::UnknownTool)?;

        if spec.kind != request.input.kind() {
            return Err(ToolValidationError::InputKindMismatch);
        }

        validate_required_fields(&request.input)?;

        Ok(ValidatedToolCall {
            spec,
            request,
            approved: false,
        })
    }
}

// Rate limit intervals per provider.
const RATE_LIMIT_DUCKDUCKGO: Duration = Duration::from_secs(2);
const RATE_LIMIT_SERPER_API: Duration = Duration::from_secs(1);

pub struct LocalToolExecutor {
    root: PathBuf,
    last_web_search: Option<Instant>,
    web_search_min_interval: Duration,
    web_search_provider: WebSearchProvider,
    serper_api_key: Option<String>,
}

#[derive(Debug)]
pub enum ToolRuntimeError {
    InvalidPath(String),
    Io(String),
}

impl Display for ToolRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(f, "invalid tool path: {path}"),
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ToolRuntimeError {}

impl LocalToolExecutor {
    pub fn new(root: impl Into<PathBuf>, config: &RuntimeConfig) -> Self {
        let interval = match config.web_search_provider {
            WebSearchProvider::DuckDuckGo => RATE_LIMIT_DUCKDUCKGO,
            WebSearchProvider::SerperApi => RATE_LIMIT_SERPER_API,
        };
        Self {
            root: root.into(),
            last_web_search: None,
            web_search_min_interval: interval,
            web_search_provider: config.web_search_provider,
            serper_api_key: config.serper_api_key.clone(),
        }
    }

    /// Create an executor without rate limiting (for tests).
    pub fn new_without_rate_limit(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            last_web_search: None,
            web_search_min_interval: Duration::ZERO,
            web_search_provider: WebSearchProvider::DuckDuckGo,
            serper_api_key: None,
        }
    }

    pub fn execute(
        &mut self,
        request: ToolExecutionRequest,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let started = Instant::now();
        match request.input {
            ToolInput::FileRead { ref path } => self.execute_file_read(&request, path, started),
            ToolInput::FileWrite {
                ref path,
                ref content,
            } => self.execute_file_write(&request, path, content, started),
            ToolInput::FileSearch {
                ref root,
                ref pattern,
            } => self.execute_file_search(&request, root, pattern, started),
            ToolInput::WebFetch { ref url } => self.execute_web_fetch(&request, url, started),
            ToolInput::ShellExec { ref command } => {
                self.execute_shell_exec(&request, command, started)
            }
            ToolInput::WebSearch { ref query } => self.execute_web_search(&request, query, started),
        }
    }

    fn execute_file_read(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        let content = if resolved.is_dir() {
            render_directory_listing(&resolved)?
        } else {
            fs::read_to_string(&resolved).map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "file.read failed for {}: {err}",
                    resolved.display()
                ))
            })?
        };
        Ok(build_completed_result(
            request,
            path.to_string(),
            ToolExecutionPayload::Text(content),
            vec![resolved.display().to_string()],
            started,
        ))
    }

    fn execute_file_write(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        content: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "file.write failed to create parent {}: {err}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&resolved, content).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.write failed for {}: {err}",
                resolved.display()
            ))
        })?;
        Ok(build_completed_result(
            request,
            path.to_string(),
            ToolExecutionPayload::None,
            vec![resolved.display().to_string()],
            started,
        ))
    }

    fn execute_file_search(
        &self,
        request: &ToolExecutionRequest,
        root: &str,
        pattern: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved_root = self.resolve_path(root)?;
        let mut matches = Vec::new();
        collect_search_matches(&resolved_root, pattern, &mut matches)?;
        Ok(build_completed_result(
            request,
            format!("{root} :: {pattern}"),
            ToolExecutionPayload::Paths(matches.clone()),
            matches,
            started,
        ))
    }

    fn execute_web_fetch(
        &self,
        request: &ToolExecutionRequest,
        url: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let output = std::process::Command::new("curl")
            .args([
                "-s",
                "-L",
                "--fail",
                "--max-time",
                "30",
                "--max-filesize",
                "1048576",
                "--max-redirs",
                "5",
                "--",
                url,
            ])
            .output()
            .map_err(|err| {
                ToolRuntimeError::Io(format!("web.fetch failed to spawn curl: {err}"))
            })?;

        if output.status.success() {
            let body = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(build_completed_result(
                request,
                url.to_string(),
                ToolExecutionPayload::Text(body),
                Vec::new(),
                started,
            ))
        } else {
            let stderr_msg = String::from_utf8_lossy(&output.stderr).to_string();
            Err(ToolRuntimeError::Io(format!(
                "web.fetch failed for {url}: {stderr_msg}"
            )))
        }
    }

    fn execute_shell_exec(
        &self,
        request: &ToolExecutionRequest,
        command: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        use std::io::{BufRead, Write as _};

        let _ = writeln!(std::io::stderr(), "\n  $ {command}");

        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| ToolRuntimeError::Io(format!("shell.exec failed to spawn: {err}")))?;

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_thread = std::thread::spawn(move || {
            let mut captured = String::new();
            if let Some(out) = stdout_handle {
                let reader = std::io::BufReader::new(out);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let _ = writeln!(std::io::stderr(), "  {line}");
                    captured.push_str(&line);
                    captured.push('\n');
                }
            }
            captured
        });

        let stderr_thread = std::thread::spawn(move || {
            let mut captured = String::new();
            if let Some(err) = stderr_handle {
                let reader = std::io::BufReader::new(err);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let _ = writeln!(std::io::stderr(), "  {line}");
                    captured.push_str(&line);
                    captured.push('\n');
                }
            }
            captured
        });

        let exit_status = child.wait().ok();
        let stdout_buf = stdout_thread.join().unwrap_or_default();
        let stderr_buf = stderr_thread.join().unwrap_or_default();

        let combined = if stderr_buf.trim().is_empty() {
            stdout_buf
        } else if stdout_buf.trim().is_empty() {
            stderr_buf
        } else {
            format!("{stdout_buf}--- stderr ---\n{stderr_buf}")
        };

        let success = exit_status.is_some_and(|s| s.success());
        let status = if success {
            ToolExecutionStatus::Completed
        } else {
            ToolExecutionStatus::Failed
        };
        let summary = if success {
            format!("shell.exec completed: {command}")
        } else {
            format!(
                "shell.exec failed (exit {}): {command}",
                exit_status.and_then(|s| s.code()).unwrap_or(-1)
            )
        };
        Ok(ToolExecutionResult {
            tool_call_id: request.tool_call_id.clone(),
            tool_name: request.spec.name.clone(),
            status,
            summary,
            payload: ToolExecutionPayload::Text(combined),
            artifacts: Vec::new(),
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    fn execute_web_search(
        &mut self,
        request: &ToolExecutionRequest,
        query: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        self.enforce_rate_limit();

        match self.web_search_provider {
            WebSearchProvider::DuckDuckGo => {
                self.execute_web_search_duckduckgo(request, query, started)
            }
            WebSearchProvider::SerperApi => self.execute_web_search_serper(request, query, started),
        }
    }

    fn execute_web_search_duckduckgo(
        &self,
        request: &ToolExecutionRequest,
        query: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let encoded_query = percent_encode(query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded_query}");

        let output = std::process::Command::new("curl")
            .args([
                "-s",
                "-L",
                "--fail",
                "--max-time",
                "15",
                "-H",
                "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                "--",
            ])
            .arg(&url)
            .output()
            .map_err(|err| {
                ToolRuntimeError::Io(format!("web.search failed to spawn curl: {err}"))
            })?;

        if !output.status.success() {
            return Err(ToolRuntimeError::Io(
                "DuckDuckGo search failed. CAPTCHA/rate limit may be active. Please wait and retry."
                    .to_string(),
            ));
        }

        let body = String::from_utf8_lossy(&output.stdout).to_string();

        // Parse results using regex
        let results = parse_duckduckgo_results(&body);

        // CAPTCHA / bot detection
        if results.is_empty() {
            let lower = body.to_ascii_lowercase();
            let has_result_elements = lower.contains("result__a");
            if !has_result_elements && (lower.contains("captcha") || lower.contains("bot")) {
                return Err(ToolRuntimeError::Io(
                    "DuckDuckGo search blocked by CAPTCHA/rate limit. Please wait and retry."
                        .to_string(),
                ));
            }
        }

        let formatted = format_search_results(&results);

        Ok(build_completed_result(
            request,
            format!("web.search: {query}"),
            ToolExecutionPayload::Text(formatted),
            Vec::new(),
            started,
        ))
    }

    fn execute_web_search_serper(
        &self,
        request: &ToolExecutionRequest,
        query: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let api_key = self.serper_api_key.as_deref().ok_or_else(|| {
            ToolRuntimeError::Io("SerperAPI search failed. SERPER_API_KEY is not set.".to_string())
        })?;

        let body = serde_json::json!({"q": query}).to_string();
        let header_value = format!("X-API-KEY: {api_key}");

        let output = std::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "-",
                "-w",
                "\n%{http_code}",
                "--max-time",
                "10",
                "-H",
                &header_value,
                "-H",
                "Content-Type: application/json",
                "-d",
                &body,
                "--",
                "https://google.serper.dev/search",
            ])
            .output()
            .map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "web.search (SerperAPI) failed to spawn curl: {err}"
                ))
            })?;

        if !output.status.success() {
            let stderr_msg = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(ToolRuntimeError::Io(format!(
                "Failed to reach SerperAPI. Check your network connection. {stderr_msg}"
            )));
        }

        let raw_output = String::from_utf8_lossy(&output.stdout).to_string();
        // Extract HTTP status code from the last line (appended by -w '\n%{http_code}')
        let (response_body, http_code) = match raw_output.rsplit_once('\n') {
            Some((body, code)) => (body.to_string(), code.trim().to_string()),
            None => (raw_output, String::new()),
        };

        match http_code.as_str() {
            "200" => {} // success
            "401" | "403" => {
                return Err(ToolRuntimeError::Io(
                    "Invalid or expired SerperAPI key.".to_string(),
                ));
            }
            "429" => {
                return Err(ToolRuntimeError::Io(
                    "SerperAPI rate limit exceeded. Please wait and retry.".to_string(),
                ));
            }
            code => {
                return Err(ToolRuntimeError::Io(format!(
                    "SerperAPI request failed with HTTP {code}."
                )));
            }
        }
        let results = parse_serper_results(&response_body);
        let formatted = format_search_results(&results);

        Ok(build_completed_result(
            request,
            format!("web.search: {query}"),
            ToolExecutionPayload::Text(formatted),
            Vec::new(),
            started,
        ))
    }

    fn enforce_rate_limit(&mut self) {
        if let Some(prev) = self.last_web_search {
            let elapsed = prev.elapsed();
            if elapsed < self.web_search_min_interval {
                std::thread::sleep(self.web_search_min_interval - elapsed);
            }
        }
        self.last_web_search = Some(Instant::now());
    }

    fn resolve_path(&self, raw: &str) -> Result<PathBuf, ToolRuntimeError> {
        let candidate = Path::new(raw);
        if candidate.is_absolute() {
            return Err(ToolRuntimeError::InvalidPath(raw.to_string()));
        }
        if candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(ToolRuntimeError::InvalidPath(raw.to_string()));
        }
        let joined = self.root.join(candidate);
        // If the path exists, canonicalize to resolve symlinks and verify
        // the result is still within the sandbox root.
        if joined.exists() {
            let canonical = fs::canonicalize(&joined).map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "failed to resolve path {}: {err}",
                    joined.display()
                ))
            })?;
            let root_canonical = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
            if !canonical.starts_with(&root_canonical) {
                return Err(ToolRuntimeError::InvalidPath(format!(
                    "{raw} resolves outside sandbox"
                )));
            }
        }
        Ok(joined)
    }
}

/// Build a [`ToolExecutionResult`] with `Completed` status.
///
/// Centralises the boilerplate shared by every successful execution path.
fn build_completed_result(
    request: &ToolExecutionRequest,
    summary: String,
    payload: ToolExecutionPayload,
    artifacts: Vec<String>,
    started: Instant,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_call_id: request.tool_call_id.clone(),
        tool_name: request.spec.name.clone(),
        status: ToolExecutionStatus::Completed,
        summary,
        payload,
        artifacts,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParallelExecutionPlan {
    pub calls: Vec<ValidatedToolCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParallelExecutionPlanError {
    ApprovalRequired(String),
    SequentialOnly(String),
}

impl ParallelExecutionPlan {
    pub fn build(
        calls: Vec<ValidatedToolCall>,
        policy: ToolExecutionPolicy,
    ) -> Result<Self, ParallelExecutionPlanError> {
        for call in &calls {
            if policy.approval_required
                && call.spec.permission_class != PermissionClass::Safe
                && !call.approved
            {
                return Err(ParallelExecutionPlanError::ApprovalRequired(
                    call.request.tool_call_id.clone(),
                ));
            }

            if call.spec.execution_mode != ExecutionMode::ParallelSafe {
                return Err(ParallelExecutionPlanError::SequentialOnly(
                    call.spec.name.clone(),
                ));
            }
        }

        Ok(Self { calls })
    }
}

fn validate_required_fields(input: &ToolInput) -> Result<(), ToolValidationError> {
    match input {
        ToolInput::FileRead { path } => {
            if path.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "path".to_string(),
                ));
            }
        }
        ToolInput::FileWrite { path, content } => {
            if path.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "path".to_string(),
                ));
            }
            if content.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "content".to_string(),
                ));
            }
        }
        ToolInput::FileSearch { root, pattern } => {
            if root.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "root".to_string(),
                ));
            }
            if pattern.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "pattern".to_string(),
                ));
            }
        }
        ToolInput::ShellExec { command } => {
            if command.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "command".to_string(),
                ));
            }
            validate_shell_command_safety(command)?;
        }
        ToolInput::WebFetch { url } => {
            if url.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField("url".to_string()));
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "url".to_string(),
                    reason: "must start with http:// or https://".to_string(),
                });
            }
        }
        ToolInput::WebSearch { query } => {
            if query.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "query".to_string(),
                ));
            }
            const MAX_QUERY_LENGTH: usize = 500;
            if query.len() > MAX_QUERY_LENGTH {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "query".to_string(),
                    reason: format!(
                        "query length {} exceeds maximum of {} characters",
                        query.len(),
                        MAX_QUERY_LENGTH
                    ),
                });
            }
        }
    }

    Ok(())
}

/// Reject shell commands that contain dangerous patterns.
///
/// This is a defence-in-depth measure.  The primary protection is the
/// `Restricted` permission class which blocks `shell.exec` by default.
fn validate_shell_command_safety(command: &str) -> Result<(), ToolValidationError> {
    /// Patterns that are unconditionally blocked regardless of context.
    const BLOCKED_PATTERNS: &[(&str, &str)] = &[
        ("rm -rf /", "recursive deletion of root filesystem"),
        ("rm -rf ~", "recursive deletion of home directory"),
        (
            "mkfs",
            "filesystem formatting — destroys all data on device",
        ),
        (
            "dd if=",
            "raw disk write — can overwrite partitions or boot sectors",
        ),
        (":(){", "fork bomb — exhausts system process table"),
        (
            ">(",
            "process substitution — can be used for command injection",
        ),
    ];

    /// Git sub-commands where --no-verify / -n is blocked.
    const GIT_NO_VERIFY_BLOCKED: &[&str] = &["commit", "push", "merge"];

    let lower = command.to_ascii_lowercase();

    for (pattern, reason) in BLOCKED_PATTERNS {
        if lower.contains(pattern) {
            return Err(ToolValidationError::DangerousCommand {
                command: command.to_string(),
                reason: reason.to_string(),
            });
        }
    }

    // Block --no-verify / -n on git commands that support hook bypass.
    if lower.starts_with("git ") {
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let is_blocked_subcommand = tokens
            .get(1)
            .is_some_and(|sub| GIT_NO_VERIFY_BLOCKED.contains(sub));
        if is_blocked_subcommand {
            let has_no_verify = tokens.contains(&"--no-verify");
            let has_short_n = tokens.contains(&"-n");
            if has_no_verify {
                return Err(ToolValidationError::DangerousCommand {
                    command: command.to_string(),
                    reason: "skipping git hooks can bypass safety checks".to_string(),
                });
            }
            if has_short_n {
                return Err(ToolValidationError::DangerousCommand {
                    command: command.to_string(),
                    reason:
                        "skipping git hooks can bypass safety checks (-n is short for --no-verify)"
                            .to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Determine whether a shell command is safe enough to auto-approve.
///
/// Commands that pass this check are promoted from `Confirm` to `Safe`
/// by [`effective_permission_class`], skipping the approval prompt.
pub fn is_safe_shell_command(command: &str) -> bool {
    let trimmed = command.trim();

    // Reject command chaining / injection vectors
    if trimmed.contains('|')
        || trimmed.contains(';')
        || trimmed.contains('`')
        || trimmed.contains("$(")
        || trimmed.contains("${")
        || trimmed.contains('\n')
        || trimmed.contains("&&")
        || trimmed.contains('>')
        || trimmed.contains('<')
    {
        return false;
    }

    // gh api: GET-only is safe (token-split based flag detection)
    if trimmed.starts_with("gh api ") {
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();

        // Flags that imply a mutating request by their mere presence.
        const BODY_FLAGS: &[&str] = &["-f", "--field", "-F", "--raw-field", "--input"];

        // Combined flag=value forms that imply mutation.
        const MUTATION_COMBINED: &[&str] = &[
            "-XPOST",
            "-XPUT",
            "-XPATCH",
            "-XDELETE",
            "--method=POST",
            "--method=PUT",
            "--method=PATCH",
            "--method=DELETE",
            "--input=",
            "-f=",
            "--field=",
            "-F=",
            "--raw-field=",
        ];

        for (i, token) in tokens.iter().enumerate() {
            // Body/field flags always imply mutation (POST is the gh default).
            if BODY_FLAGS.iter().any(|f| token == f) {
                return false;
            }

            // --method / -X followed by a mutating HTTP verb.
            if (*token == "--method" || *token == "-X")
                && let Some(next) = tokens.get(i + 1)
            {
                let upper = next.to_uppercase();
                if ["POST", "PUT", "PATCH", "DELETE"].contains(&upper.as_str()) {
                    return false;
                }
            }

            // Combined forms (e.g. -XPOST, --method=POST, --input=file)
            if MUTATION_COMBINED.iter().any(|f| token.starts_with(f)) {
                return false;
            }
        }
        return true;
    }

    // Auto-approved command prefixes, grouped by category for readability.
    const SAFE_PREFIXES: &[&str] = &[
        // Git read-only
        "git log",
        "git status",
        "git diff",
        "git branch",
        "git show ", // trailing space requires an argument (ref)
        "git remote -v",
        "git rev-parse",
        // GitHub CLI read-only
        "gh repo view",
        "gh pr list",
        "gh issue list",
        "gh pr view",
        "gh issue view",
        "gh auth status",
        // Rust build/test/lint
        "cargo clippy",
        "cargo fmt --check",
        "cargo test",
        "cargo check",
        "cargo build",
        // Node.js build/test/lint
        "npm test",
        "npx jest ",
        "npx eslint ",
        "npx prettier --check",
        // Environment inspection
        "which ",
        "uname",
        "node -v",
        "node --version",
        "rustc --version",
        "cargo --version",
        "python --version",
        "go version",
        // Process inspection
        "lsof -i",
    ];

    if !SAFE_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return false;
    }

    // Block dangerous options that may launch external processes
    let dangerous_options = ["--web", "--browse"];
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for token in &tokens {
        if dangerous_options.iter().any(|opt| token == opt) {
            return false;
        }
    }
    true
}

/// Compute the effective permission class for a tool call.
///
/// Safe shell commands (as determined by [`is_safe_shell_command`]) are
/// promoted from `Confirm` to `Safe`, skipping the approval prompt.
pub fn effective_permission_class(input: &ToolInput, spec: &ToolSpec) -> PermissionClass {
    match input {
        ToolInput::ShellExec { command } if is_safe_shell_command(command) => PermissionClass::Safe,
        _ => spec.permission_class,
    }
}

// --- Search result parsing helpers ---

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

fn parse_duckduckgo_results(html: &str) -> Vec<SearchResult> {
    use regex::Regex;

    let mut results = Vec::new();

    // Filter out ad results
    let ad_re = Regex::new(r#"class="[^"]*result--ad[^"]*""#).ok();

    // Match result links: <a ... class="result__a" ... href="URL">TITLE</a>
    let link_re = Regex::new(r#"<a[^>]+class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).ok();

    // Match snippets: <a class="result__snippet" ...>SNIPPET</a>
    // Also try <td class="result__snippet"> for alternative format
    let snippet_re = Regex::new(r#"class="result__snippet"[^>]*>(.*?)</(?:a|td)>"#).ok();

    let (Some(link_re), Some(snippet_re)) = (link_re, snippet_re) else {
        return results;
    };

    // Split the HTML into result blocks (each starts with result__a)
    let link_captures: Vec<_> = link_re.captures_iter(html).collect();
    let snippet_captures: Vec<_> = snippet_re.captures_iter(html).collect();

    for (i, link_cap) in link_captures.iter().enumerate() {
        if results.len() >= 10 {
            break;
        }

        // Check if this result is within an ad block
        let match_start = link_cap.get(0).map(|m| m.start()).unwrap_or(0);
        if let Some(ref ad_re) = ad_re {
            // Look backwards for ad class marker
            let preceding = &html[..match_start];
            let last_result_start = preceding.rfind("class=\"result ");
            let last_ad = preceding.rfind("result--ad");
            if let (Some(result_pos), Some(ad_pos)) = (last_result_start, last_ad)
                && ad_pos > result_pos
            {
                continue;
            }
            // Also check forward context for ad markers
            let end = (match_start + 500).min(html.len());
            let context = &html[match_start..end];
            if ad_re.is_match(context) {
                continue;
            }
        }

        let raw_url = link_cap.get(1).map_or("", |m| m.as_str());
        let raw_title = link_cap.get(2).map_or("", |m| m.as_str());

        // Decode DuckDuckGo redirect URLs
        let url = decode_duckduckgo_url(raw_url);
        let title = strip_html_tags(raw_title);

        let snippet = snippet_captures
            .get(i)
            .and_then(|cap| cap.get(1))
            .map(|m| strip_html_tags(m.as_str()))
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    results
}

fn parse_serper_results(json_str: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return results;
    };

    if let Some(organic) = value.get("organic").and_then(|v| v.as_array()) {
        for item in organic.iter().take(10) {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = item
                .get("link")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let snippet = item
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }

    results
}

fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No search results found.".to_string();
    }
    let mut lines = Vec::new();
    for (i, result) in results.iter().enumerate() {
        lines.push(format!("[{}] {}", i + 1, result.title));
        lines.push(format!("    {}", result.url));
        if !result.snippet.is_empty() {
            lines.push(format!("    {}", result.snippet));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Percent-encode a query string for use in URLs.
fn percent_encode(input: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

/// Decode a DuckDuckGo redirect URL to extract the actual target URL.
fn decode_duckduckgo_url(url: &str) -> String {
    // DuckDuckGo wraps URLs in redirects like //duckduckgo.com/l/?uddg=ENCODED_URL&...
    if (url.contains("duckduckgo.com/l/") || url.contains("uddg="))
        && let Some(start) = url.find("uddg=")
    {
        let rest = &url[start + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        let encoded = &rest[..end];
        return percent_decode(encoded);
    }
    url.to_string()
}

/// Simple percent-decode.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if ch == '+' {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

/// Strip HTML tags from a string.
fn strip_html_tags(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

fn collect_search_matches(
    root: &Path,
    pattern: &str,
    matches: &mut Vec<String>,
) -> Result<(), ToolRuntimeError> {
    if root.is_file() {
        check_file_match(root, pattern, matches);
        return Ok(());
    }

    let entries = fs::read_dir(root).map_err(|err| {
        ToolRuntimeError::Io(format!("file.search failed for {}: {err}", root.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.search failed while reading {}: {err}",
                root.display()
            ))
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip common non-project directories
        if path.is_dir() {
            if matches!(
                name_str.as_ref(),
                ".git" | "target" | ".anvil" | "node_modules" | ".DS_Store"
            ) {
                continue;
            }
            collect_search_matches(&path, pattern, matches)?;
        } else {
            check_file_match(&path, pattern, matches);
        }
    }

    Ok(())
}

/// Check whether a single file matches `pattern` by path name or content.
fn check_file_match(path: &Path, pattern: &str, matches: &mut Vec<String>) {
    use std::io::BufRead;

    let path_str = path.display().to_string();
    if path_str.contains(pattern) {
        matches.push(path_str);
        return;
    }
    if !is_searchable_file(path) {
        return;
    }
    if let Ok(file) = fs::File::open(path) {
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.contains(pattern) {
                matches.push(path_str);
                break;
            }
        }
    }
}

/// Check if a file is likely to be text and worth searching.
fn is_searchable_file(path: &Path) -> bool {
    !matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "pdf"
                | "zip"
                | "gz"
                | "tar"
                | "wasm"
                | "ico"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
                | "o"
                | "a"
                | "class"
                | "pyc"
                | "pyo"
                | "lock"
        )
    )
}

fn render_directory_listing(path: &Path) -> Result<String, ToolRuntimeError> {
    let entries = fs::read_dir(path).map_err(|err| {
        ToolRuntimeError::Io(format!("file.read failed for {}: {err}", path.display()))
    })?;
    let mut lines = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.read failed while reading {}: {err}",
                path.display()
            ))
        })?;
        let entry_path = entry.path();
        let suffix = if entry_path.is_dir() { "/" } else { "" };
        lines.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
    }

    lines.sort();
    Ok(lines.join("\n"))
}
