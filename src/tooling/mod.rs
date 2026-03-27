//! Tool registry, validation, and local execution.
//!
//! Tools are declared as [`ToolSpec`] entries in a [`ToolRegistry`], validated
//! through a permission and plan-mode pipeline, and executed by
//! [`LocalToolExecutor`] within a sandboxed workspace root.

pub mod diff;
pub mod file_cache;
pub mod shell_policy;

pub use shell_policy::{ShellPolicy, classify_shell_policy, is_network_command};

use crate::config::{RuntimeConfig, WebSearchProvider};
use crate::contracts::ToolLogView;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Maximum image file size in bytes (20 MB).
const IMAGE_SIZE_LIMIT: u64 = 20 * 1024 * 1024;

/// Maximum number of context lines around a match (file.search).
const MAX_CONTEXT_LINES: u32 = 10;

/// Maximum number of matched files returned by file.search.
const MAX_SEARCH_RESULTS: usize = 100;

/// Files larger than this are blocked by the safe-write guard without reading
/// their full contents, to avoid memory pressure (10 MB).
const MAX_SAFE_WRITE_GUARD_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Detect the MIME type of an image file based on its extension.
///
/// Returns `None` for non-image extensions.
pub fn detect_image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

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
    FileEdit,
    FileEditAnchor,
    FileSearch,
    ShellExec,
    WebFetch,
    WebSearch,
    Mcp,
    AgentExplore,
    AgentPlan,
    GitStatus,
    GitDiff,
    GitLog,
}

impl ToolKind {
    /// Returns `true` if the tool may produce stderr output that would
    /// conflict with the spinner display (e.g. shell commands, git operations).
    pub fn produces_stderr(&self) -> bool {
        matches!(
            self,
            ToolKind::ShellExec | ToolKind::GitStatus | ToolKind::GitDiff | ToolKind::GitLog
        )
    }
}

/// Anchor-based edit parameters (Wave 1: indent normalization only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorEditParams {
    pub old_content: String,
    pub new_content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolInput {
    FileRead {
        path: String,
    },
    FileWrite {
        path: String,
        content: String,
    },
    FileEdit {
        path: String,
        old_string: String,
        new_string: String,
    },
    FileSearch {
        root: String,
        pattern: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        context_lines: u32,
    },
    ShellExec {
        command: String,
    },
    WebFetch {
        url: String,
    },
    WebSearch {
        query: String,
    },
    Mcp {
        server: String,
        tool: String,
        arguments: serde_json::Value,
    },
    AgentExplore {
        prompt: String,
        scope: Option<String>,
    },
    AgentPlan {
        prompt: String,
        scope: Option<String>,
    },
    GitStatus {},
    GitDiff {
        path: Option<String>,
        staged: Option<bool>,
        commit: Option<String>,
    },
    GitLog {
        count: Option<u32>,
        path: Option<String>,
    },
    FileEditAnchor {
        path: String,
        params: AnchorEditParams,
    },
}

impl ToolInput {
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::FileRead { .. } => ToolKind::FileRead,
            Self::FileWrite { .. } => ToolKind::FileWrite,
            Self::FileEdit { .. } => ToolKind::FileEdit,
            Self::FileSearch { .. } => ToolKind::FileSearch,
            Self::ShellExec { .. } => ToolKind::ShellExec,
            Self::WebFetch { .. } => ToolKind::WebFetch,
            Self::WebSearch { .. } => ToolKind::WebSearch,
            Self::Mcp { .. } => ToolKind::Mcp,
            Self::AgentExplore { .. } => ToolKind::AgentExplore,
            Self::AgentPlan { .. } => ToolKind::AgentPlan,
            Self::GitStatus { .. } => ToolKind::GitStatus,
            Self::GitDiff { .. } => ToolKind::GitDiff,
            Self::GitLog { .. } => ToolKind::GitLog,
            Self::FileEditAnchor { .. } => ToolKind::FileEditAnchor,
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
            "file.edit" => {
                let path = value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| "missing path in file.edit tool block".to_string())?;
                let old_string = value
                    .get("old_string")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| "missing old_string in file.edit tool block".to_string())?;
                let new_string = value
                    .get("new_string")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(ToolInput::FileEdit {
                    path,
                    old_string,
                    new_string,
                })
            }
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
                regex: value
                    .get("regex")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                context_lines: value
                    .get("context_lines")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as u32,
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
            "agent.explore" => Ok(ToolInput::AgentExplore {
                prompt: value
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing prompt in agent.explore tool block".to_string())?
                    .to_string(),
                scope: value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
            }),
            "agent.plan" => Ok(ToolInput::AgentPlan {
                prompt: value
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing prompt in agent.plan tool block".to_string())?
                    .to_string(),
                scope: value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
            }),
            "git.status" => Ok(ToolInput::GitStatus {}),
            "git.diff" => Ok(ToolInput::GitDiff {
                path: value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
                staged: value.get("staged").and_then(serde_json::Value::as_bool),
                commit: value
                    .get("commit")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
            }),
            "git.log" => Ok(ToolInput::GitLog {
                count: value
                    .get("count")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as u32),
                path: value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
            }),
            "file.edit_anchor" => {
                let path = value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| "missing path in file.edit_anchor tool block".to_string())?;
                let old_content = value
                    .get("old_content")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| {
                        "missing old_content in file.edit_anchor tool block".to_string()
                    })?;
                let new_content = value
                    .get("new_content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(ToolInput::FileEditAnchor {
                    path,
                    params: AnchorEditParams {
                        old_content,
                        new_content,
                    },
                })
            }
            other => {
                // mcp__<server>__<tool> pattern detection
                if let Some((server, tool)) = parse_mcp_tool_name(other) {
                    Ok(ToolInput::Mcp {
                        server,
                        tool,
                        arguments: value.clone(),
                    })
                } else {
                    Err(format!("unsupported tool in ANVIL_TOOL block: {other}"))
                }
            }
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
            "file.edit" => {
                let path = extract_simple(block, "path")?;
                let old_string = extract_simple(block, "old_string")?;
                let new_string = extract_trailing(block, "new_string").unwrap_or_default();
                Some(ToolInput::FileEdit {
                    path,
                    old_string,
                    new_string,
                })
            }
            "file.read" => Some(ToolInput::FileRead {
                path: extract_simple(block, "path")?,
            }),
            "file.search" => Some(ToolInput::FileSearch {
                root: extract_simple(block, "root").or_else(|| extract_simple(block, "path"))?,
                pattern: extract_simple(block, "pattern")
                    .or_else(|| extract_simple(block, "content"))
                    .or_else(|| extract_simple(block, "query"))?,
                regex: extract_simple(block, "regex")
                    .map(|v| v == "true")
                    .unwrap_or(false),
                context_lines: extract_simple(block, "context_lines")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0),
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
            "agent.explore" => Some(ToolInput::AgentExplore {
                prompt: extract_simple(block, "prompt")?,
                scope: extract_simple(block, "scope"),
            }),
            "agent.plan" => Some(ToolInput::AgentPlan {
                prompt: extract_simple(block, "prompt")?,
                scope: extract_simple(block, "scope"),
            }),
            "git.status" => Some(ToolInput::GitStatus {}),
            "git.diff" => Some(ToolInput::GitDiff {
                path: extract_simple(block, "path"),
                staged: extract_simple(block, "staged").and_then(|s| s.parse::<bool>().ok()),
                commit: extract_simple(block, "commit"),
            }),
            "git.log" => Some(ToolInput::GitLog {
                count: extract_simple(block, "count").and_then(|s| s.parse::<u32>().ok()),
                path: extract_simple(block, "path"),
            }),
            "file.edit_anchor" => {
                let path = extract_simple(block, "path")?;
                let old_content = extract_simple(block, "old_content")?;
                let new_content = extract_trailing(block, "new_content").unwrap_or_default();
                Some(ToolInput::FileEditAnchor {
                    path,
                    params: AnchorEditParams {
                        old_content,
                        new_content,
                    },
                })
            }
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
    Image {
        source_path: String,
        mime_type: String,
    },
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
    /// Compact diff summary for file-mutating tools (file.write/file.edit/file.edit_anchor).
    /// `None` for non-mutating tools, MCP tools, and subagent results.
    pub diff_summary: Option<String>,
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
            elapsed_ms: Some(self.elapsed_ms.min(u64::MAX as u128) as u64),
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

    pub fn register_file_edit(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "file.edit".to_string(),
            kind: ToolKind::FileEdit,
            execution_class: ExecutionClass::Mutating,
            permission_class: PermissionClass::Confirm,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::AllowedWithScope,
            rollback_policy: RollbackPolicy::CheckpointBeforeWrite,
        });
    }

    pub fn register_file_edit_anchor(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "file.edit_anchor".to_string(),
            kind: ToolKind::FileEditAnchor,
            execution_class: ExecutionClass::Mutating,
            permission_class: PermissionClass::Confirm,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::Allowed,
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

    pub fn register_agent_explore(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "agent.explore".to_string(),
            kind: ToolKind::AgentExplore,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_agent_plan(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "agent.plan".to_string(),
            kind: ToolKind::AgentPlan,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_git_status(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "git.status".to_string(),
            kind: ToolKind::GitStatus,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_git_diff(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "git.diff".to_string(),
            kind: ToolKind::GitDiff,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_git_log(&mut self) {
        self.register(ToolSpec {
            version: 1,
            name: "git.log".to_string(),
            kind: ToolKind::GitLog,
            execution_class: ExecutionClass::ReadOnly,
            permission_class: PermissionClass::Safe,
            execution_mode: ExecutionMode::ParallelSafe,
            plan_mode: PlanModePolicy::Allowed,
            rollback_policy: RollbackPolicy::None,
        });
    }

    /// Register the subset of tools available to the Explore sub-agent.
    pub fn register_explore_tools(&mut self) {
        self.register_file_read();
        self.register_file_search();
        self.register_git_status();
        self.register_git_diff();
        self.register_git_log();
    }

    /// Register the subset of tools available to the Plan sub-agent.
    pub fn register_plan_tools(&mut self) {
        self.register_file_read();
        self.register_file_search();
        self.register_web_fetch();
        self.register_git_status();
    }

    pub fn register_standard_tools(&mut self) {
        self.register_file_read();
        self.register_file_write();
        self.register_file_edit();
        self.register_file_edit_anchor();
        self.register_file_search();
        self.register_shell_exec();
        self.register_web_fetch();
        self.register_web_search();
        self.register_git_status();
        self.register_git_diff();
        self.register_git_log();
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
    shutdown_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    http_client: reqwest::blocking::Client,
    file_cache: Option<std::sync::Arc<std::sync::Mutex<file_cache::FileReadCache>>>,
    safe_write_max_lines: usize,
}

/// Build the shared HTTP client used by tooling (web.fetch / web.search).
fn build_tooling_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client for tooling")
}

#[derive(Debug)]
pub enum ToolRuntimeError {
    InvalidPath(String),
    Io(String),
    CaptchaBlocked {
        query: String,
    },
    EditNotFound {
        message: String,
        context_snippet: Option<String>,
    },
    LargeFileBlocked {
        path: String,
        line_count: usize,
        threshold: usize,
    },
}

impl ToolRuntimeError {
    /// Create an EditNotFound error without context snippet.
    pub fn edit_not_found(message: impl Into<String>) -> Self {
        Self::EditNotFound {
            message: message.into(),
            context_snippet: None,
        }
    }

    /// Create an EditNotFound error with a file context snippet.
    pub fn edit_not_found_with_context(message: impl Into<String>, context: String) -> Self {
        Self::EditNotFound {
            message: message.into(),
            context_snippet: Some(context),
        }
    }

    pub fn is_edit_not_found(&self) -> bool {
        matches!(self, Self::EditNotFound { .. })
    }
}

impl Display for ToolRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(f, "invalid tool path: {path}"),
            Self::Io(message) => write!(f, "{message}"),
            Self::CaptchaBlocked { query } => write!(
                f,
                "DuckDuckGo search for '{query}' blocked by CAPTCHA. \
                 Consider setting SERPER_API_KEY for automatic fallback, \
                 or use web.fetch to access specific URLs directly."
            ),
            Self::EditNotFound { message, .. } => write!(f, "{message}"),
            Self::LargeFileBlocked {
                path,
                line_count,
                threshold,
            } => write!(
                f,
                "File '{}' has {} lines (threshold: {}).\n\
                 Large existing files cannot be overwritten with file.write.\n\
                 Use file.edit or file.edit_anchor to make targeted changes instead.",
                sanitize_path_for_display(path),
                line_count,
                threshold
            ),
        }
    }
}

/// Sanitize a path string for display by removing control characters.
fn sanitize_path_for_display(path: &str) -> String {
    path.chars().filter(|c| !c.is_control()).collect()
}

impl std::error::Error for ToolRuntimeError {}

/// Sensitive file patterns that should never have their content included
/// in error responses to prevent accidental leakage of secrets.
const SENSITIVE_FILE_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "credentials.json",
    "secrets.*",
    "*.secret",
    "*.secrets",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "token.json",
    "service-account*.json",
];

/// Check whether a file path matches the sensitive file blocklist.
///
/// Matching is performed against the file name (basename) only, using
/// simple string operations: exact match, prefix (`starts_with`),
/// suffix (`ends_with`), and prefix+suffix for patterns like `service-account*.json`.
pub fn is_sensitive_file(path: &str) -> bool {
    let file_name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if file_name.is_empty() {
        return false;
    }
    for pattern in SENSITIVE_FILE_PATTERNS {
        if let Some(suffix) = pattern.strip_prefix('*') {
            if file_name.ends_with(suffix) {
                return true;
            }
        } else if let Some(prefix) = pattern.strip_suffix('*') {
            if file_name.starts_with(prefix) {
                return true;
            }
        } else if pattern.contains('*') {
            let parts: Vec<&str> = pattern.splitn(2, '*').collect();
            if parts.len() == 2
                && file_name.starts_with(parts[0])
                && file_name.ends_with(parts[1])
                && file_name.len() >= parts[0].len() + parts[1].len()
            {
                return true;
            }
        } else if file_name == *pattern {
            return true;
        }
    }
    false
}

/// Extract context lines around the best matching location for `old_string`
/// in `content`. Searches for the first line of `old_string` (trimmed) and
/// returns surrounding lines with line numbers.
pub fn extract_edit_context(
    content: &str,
    old_string: &str,
    context_lines: usize,
) -> Option<String> {
    let first_line = old_string.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let match_idx = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.trim().contains(first_line))?
        .0;

    let start = match_idx.saturating_sub(context_lines);
    let end = (match_idx + context_lines + 1).min(lines.len());

    let context: String = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:4} | {}", start + i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    Some(context)
}

impl LocalToolExecutor {
    pub fn new(
        root: impl Into<PathBuf>,
        config: &RuntimeConfig,
        file_cache: Option<std::sync::Arc<std::sync::Mutex<file_cache::FileReadCache>>>,
    ) -> Self {
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
            shutdown_flag: None,
            http_client: build_tooling_http_client(),
            file_cache,
            safe_write_max_lines: config.safe_write_max_lines,
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
            shutdown_flag: None,
            http_client: build_tooling_http_client(),
            file_cache: None,
            safe_write_max_lines: 0,
        }
    }

    /// Set the safe_write_max_lines threshold (for tests).
    pub fn set_safe_write_max_lines(&mut self, value: usize) {
        self.safe_write_max_lines = value;
    }

    /// Inject a file read cache (for tests).
    pub fn set_file_cache(
        &mut self,
        cache: std::sync::Arc<std::sync::Mutex<file_cache::FileReadCache>>,
    ) {
        self.file_cache = Some(cache);
    }

    /// Create an executor with a Serper API key set (for testing fallback).
    pub fn new_test_with_serper_key(root: impl Into<PathBuf>, key: String) -> Self {
        let mut executor = Self::new_without_rate_limit(root);
        executor.serper_api_key = Some(key);
        executor
    }

    /// Set the shutdown flag for graceful shutdown support.
    pub fn with_shutdown_flag(
        mut self,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.shutdown_flag = Some(flag);
        self
    }

    /// Check whether a shutdown has been requested via the shared flag.
    fn is_shutdown(&self) -> bool {
        self.shutdown_flag
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Invalidate the file cache entry for the given path (best-effort).
    fn invalidate_cache(&self, resolved: &Path) {
        if let Some(ref cache_arc) = self.file_cache
            && let Ok(mut cache) = cache_arc.lock()
        {
            cache.invalidate(resolved);
        }
    }

    pub fn execute(
        &mut self,
        request: ToolExecutionRequest,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let tool_name = &request.spec.name;
        tracing::info!(tool = %tool_name, "executing tool");
        let started = Instant::now();
        let result = match request.input {
            ToolInput::FileRead { ref path } => self.execute_file_read(&request, path, started),
            ToolInput::FileWrite {
                ref path,
                ref content,
            } => self.execute_file_write(&request, path, content, started),
            ToolInput::FileEdit {
                ref path,
                ref old_string,
                ref new_string,
            } => self
                .execute_file_edit_with_fallback(&request, path, old_string, new_string, started),
            ToolInput::FileSearch {
                ref root,
                ref pattern,
                regex,
                context_lines,
            } => self.execute_file_search(&request, root, pattern, regex, context_lines, started),
            ToolInput::WebFetch { ref url } => self.execute_web_fetch(&request, url, started),
            ToolInput::ShellExec { ref command } => {
                self.execute_shell_exec(&request, command, started)
            }
            ToolInput::WebSearch { ref query } => self.execute_web_search(&request, query, started),
            ToolInput::GitStatus {} => self.execute_git_status(&request, started),
            ToolInput::GitDiff {
                ref path,
                ref staged,
                ref commit,
            } => self.execute_git_diff(&request, path, staged, commit, started),
            ToolInput::GitLog {
                ref count,
                ref path,
            } => self.execute_git_log(&request, count, path, started),
            ToolInput::FileEditAnchor {
                ref path,
                ref params,
            } => self.execute_file_edit_anchor(&request, path, params, started),
            ToolInput::Mcp { .. } => unreachable!("MCP tools are dispatched in agentic.rs"),
            ToolInput::AgentExplore { .. } | ToolInput::AgentPlan { .. } => {
                unreachable!("agent tools are dispatched in agentic.rs")
            }
        };
        tracing::info!(
            tool = %tool_name,
            elapsed_ms = %started.elapsed().as_millis(),
            "tool execution completed"
        );
        result
    }

    fn execute_file_read(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        if resolved.is_dir() {
            let content = render_directory_listing(&resolved)?;
            return Ok(build_completed_result(
                request,
                path.to_string(),
                ToolExecutionPayload::Text(content),
                vec![resolved.display().to_string()],
                started,
            ));
        }
        // Check if the file is an image based on extension
        if let Some(mime_type) = detect_image_mime(&resolved) {
            return self.execute_image_read(request, path, &resolved, mime_type, started);
        }

        // Cache check (high-level API: canonicalize + mtime + sandbox validation internal)
        // DR-005: canonical_for_read is deferred to cache-miss path below to avoid
        // redundant canonicalize on cache hits. try_get performs its own sandbox check.
        if let Some(ref cache_arc) = self.file_cache
            && let Ok(mut cache) = cache_arc.lock()
            && let Some(hit) = cache.try_get(&resolved)
        {
            tracing::debug!(path = %path, hit_count = hit.hit_count, "file.read cache hit");
            let header = format!(
                "[cached: read #{} — content unchanged, {} bytes]\n",
                hit.hit_count,
                hit.content.len()
            );
            let payload = format!("{}{}", header, hit.content);
            return Ok(build_completed_result(
                request,
                path.to_string(),
                ToolExecutionPayload::Text(payload),
                vec![resolved.display().to_string()],
                started,
            ));
        }
        // Mutex poison → fall through to normal read (best-effort)

        // TOCTOU defense (DR4-002): resolve canonical path ONCE for disk read,
        // preventing symlink swap between check and read. Deferred here per DR-005.
        let canonical_for_read = self
            .file_cache
            .as_ref()
            .and_then(|c| c.lock().ok())
            .and_then(|c| c.validate_canonical_path(&resolved));

        // Read from canonical path when available (TOCTOU defense), fallback to resolved
        let read_path = canonical_for_read.as_deref().unwrap_or(&resolved);
        let content = fs::read_to_string(read_path).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.read failed for {}: {err}",
                resolved.display()
            ))
        })?;

        // Cache record (high-level API: canonicalize + LRU eviction internal)
        if let Some(ref cache_arc) = self.file_cache
            && let Ok(mut cache) = cache_arc.lock()
        {
            cache.record(&resolved, content.clone());
        }

        Ok(build_completed_result(
            request,
            path.to_string(),
            ToolExecutionPayload::Text(content),
            vec![resolved.display().to_string()],
            started,
        ))
    }

    fn execute_image_read(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        resolved: &Path,
        mime_type: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let metadata = fs::metadata(resolved).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.read failed for {}: {err}",
                resolved.display()
            ))
        })?;
        if metadata.len() > IMAGE_SIZE_LIMIT {
            return Ok(build_completed_result(
                request,
                path.to_string(),
                ToolExecutionPayload::Text(format!(
                    "ファイルサイズが上限(20MB)を超えています: {} bytes",
                    metadata.len()
                )),
                vec![resolved.display().to_string()],
                started,
            ));
        }
        Ok(build_completed_result(
            request,
            path.to_string(),
            ToolExecutionPayload::Image {
                source_path: resolved.display().to_string(),
                mime_type: mime_type.to_string(),
            },
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

        // Large file write guard (safe_write_max_lines > 0 = enabled)
        if self.safe_write_max_lines > 0 {
            match std::fs::metadata(&resolved) {
                Ok(meta) => {
                    // Extremely large files: block without full read to avoid memory pressure
                    if meta.len() > MAX_SAFE_WRITE_GUARD_READ_BYTES {
                        return Err(ToolRuntimeError::LargeFileBlocked {
                            path: path.to_string(),
                            line_count: 0,
                            threshold: self.safe_write_max_lines,
                        });
                    }
                    if let Ok(existing) = std::fs::read(&resolved) {
                        let newline_count = existing.iter().filter(|&&b| b == b'\n').count();
                        let line_count = if existing.is_empty() {
                            0
                        } else {
                            newline_count + usize::from(!existing.ends_with(b"\n"))
                        };
                        if line_count > self.safe_write_max_lines {
                            return Err(ToolRuntimeError::LargeFileBlocked {
                                path: path.to_string(),
                                line_count,
                                threshold: self.safe_write_max_lines,
                            });
                        }
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // New file creation — not subject to the guard
                }
                Err(_) => {
                    // Other metadata errors — proceed to let fs::write handle it
                }
            }
        }

        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "file.write failed for {} (parent creation failed for {}): {err}",
                    resolved.display(),
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
        self.invalidate_cache(&resolved);
        let diff = format!("wrote {} bytes to {}", content.len(), path);
        Ok(build_completed_result_with_diff(
            request,
            path.to_string(),
            ToolExecutionPayload::None,
            vec![resolved.display().to_string()],
            started,
            Some(diff),
        ))
    }

    /// file.edit成功時のdiffフィードバック用Payloadを生成
    fn build_edit_diff_payload(old_string: &str, new_string: &str) -> ToolExecutionPayload {
        diff::generate_file_edit_diff(old_string, new_string)
            .map(ToolExecutionPayload::Text)
            .unwrap_or(ToolExecutionPayload::None)
    }

    fn execute_file_edit(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        old_string: &str,
        new_string: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        let content = fs::read_to_string(&resolved).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.edit failed to read {}: {err}",
                resolved.display()
            ))
        })?;
        if old_string == new_string {
            return Ok(build_completed_result(
                request,
                format!("{path} (no changes)"),
                ToolExecutionPayload::None,
                vec![],
                started,
            ));
        }
        let count = content.matches(old_string).count();
        if count == 0 {
            return Err(ToolRuntimeError::edit_not_found(format!(
                "file.edit: old_string not found in {path}. \
                 Ensure the string exactly matches the file content, \
                 including whitespace and indentation."
            )));
        }
        if count > 1 {
            return Err(ToolRuntimeError::edit_not_found(format!(
                "file.edit: old_string found {count} times in {path}. \
                 Include more surrounding context to make the match unique."
            )));
        }
        let new_content = content.replacen(old_string, new_string, 1);
        fs::write(&resolved, &new_content).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.edit failed to write {}: {err}",
                resolved.display()
            ))
        })?;
        self.invalidate_cache(&resolved);
        let diff = diff::generate_file_edit_diff(old_string, new_string);
        Ok(build_completed_result_with_diff(
            request,
            path.to_string(),
            Self::build_edit_diff_payload(old_string, new_string),
            vec![resolved.display().to_string()],
            started,
            diff,
        ))
    }

    fn execute_file_edit_anchor(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        params: &AnchorEditParams,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        let content = fs::read_to_string(&resolved).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.edit_anchor failed to read {}: {err}",
                resolved.display()
            ))
        })?;

        if params.old_content == params.new_content {
            return Ok(build_completed_result(
                request,
                format!("{path} (no changes)"),
                ToolExecutionPayload::None,
                vec![],
                started,
            ));
        }

        let normalized_matches = find_indent_normalized_matches(&content, &params.old_content);

        match normalized_matches.len() {
            1 => {
                let new_content =
                    apply_normalized_edit(&content, &normalized_matches[0], &params.new_content);
                fs::write(&resolved, &new_content).map_err(|err| {
                    ToolRuntimeError::Io(format!(
                        "file.edit_anchor failed to write {}: {err}",
                        resolved.display()
                    ))
                })?;
                self.invalidate_cache(&resolved);
                let diff = diff::generate_file_edit_diff(&params.old_content, &params.new_content);
                Ok(build_completed_result_with_diff(
                    request,
                    path.to_string(),
                    Self::build_edit_diff_payload(&params.old_content, &params.new_content),
                    vec![resolved.display().to_string()],
                    started,
                    diff,
                ))
            }
            0 => Err(ToolRuntimeError::edit_not_found(format!(
                "anchor: normalized old_content not found in {path}"
            ))),
            n => Err(ToolRuntimeError::edit_not_found(format!(
                "anchor: old_content matched {n} locations in {path}, need unique match"
            ))),
        }
    }

    /// Try file.edit with 3-level fallback: strict → trailing-ws → anchor.
    /// On final failure, includes file context snippet in the error for LLM recovery.
    fn execute_file_edit_with_fallback(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        old_string: &str,
        new_string: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        // Level 1: strict replace
        let original_err =
            match self.execute_file_edit(request, path, old_string, new_string, started) {
                Ok(result) => return Ok(result),
                Err(err) if !err.is_edit_not_found() => return Err(err),
                Err(err) => err,
            };

        // Level 2: trailing whitespace normalized
        if let Ok(mut result) =
            self.execute_file_edit_trailing_ws(request, path, old_string, new_string, started)
        {
            result.summary = format!("{} (trailing-ws fallback)", result.summary);
            return Ok(result);
        }

        // Level 3: anchor-based (indent-normalized)
        let params = AnchorEditParams {
            old_content: old_string.to_string(),
            new_content: new_string.to_string(),
        };
        match self.execute_file_edit_anchor(request, path, &params, started) {
            Ok(mut result) => {
                result.summary = format!("{} (anchor fallback)", result.summary);
                Ok(result)
            }
            // All levels failed — enrich error with file context
            Err(_) => Err(self.build_edit_not_found_with_context(path, old_string, &original_err)),
        }
    }

    /// Level 2 fallback: match after stripping trailing whitespace from each line.
    fn execute_file_edit_trailing_ws(
        &self,
        request: &ToolExecutionRequest,
        path: &str,
        old_string: &str,
        new_string: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved = self.resolve_path(path)?;
        let content = fs::read_to_string(&resolved).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.edit failed to read {}: {err}",
                resolved.display()
            ))
        })?;

        let normalize_lines = |s: &str| -> Vec<String> {
            s.lines().map(|line| line.trim_end().to_string()).collect()
        };

        let content_lines = normalize_lines(&content);
        let old_lines = normalize_lines(old_string);

        if old_lines.is_empty() {
            return Err(ToolRuntimeError::edit_not_found(
                "file.edit: empty old_string",
            ));
        }

        // Find matching line range using trailing-ws-normalized comparison
        let mut match_positions = Vec::new();
        for i in 0..content_lines.len() {
            if i + old_lines.len() > content_lines.len() {
                break;
            }
            if content_lines[i..i + old_lines.len()] == old_lines[..] {
                match_positions.push(i);
            }
        }

        if match_positions.len() != 1 {
            return Err(ToolRuntimeError::edit_not_found(format!(
                "file.edit: trailing-ws-normalized old_string {} in {path}",
                if match_positions.is_empty() {
                    "not found".to_string()
                } else {
                    format!("found {} times", match_positions.len())
                }
            )));
        }

        let match_start = match_positions[0];
        let match_end = match_start + old_lines.len();

        // Replace the matched line range with new_string
        let original_lines: Vec<&str> = content.lines().collect();
        let mut result_parts: Vec<&str> = Vec::new();
        for (i, line) in original_lines.iter().enumerate() {
            if i < match_start || i >= match_end {
                result_parts.push(line);
            } else if i == match_start {
                // Insert new_string at match position
                result_parts.push(new_string);
            }
        }

        // Handle trailing newline from original content
        let new_content = if content.ends_with('\n') {
            format!("{}\n", result_parts.join("\n"))
        } else {
            result_parts.join("\n")
        };

        fs::write(&resolved, &new_content).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "file.edit failed to write {}: {err}",
                resolved.display()
            ))
        })?;
        self.invalidate_cache(&resolved);
        let diff = diff::generate_file_edit_diff(old_string, new_string);

        Ok(build_completed_result_with_diff(
            request,
            path.to_string(),
            Self::build_edit_diff_payload(old_string, new_string),
            vec![resolved.display().to_string()],
            started,
            diff,
        ))
    }

    /// Build an EditNotFound error enriched with file context snippet.
    /// Skips context for sensitive files (e.g., .env, credentials).
    fn build_edit_not_found_with_context(
        &self,
        path: &str,
        old_string: &str,
        original_err: &ToolRuntimeError,
    ) -> ToolRuntimeError {
        let message = original_err.to_string();

        // Don't include context for sensitive files
        if is_sensitive_file(path) {
            return ToolRuntimeError::edit_not_found(message);
        }

        // Try to read file content for context extraction
        let context_snippet = self
            .resolve_path(path)
            .ok()
            .and_then(|resolved| fs::read_to_string(resolved).ok())
            .and_then(|content| extract_edit_context(&content, old_string, 5));

        match context_snippet {
            Some(ctx) => ToolRuntimeError::edit_not_found_with_context(message, ctx),
            None => ToolRuntimeError::edit_not_found(message),
        }
    }

    fn execute_file_search(
        &self,
        request: &ToolExecutionRequest,
        root: &str,
        pattern: &str,
        regex: bool,
        context_lines: u32,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let resolved_root = self.resolve_path(root)?;

        let search_pattern = if regex {
            let re = regex::RegexBuilder::new(pattern)
                .size_limit(1 << 20)
                .build()
                .map_err(|err| ToolRuntimeError::Io(format!("invalid regex pattern: {err}")))?;
            SearchPattern::Regex(re)
        } else {
            SearchPattern::Literal(pattern.to_string())
        };

        let mut file_matches: Vec<FileMatchResult> = Vec::new();
        let mut total_count: usize = 0;
        collect_search_matches_v2(
            &resolved_root,
            &search_pattern,
            context_lines,
            &mut file_matches,
            &mut total_count,
        )?;

        let (payload, artifacts) =
            format_file_search_results(&file_matches, context_lines, total_count);

        Ok(build_completed_result(
            request,
            format!("{root} :: {pattern}"),
            payload,
            artifacts,
            started,
        ))
    }

    fn execute_web_fetch(
        &self,
        request: &ToolExecutionRequest,
        url: &str,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolRuntimeError::Io(
                "Only http/https URLs allowed".to_string(),
            ));
        }
        let response = self
            .http_client
            .get(url)
            .send()
            .map_err(|err| ToolRuntimeError::Io(format!("web.fetch failed: {err}")))?;
        if !response.status().is_success() {
            return Err(ToolRuntimeError::Io(format!(
                "web.fetch failed for {url}: HTTP {}",
                response.status()
            )));
        }
        if let Some(len) = response.content_length()
            && len > 1_048_576
        {
            return Err(ToolRuntimeError::Io(
                "Response too large (>1MB)".to_string(),
            ));
        }
        let max_size: u64 = 1_048_576;
        let mut body_bytes = Vec::new();
        use std::io::Read;
        response
            .take(max_size)
            .read_to_end(&mut body_bytes)
            .map_err(|err| ToolRuntimeError::Io(format!("web.fetch read error: {err}")))?;
        let body = String::from_utf8_lossy(&body_bytes).to_string();
        Ok(build_completed_result(
            request,
            url.to_string(),
            ToolExecutionPayload::Text(body),
            Vec::new(),
            started,
        ))
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

        // Poll child process with shutdown flag check
        let exit_status = loop {
            if self.is_shutdown() {
                if let Err(e) = child.kill() {
                    tracing::warn!("failed to kill child process: {e}");
                }
                let stdout_buf = stdout_thread.join().unwrap_or_default();
                let stderr_buf = stderr_thread.join().unwrap_or_default();
                let _ = child.wait();
                let combined = combine_process_output(stdout_buf, stderr_buf);
                return Ok(ToolExecutionResult {
                    tool_call_id: request.tool_call_id.clone(),
                    tool_name: request.spec.name.clone(),
                    status: ToolExecutionStatus::Interrupted,
                    summary: format!("shell.exec interrupted: {command}"),
                    payload: ToolExecutionPayload::Text(combined),
                    artifacts: Vec::new(),
                    elapsed_ms: started.elapsed().as_millis(),
                    diff_summary: None,
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break None,
            }
        };
        let stdout_buf = stdout_thread.join().unwrap_or_default();
        let stderr_buf = stderr_thread.join().unwrap_or_default();

        let combined = combine_process_output(stdout_buf, stderr_buf);

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
            diff_summary: None,
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
                match self.execute_web_search_duckduckgo(request, query, started) {
                    Err(ToolRuntimeError::CaptchaBlocked { .. })
                        if self.serper_api_key.is_some() =>
                    {
                        self.execute_web_search_serper(request, query, started)
                    }
                    other => other,
                }
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

        // Resolve locale parameters for CJK languages
        let locale_params = detect_system_locale().and_then(|lang| resolve_locale_params(&lang));

        let url = if let Some(ref lp) = locale_params {
            format!(
                "https://html.duckduckgo.com/html/?q={encoded_query}&kl={}",
                lp.kl
            )
        } else {
            format!("https://html.duckduckgo.com/html/?q={encoded_query}")
        };

        let mut request_builder = self
            .http_client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            );

        // Add Accept-Language header for CJK locales
        if let Some(ref lp) = locale_params {
            request_builder = request_builder.header("Accept-Language", &lp.accept_language);
        }

        let response = request_builder
            .send()
            .map_err(|err| ToolRuntimeError::Io(format!("web.search failed: {err}")))?;

        if !response.status().is_success() {
            return Err(ToolRuntimeError::CaptchaBlocked {
                query: query.to_string(),
            });
        }

        let body = response
            .text()
            .map_err(|err| ToolRuntimeError::Io(format!("web.search read error: {err}")))?;

        // Parse results using regex
        let results = parse_duckduckgo_results(&body);

        // CAPTCHA detection using pure function
        if is_captcha_response(&body, results.len()) {
            return Err(ToolRuntimeError::CaptchaBlocked {
                query: query.to_string(),
            });
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

        let response = self
            .http_client
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", api_key)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .map_err(|err| {
                ToolRuntimeError::Io(format!(
                    "Failed to reach SerperAPI. Check your network connection. {err}"
                ))
            })?;

        let status = response.status().as_u16();
        match status {
            200 => {} // success
            401 | 403 => {
                return Err(ToolRuntimeError::Io(
                    "Invalid or expired SerperAPI key.".to_string(),
                ));
            }
            429 => {
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

        let response_body = response
            .text()
            .map_err(|err| ToolRuntimeError::Io(format!("SerperAPI read error: {err}")))?;
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

    /// Shared helper: execute a git command and return a [`ToolExecutionResult`].
    fn run_git_command(
        &self,
        args: &[&str],
        request: &ToolExecutionRequest,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let mut cmd = std::process::Command::new("git");
        for arg in args {
            cmd.arg(arg);
        }
        cmd.current_dir(&self.root);
        let output = cmd
            .output()
            .map_err(|e| ToolRuntimeError::Io(format!("failed to execute git: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let error_msg = if exit_code == 128 {
                format!("not a git repository: {stderr}")
            } else {
                format!("git command failed (exit {exit_code}): {stderr}")
            };
            return Ok(ToolExecutionResult {
                tool_call_id: request.tool_call_id.clone(),
                tool_name: request.spec.name.clone(),
                status: ToolExecutionStatus::Failed,
                summary: error_msg.clone(),
                payload: ToolExecutionPayload::Text(error_msg),
                artifacts: Vec::new(),
                elapsed_ms: started.elapsed().as_millis(),
                diff_summary: None,
            });
        }

        let combined = combine_process_output(stdout, stderr);
        Ok(build_completed_result(
            request,
            format!("{} completed", request.spec.name),
            ToolExecutionPayload::Text(combined),
            Vec::new(),
            started,
        ))
    }

    fn execute_git_status(
        &self,
        request: &ToolExecutionRequest,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        self.run_git_command(&["status", "--porcelain"], request, started)
    }

    fn execute_git_diff(
        &self,
        request: &ToolExecutionRequest,
        path: &Option<String>,
        staged: &Option<bool>,
        commit: &Option<String>,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let mut args: Vec<String> = vec!["diff".to_string()];
        if staged.unwrap_or(false) {
            args.push("--staged".to_string());
        } else if let Some(c) = commit {
            args.push(c.clone());
        }
        if let Some(p) = path {
            args.push("--".to_string());
            args.push(p.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.run_git_command(&arg_refs, request, started)
    }

    fn execute_git_log(
        &self,
        request: &ToolExecutionRequest,
        count: &Option<u32>,
        path: &Option<String>,
        started: Instant,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let count_val = count.unwrap_or(10);
        let count_str = count_val.to_string();
        let mut args: Vec<&str> = vec!["log", "--oneline", "-n", &count_str];
        let path_owned;
        if let Some(p) = path {
            path_owned = p.clone();
            args.push("--");
            args.push(&path_owned);
        }
        self.run_git_command(&args, request, started)
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
        resolve_sandbox_path(&self.root, raw)
    }
}

/// A matched region in file content for indent-normalized editing.
#[derive(Debug, Clone)]
struct NormalizedMatch {
    /// Byte offset of the matched region start in the original file content.
    start: usize,
    /// Byte offset of the matched region end in the original file content.
    end: usize,
    /// The leading whitespace prefix of the first line of the matched region.
    indent_prefix: String,
}

/// Compute the byte offset of the start of `line_idx` in `content`.
fn byte_offset_of_line(content: &str, lines: &[&str], line_idx: usize) -> usize {
    let mut pos = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == line_idx {
            return pos;
        }
        pos += line.len();
        pos += line_terminator_len(&content[pos..]);
    }
    pos
}

/// Compute the byte offset just after the end of `line_idx` in `content`
/// (i.e., after the line's text but before its terminator).
fn byte_offset_after_line(content: &str, lines: &[&str], line_idx: usize) -> usize {
    let mut pos = 0;
    for (idx, line) in lines.iter().enumerate() {
        pos += line.len();
        if idx == line_idx {
            return pos;
        }
        pos += line_terminator_len(&content[pos..]);
    }
    pos
}

/// Return the byte length of the line terminator at the start of `s` (0, 1, or 2).
fn line_terminator_len(s: &str) -> usize {
    if s.starts_with("\r\n") {
        2
    } else if s.starts_with('\n') {
        1
    } else {
        0
    }
}

/// Find regions in `content` that match `pattern` after stripping leading whitespace
/// from each line of both pattern and content.
fn find_indent_normalized_matches(content: &str, pattern: &str) -> Vec<NormalizedMatch> {
    let pattern_lines: Vec<&str> = pattern.lines().collect();
    if pattern_lines.is_empty() {
        return Vec::new();
    }

    let normalized_pattern: Vec<String> = pattern_lines
        .iter()
        .map(|line| line.trim_start().to_string())
        .collect();

    // Skip all-empty patterns
    if normalized_pattern.iter().all(|l| l.is_empty()) {
        return Vec::new();
    }

    let content_lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();

    'outer: for i in 0..content_lines.len() {
        if i + pattern_lines.len() > content_lines.len() {
            break;
        }

        // Check if lines match after trimming leading whitespace
        for (j, norm_pat) in normalized_pattern.iter().enumerate() {
            let content_line_trimmed = content_lines[i + j].trim_start();
            if content_line_trimmed != norm_pat.as_str() {
                continue 'outer;
            }
        }

        // Compute byte offsets using actual content positions
        // (handles CRLF and missing trailing newline).
        let start_byte = byte_offset_of_line(content, &content_lines, i);
        let end_line = i + pattern_lines.len() - 1;
        let end_byte = byte_offset_after_line(content, &content_lines, end_line);

        // Extract indent prefix from first matched line
        let first_line = content_lines[i];
        let trimmed_len = first_line.trim_start().len();
        let indent_prefix = first_line[..first_line.len() - trimmed_len].to_string();

        matches.push(NormalizedMatch {
            start: start_byte,
            end: end_byte,
            indent_prefix,
        });
    }

    matches
}

/// Apply a normalized edit by replacing the matched region with new content,
/// preserving the original indentation.
fn apply_normalized_edit(content: &str, matched: &NormalizedMatch, new_content: &str) -> String {
    // Re-indent new_content to match the indentation of the matched region.
    // Wave 1: all non-empty lines use the same indent prefix as the first matched line.
    let new_lines: Vec<&str> = new_content.lines().collect();
    let reindented: Vec<String> = new_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", matched.indent_prefix, line.trim_start())
            }
        })
        .collect();

    let replacement = reindented.join("\n");

    format!(
        "{}{}{}",
        &content[..matched.start],
        replacement,
        &content[matched.end..]
    )
}

/// Resolve a relative path within a sandbox root directory.
///
/// Rejects absolute paths, parent-directory traversal (`..`), and symlinks
/// that resolve outside the sandbox root.  This is the same logic used by
/// [`LocalToolExecutor::resolve_path`], extracted as a pure function for
/// reuse in diff preview generation.
pub(crate) fn resolve_sandbox_path(root: &Path, raw: &str) -> Result<PathBuf, ToolRuntimeError> {
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
    let joined = root.join(candidate);
    // If the path exists, canonicalize to resolve symlinks and verify
    // the result is still within the sandbox root.
    if joined.exists() {
        let canonical = fs::canonicalize(&joined).map_err(|err| {
            ToolRuntimeError::Io(format!(
                "failed to resolve path {}: {err}",
                joined.display()
            ))
        })?;
        let root_canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !canonical.starts_with(&root_canonical) {
            return Err(ToolRuntimeError::InvalidPath(format!(
                "{raw} resolves outside sandbox"
            )));
        }
    }
    Ok(joined)
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
    build_completed_result_with_diff(request, summary, payload, artifacts, started, None)
}

fn build_completed_result_with_diff(
    request: &ToolExecutionRequest,
    summary: String,
    payload: ToolExecutionPayload,
    artifacts: Vec<String>,
    started: Instant,
    diff_summary: Option<String>,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_call_id: request.tool_call_id.clone(),
        tool_name: request.spec.name.clone(),
        status: ToolExecutionStatus::Completed,
        summary,
        payload,
        artifacts,
        elapsed_ms: started.elapsed().as_millis(),
        diff_summary,
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
            // Use effective_permission_class (not spec.permission_class) so that
            // safe shell commands (e.g. git log) are correctly recognised as Safe.
            // Currently shell.exec is SequentialOnly so it never reaches this path,
            // but we use the effective class defensively for future-proofing.
            let effective = effective_permission_class(&call.request.input, &call.spec);
            if policy.approval_required && effective != PermissionClass::Safe && !call.approved {
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
        ToolInput::FileEdit {
            path, old_string, ..
        } => {
            if path.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "path".to_string(),
                ));
            }
            if old_string.is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "old_string".to_string(),
                ));
            }
        }
        ToolInput::FileSearch {
            root,
            pattern,
            regex,
            context_lines,
        } => {
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
            if *context_lines > MAX_CONTEXT_LINES {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "context_lines".to_string(),
                    reason: format!(
                        "context_lines {} exceeds maximum of {}",
                        context_lines, MAX_CONTEXT_LINES
                    ),
                });
            }
            if *regex && let Err(err) = regex::Regex::new(pattern) {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "pattern".to_string(),
                    reason: format!("invalid regex pattern: {err}"),
                });
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
        ToolInput::AgentExplore { prompt, .. } | ToolInput::AgentPlan { prompt, .. } => {
            if prompt.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "prompt".to_string(),
                ));
            }
            const MAX_PROMPT_LENGTH: usize = 10000;
            if prompt.len() > MAX_PROMPT_LENGTH {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "prompt".to_string(),
                    reason: format!(
                        "prompt length {} exceeds maximum of {} characters",
                        prompt.len(),
                        MAX_PROMPT_LENGTH
                    ),
                });
            }
        }
        ToolInput::FileEditAnchor { path, params } => {
            if path.trim().is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "path".to_string(),
                ));
            }
            if params.old_content.is_empty() {
                return Err(ToolValidationError::MissingRequiredField(
                    "old_content".to_string(),
                ));
            }
        }
        // [D3-001] MCP tool input validation is handled by the MCP server side
        ToolInput::Mcp { .. } => {}
        ToolInput::GitStatus {} => {}
        ToolInput::GitDiff { commit, path, .. } => {
            if let Some(c) = commit {
                validate_git_ref(c)?;
            }
            if let Some(p) = path {
                validate_git_path(p)?;
            }
        }
        ToolInput::GitLog { count, path } => {
            if let Some(c) = count
                && (*c == 0 || *c > 100)
            {
                return Err(ToolValidationError::InvalidFieldValue {
                    field: "count".to_string(),
                    reason: "count must be between 1 and 100".to_string(),
                });
            }
            if let Some(p) = path {
                validate_git_path(p)?;
            }
        }
    }

    Ok(())
}

/// Validate a git ref value (commit, branch name, etc.).
///
/// Rejects values starting with `-` (flag injection) and values not matching
/// `^[a-zA-Z0-9_.~^/-]+$`.
fn validate_git_ref(value: &str) -> Result<(), ToolValidationError> {
    if value.starts_with('-') {
        return Err(ToolValidationError::InvalidFieldValue {
            field: "commit".to_string(),
            reason: "commit must not start with '-' (flag injection prevention)".to_string(),
        });
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '^' | '/' | '-'))
    {
        return Err(ToolValidationError::InvalidFieldValue {
            field: "commit".to_string(),
            reason: format!("commit contains invalid characters: {value}"),
        });
    }
    Ok(())
}

/// Validate a git path parameter.
///
/// Rejects paths containing `..` as a defence-in-depth measure.
fn validate_git_path(path: &str) -> Result<(), ToolValidationError> {
    if path.contains("..") {
        return Err(ToolValidationError::InvalidFieldValue {
            field: "path".to_string(),
            reason: "path must not contain '..' (path traversal prevention)".to_string(),
        });
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
///
/// This is a backward-compatible wrapper around [`classify_shell_policy`].
pub fn is_safe_shell_command(command: &str) -> bool {
    classify_shell_policy(command) != ShellPolicy::General
}

/// Compute the effective permission class for a tool call.
///
/// Shell commands are classified via [`classify_shell_policy`]:
/// - `ReadOnly` / `BuildTest` → `Safe` (auto-approved)
/// - `General` → uses spec's permission class (typically `Confirm`)
///
/// All other tools (including MCP) use their spec's permission class directly.
pub fn effective_permission_class(input: &ToolInput, spec: &ToolSpec) -> PermissionClass {
    match input {
        ToolInput::ShellExec { command } => match classify_shell_policy(command) {
            ShellPolicy::ReadOnly | ShellPolicy::BuildTest => PermissionClass::Safe,
            ShellPolicy::General => spec.permission_class,
        },
        _ => spec.permission_class,
    }
}

/// Parse an MCP tool name: "mcp__github__create_issue" → ("github", "create_issue").
///
/// Returns `None` if the name does not follow the `mcp__<server>__<tool>` convention.
pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.splitn(3, "__").collect();
    if parts.len() == 3 && parts[0] == "mcp" && !parts[1].is_empty() && !parts[2].is_empty() {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
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

/// Combine stdout and stderr into a single output string.
///
/// If only one stream has content, returns just that stream.
/// If both have content, joins them with a `--- stderr ---` separator.
fn combine_process_output(stdout: String, stderr: String) -> String {
    if stderr.trim().is_empty() {
        stdout
    } else if stdout.trim().is_empty() {
        stderr
    } else {
        format!("{stdout}--- stderr ---\n{stderr}")
    }
}

/// Locale parameters for DuckDuckGo search (CJK locale support).
pub struct LocaleParams {
    pub kl: String,
    pub accept_language: String,
}

/// Resolve a LANG/LC_ALL string into DuckDuckGo locale parameters.
/// Returns `None` for non-CJK locales (English, C, POSIX, etc.).
pub fn resolve_locale_params(lang: &str) -> Option<LocaleParams> {
    let prefix = lang
        .split(['_', '.', '-'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match prefix.as_str() {
        "ja" => Some(LocaleParams {
            kl: "jp-ja".into(),
            accept_language: "ja,en;q=0.9".into(),
        }),
        "zh" => Some(LocaleParams {
            kl: "cn-zh".into(),
            accept_language: "zh,en;q=0.9".into(),
        }),
        "ko" => Some(LocaleParams {
            kl: "kr-kr".into(),
            accept_language: "ko,en;q=0.9".into(),
        }),
        _ => None,
    }
}

/// Detect the system locale from environment variables.
fn detect_system_locale() -> Option<String> {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .ok()
        .filter(|v| !v.is_empty())
}

/// Detect whether a DuckDuckGo response body indicates a CAPTCHA block.
/// Returns `false` when search results are present (`results_count > 0`)
/// or when result elements are found in the HTML.
pub fn is_captcha_response(body: &str, results_count: usize) -> bool {
    if results_count > 0 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    if lower.contains("result__a") {
        return false;
    }
    let ddg_captcha = body.contains("Unfortunately, bots use DuckDuckGo too.");
    let generic_captcha = lower.contains("captcha");
    ddg_captcha || generic_captcha
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

// ---------------------------------------------------------------------------
// file.search v2: regex support, context lines, result limits
// ---------------------------------------------------------------------------

/// Internal search pattern representation.
enum SearchPattern {
    Literal(String),
    Regex(regex::Regex),
}

impl SearchPattern {
    /// Check whether `text` matches this pattern.
    fn is_match(&self, text: &str) -> bool {
        match self {
            SearchPattern::Literal(lit) => text.contains(lit.as_str()),
            SearchPattern::Regex(re) => re.is_match(text),
        }
    }
}

/// A single matched line with surrounding context.
struct MatchedLine {
    line_number: usize,
    content: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

/// Intermediate search result for a single file.
struct FileMatchResult {
    path: String,
    matched_lines: Vec<MatchedLine>,
}

/// Maximum number of matches collected per file.
const MAX_MATCHES_PER_FILE: usize = 50;

/// Collect context lines around matches in a single file.
fn collect_context_lines(
    path: &Path,
    pattern: &SearchPattern,
    context_lines: u32,
) -> Vec<MatchedLine> {
    use std::io::BufRead;

    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let ctx = context_lines as usize;

    let mut results = Vec::new();
    for (idx, line) in all_lines.iter().enumerate() {
        if results.len() >= MAX_MATCHES_PER_FILE {
            break;
        }
        if pattern.is_match(line) {
            let start = idx.saturating_sub(ctx);
            let end = (idx + ctx + 1).min(all_lines.len());

            let context_before: Vec<String> = all_lines[start..idx].to_vec();
            let context_after: Vec<String> = if idx + 1 < end {
                all_lines[idx + 1..end].to_vec()
            } else {
                Vec::new()
            };

            results.push(MatchedLine {
                line_number: idx + 1, // 1-based
                content: line.clone(),
                context_before,
                context_after,
            });
        }
    }
    results
}

/// Recursively collect file matches with search pattern support and result limits.
fn collect_search_matches_v2(
    root: &Path,
    pattern: &SearchPattern,
    context_lines: u32,
    results: &mut Vec<FileMatchResult>,
    total_count: &mut usize,
) -> Result<(), ToolRuntimeError> {
    if root.is_file() {
        check_file_match_v2(root, pattern, context_lines, results, total_count);
        return Ok(());
    }

    for path in crate::walk::walk(root) {
        if results.len() >= MAX_SEARCH_RESULTS {
            break;
        }
        check_file_match_v2(&path, pattern, context_lines, results, total_count);
    }
    Ok(())
}

/// Check whether a single file matches the pattern. Collects context if requested.
fn check_file_match_v2(
    path: &Path,
    pattern: &SearchPattern,
    context_lines: u32,
    results: &mut Vec<FileMatchResult>,
    total_count: &mut usize,
) {
    use std::io::BufRead;

    let path_str = path.display().to_string();

    // Path name match is always literal contains (even for regex mode).
    let pattern_str = match pattern {
        SearchPattern::Literal(lit) => lit.as_str(),
        SearchPattern::Regex(re) => re.as_str(),
    };
    if path_str.contains(pattern_str) {
        *total_count += 1;
        if results.len() < MAX_SEARCH_RESULTS {
            results.push(FileMatchResult {
                path: path_str,
                matched_lines: Vec::new(), // path-only match
            });
        }
        return;
    }

    // Content search
    if context_lines > 0 {
        let matched = collect_context_lines(path, pattern, context_lines);
        if !matched.is_empty() {
            *total_count += 1;
            if results.len() < MAX_SEARCH_RESULTS {
                results.push(FileMatchResult {
                    path: path_str,
                    matched_lines: matched,
                });
            }
        }
    } else {
        // Fast path: just check if any line matches
        if let Ok(file) = fs::File::open(path) {
            let reader = std::io::BufReader::new(file);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if pattern.is_match(&line) {
                    *total_count += 1;
                    if results.len() < MAX_SEARCH_RESULTS {
                        results.push(FileMatchResult {
                            path: path_str,
                            matched_lines: Vec::new(),
                        });
                    }
                    break;
                }
            }
        }
    }
}

/// Format file search results into a `ToolExecutionPayload`.
///
/// Returns `(payload, artifacts)`.
fn format_file_search_results(
    results: &[FileMatchResult],
    context_lines: u32,
    total_count: usize,
) -> (ToolExecutionPayload, Vec<String>) {
    let truncated = total_count > MAX_SEARCH_RESULTS;

    if context_lines == 0 {
        // Paths mode (backward compatible)
        let paths: Vec<String> = results.iter().map(|r| r.path.clone()).collect();
        let artifacts = paths.clone();
        let mut payload = ToolExecutionPayload::Paths(paths);
        if truncated {
            // Wrap in Text with notification
            if let ToolExecutionPayload::Paths(ref p) = payload {
                let mut text = p.join("\n");
                text.push_str(&format!(
                    "\n\n({total_count}件中{MAX_SEARCH_RESULTS}件を表示しています。パターンを絞り込んでください。)"
                ));
                payload = ToolExecutionPayload::Text(text);
            }
        }
        (payload, artifacts)
    } else {
        // Text mode with context lines
        let mut output = String::new();
        let artifacts: Vec<String> = results.iter().map(|r| r.path.clone()).collect();

        for (file_idx, result) in results.iter().enumerate() {
            if file_idx > 0 {
                output.push_str("--\n");
            }

            if result.matched_lines.is_empty() {
                // Path-only match
                output.push_str(&result.path);
                output.push('\n');
            } else {
                for matched in &result.matched_lines {
                    // Context before
                    let start_line = matched
                        .line_number
                        .saturating_sub(matched.context_before.len());
                    for (i, ctx_line) in matched.context_before.iter().enumerate() {
                        output.push_str(&format!(
                            "{}:{}: {}\n",
                            result.path,
                            start_line + i,
                            ctx_line
                        ));
                    }
                    // Match line
                    output.push_str(&format!(
                        "{}:{}: {}\n",
                        result.path, matched.line_number, matched.content
                    ));
                    // Context after
                    for (i, ctx_line) in matched.context_after.iter().enumerate() {
                        output.push_str(&format!(
                            "{}:{}: {}\n",
                            result.path,
                            matched.line_number + 1 + i,
                            ctx_line
                        ));
                    }
                }
            }
        }

        if truncated {
            output.push_str(&format!(
                "\n({total_count}件中{MAX_SEARCH_RESULTS}件を表示しています。パターンを絞り込んでください。)"
            ));
        }

        (ToolExecutionPayload::Text(output), artifacts)
    }
}

// ---------------------------------------------------------------------------
// Checkpoint / Undo types (Issue #68)
// ---------------------------------------------------------------------------

/// Maximum file size for checkpoint capture (1 MB).
pub const CHECKPOINT_FILE_SIZE_LIMIT: u64 = 1_048_576;

/// Checkpoint entry representing a single file state before a tool write.
#[derive(Debug, Clone)]
pub struct CheckpointEntry {
    /// Sandbox-resolved absolute path.
    pub path: PathBuf,
    /// File content before the write (`None` = file did not exist).
    pub previous_content: Option<String>,
    /// Byte size of the stored content (for capacity tracking).
    pub byte_size: usize,
}

impl CheckpointEntry {
    /// Generate a diff preview showing current file state vs. the checkpoint.
    ///
    /// Returns `None` when the file cannot be read (e.g. already deleted).
    pub fn generate_restore_preview(&self) -> Option<String> {
        let current = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if self.previous_content.is_none() {
                    return Some("(file does not exist, nothing to restore)".to_string());
                }
                return Some("(file was deleted externally, will recreate)".to_string());
            }
            Err(_) => return None,
        };

        let previous = self.previous_content.as_deref().unwrap_or("");
        if current == previous {
            return Some("(no changes to undo)".to_string());
        }

        let diff = similar::TextDiff::from_lines(current.as_str(), previous);
        let diff_text = diff
            .unified_diff()
            .context_radius(3)
            .header("a (current)", "b (restored)")
            .to_string();

        if diff_text.trim().is_empty() {
            Some("(no changes to undo)".to_string())
        } else {
            Some(diff_text)
        }
    }

    /// Restore this checkpoint entry to disk.
    ///
    /// Returns a [`RestoreResult`] describing the outcome.
    pub fn restore(&self) -> RestoreResult {
        let diff_preview = self.generate_restore_preview();
        match &self.previous_content {
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => RestoreResult {
                    path: self.path.clone(),
                    action: RestoreAction::FileRemoved,
                    diff_preview,
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => RestoreResult {
                    path: self.path.clone(),
                    action: RestoreAction::Skipped,
                    diff_preview: Some("(file already removed)".to_string()),
                },
                Err(err) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        "undo: failed to remove file: {err}"
                    );
                    RestoreResult {
                        path: self.path.clone(),
                        action: RestoreAction::Skipped,
                        diff_preview: Some(format!("(IO error: {err})")),
                    }
                }
            },
            Some(content) => match std::fs::write(&self.path, content) {
                Ok(()) => RestoreResult {
                    path: self.path.clone(),
                    action: RestoreAction::ContentRestored,
                    diff_preview,
                },
                Err(err) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        "undo: failed to restore file: {err}"
                    );
                    RestoreResult {
                        path: self.path.clone(),
                        action: RestoreAction::Skipped,
                        diff_preview: Some(format!("(IO error: {err})")),
                    }
                }
            },
        }
    }
}

/// Result of restoring a single checkpoint entry.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// Path of the restored file.
    pub path: PathBuf,
    /// What kind of restoration was performed.
    pub action: RestoreAction,
    /// Diff preview (for display).
    pub diff_preview: Option<String>,
}

/// Describes the type of restore action taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreAction {
    /// File content was restored to its previous state.
    ContentRestored,
    /// A newly created file was removed.
    FileRemoved,
    /// The entry was skipped (e.g. IO error).
    Skipped,
}

/// Stack-based checkpoint store for undo functionality.
pub struct CheckpointStack {
    entries: Vec<CheckpointEntry>,
    total_bytes: usize,
    max_depth: usize,
    max_bytes: usize,
    /// Active transaction mark position (`None` = no active transaction).
    active_mark: Option<usize>,
}

impl CheckpointStack {
    /// Default max depth = 20, max bytes = 10 MB.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            total_bytes: 0,
            max_depth: 20,
            max_bytes: 10 * 1024 * 1024,
            active_mark: None,
        }
    }

    /// Push a checkpoint entry. Returns the index at which it was stored.
    ///
    /// When the stack exceeds depth or byte limits, the oldest entries are
    /// discarded automatically. During an active transaction, only entries
    /// before the mark are eligible for eviction.
    pub fn push(&mut self, entry: CheckpointEntry) -> usize {
        self.total_bytes += entry.byte_size;
        self.entries.push(entry);
        self.evict_oldest_while(|s| s.entries.len() > s.max_depth);
        self.evict_oldest_while(|s| s.total_bytes > s.max_bytes);
        self.entries.len() - 1
    }

    /// Eviction helper (DRY for depth/byte limits).
    ///
    /// When `active_mark` is set, only entries before the mark position are
    /// evicted, and the mark value is decremented accordingly.
    ///
    /// Note: `Vec::remove(0)` is O(n) per call, but acceptable here since
    /// `max_depth` is small (default 20) and eviction runs infrequently.
    fn evict_oldest_while(&mut self, should_evict: impl Fn(&Self) -> bool) {
        while should_evict(self) && !self.entries.is_empty() {
            // During a transaction, refuse to evict entries at or after the mark.
            if self.active_mark == Some(0) {
                break;
            }
            self.total_bytes = self.total_bytes.saturating_sub(self.entries[0].byte_size);
            self.entries.remove(0);
            if let Some(ref mut mark) = self.active_mark {
                *mark = mark.saturating_sub(1);
            }
        }
    }

    /// Remove the entry at the given index (for rollback on tool failure).
    ///
    /// When an active transaction mark exists, the mark is adjusted if the
    /// removed entry was before the mark position.
    pub fn remove(&mut self, index: usize) -> Option<CheckpointEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        if let Some(ref mut mark) = self.active_mark
            && index < *mark
        {
            *mark = mark.saturating_sub(1);
        }
        Some(entry)
    }

    /// Record the current stack position as a transaction mark.
    ///
    /// Returns the mark value (current `entries.len()`). Use with
    /// `rollback_to_mark()` or `commit_mark()` to end the transaction.
    pub fn mark(&mut self) -> usize {
        let pos = self.entries.len();
        self.active_mark = Some(pos);
        pos
    }

    /// Clear the transaction mark without removing any entries.
    ///
    /// Called on successful transaction completion; checkpoints are kept
    /// for `/undo`.
    pub fn commit_mark(&mut self) {
        self.active_mark = None;
    }

    /// Whether a transaction is currently active.
    pub fn is_in_transaction(&self) -> bool {
        self.active_mark.is_some()
    }

    /// Pop all entries added since the transaction mark and return them
    /// (newest-first, deduplicated by path keeping the oldest checkpoint
    /// per file).
    ///
    /// The `_mark` parameter is accepted for call-site clarity but ignored
    /// internally; the actual mark position is tracked by [`mark()`] and
    /// may have been adjusted by eviction.  The transaction is always
    /// cleared after this call.
    pub fn rollback_to_mark(&mut self, _mark: usize) -> Vec<CheckpointEntry> {
        let effective_mark = match self.active_mark.take() {
            Some(m) => m,
            None => return Vec::new(),
        };
        if effective_mark >= self.entries.len() {
            return Vec::new();
        }
        let n = self.entries.len() - effective_mark;
        self.pop_n(n)
    }

    /// Pop the most recent entry.
    pub fn pop(&mut self) -> Option<CheckpointEntry> {
        let entry = self.entries.pop()?;
        self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        Some(entry)
    }

    /// Pop up to `n` entries from the stack (newest first).
    ///
    /// When the same file path appears multiple times, only the oldest
    /// entry (earliest checkpoint) is kept so that undo restores the file
    /// to its state before the first change.
    pub fn pop_n(&mut self, n: usize) -> Vec<CheckpointEntry> {
        let actual = n.min(self.entries.len());
        let mut popped = Vec::with_capacity(actual);
        for _ in 0..actual {
            if let Some(entry) = self.pop() {
                popped.push(entry);
            }
        }

        // Deduplicate by path: keep the oldest entry for each path.
        // Since we pop newest-first, the last occurrence in `popped` is the
        // oldest checkpoint. We iterate in reverse (oldest-first) and keep the
        // first occurrence we see for each path.
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::new();
        for entry in popped.into_iter().rev() {
            if !seen.insert(entry.path.clone()) {
                continue;
            }
            deduped.push(entry);
        }
        // Reverse back to newest-first order
        deduped.reverse();
        deduped
    }

    /// Number of entries currently in the stack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for CheckpointStack {
    fn default() -> Self {
        Self::new()
    }
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
