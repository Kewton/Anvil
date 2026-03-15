//! Tool registry, validation, and local execution.
//!
//! Tools are declared as [`ToolSpec`] entries in a [`ToolRegistry`], validated
//! through a permission and plan-mode pipeline, and executed by
//! [`LocalToolExecutor`] within a sandboxed workspace root.

use crate::contracts::ToolLogView;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolInput {
    FileRead { path: String },
    FileWrite { path: String, content: String },
    FileSearch { root: String, pattern: String },
    ShellExec { command: String },
}

impl ToolInput {
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::FileRead { .. } => ToolKind::FileRead,
            Self::FileWrite { .. } => ToolKind::FileWrite,
            Self::FileSearch { .. } => ToolKind::FileSearch,
            Self::ShellExec { .. } => ToolKind::ShellExec,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

        match self.spec.permission_class {
            PermissionClass::Safe => None,
            PermissionClass::Confirm | PermissionClass::Restricted => Some(ApprovalRequest {
                tool_call_id: self.request.tool_call_id.clone(),
                tool_name: self.spec.name.clone(),
            }),
        }
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
    DangerousCommand(String),
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
            permission_class: PermissionClass::Restricted,
            execution_mode: ExecutionMode::SequentialOnly,
            plan_mode: PlanModePolicy::Blocked,
            rollback_policy: RollbackPolicy::None,
        });
    }

    pub fn register_standard_tools(&mut self) {
        self.register_file_read();
        self.register_file_write();
        self.register_file_search();
        self.register_shell_exec();
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

pub struct LocalToolExecutor {
    root: PathBuf,
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
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn execute(
        &self,
        request: ToolExecutionRequest,
    ) -> Result<ToolExecutionResult, ToolRuntimeError> {
        let started = Instant::now();
        match request.input {
            ToolInput::FileRead { path } => {
                let resolved = self.resolve_path(&path)?;
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
                Ok(ToolExecutionResult {
                    tool_call_id: request.tool_call_id,
                    tool_name: request.spec.name,
                    status: ToolExecutionStatus::Completed,
                    summary: path,
                    payload: ToolExecutionPayload::Text(content),
                    artifacts: vec![resolved.display().to_string()],
                    elapsed_ms: started.elapsed().as_millis(),
                })
            }
            ToolInput::FileWrite { path, content } => {
                let resolved = self.resolve_path(&path)?;
                if let Some(parent) = resolved.parent() {
                    fs::create_dir_all(parent).map_err(|err| {
                        ToolRuntimeError::Io(format!(
                            "file.write failed to create parent {}: {err}",
                            parent.display()
                        ))
                    })?;
                }
                fs::write(&resolved, &content).map_err(|err| {
                    ToolRuntimeError::Io(format!(
                        "file.write failed for {}: {err}",
                        resolved.display()
                    ))
                })?;
                Ok(ToolExecutionResult {
                    tool_call_id: request.tool_call_id,
                    tool_name: request.spec.name,
                    status: ToolExecutionStatus::Completed,
                    summary: path,
                    payload: ToolExecutionPayload::None,
                    artifacts: vec![resolved.display().to_string()],
                    elapsed_ms: started.elapsed().as_millis(),
                })
            }
            ToolInput::FileSearch { root, pattern } => {
                let resolved_root = self.resolve_path(&root)?;
                let mut matches = Vec::new();
                collect_search_matches(&resolved_root, &pattern, &mut matches)?;
                Ok(ToolExecutionResult {
                    tool_call_id: request.tool_call_id,
                    tool_name: request.spec.name,
                    status: ToolExecutionStatus::Completed,
                    summary: format!("{root} :: {pattern}"),
                    payload: ToolExecutionPayload::Paths(matches.clone()),
                    artifacts: matches,
                    elapsed_ms: started.elapsed().as_millis(),
                })
            }
            ToolInput::ShellExec { command } => Err(ToolRuntimeError::Io(format!(
                "shell.exec is not enabled in the local executor: {command}"
            ))),
        }
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
        Ok(self.root.join(candidate))
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
    }

    Ok(())
}

/// Reject shell commands that contain dangerous patterns.
///
/// This is a defence-in-depth measure.  The primary protection is the
/// `Restricted` permission class which blocks `shell.exec` by default.
fn validate_shell_command_safety(command: &str) -> Result<(), ToolValidationError> {
    const BLOCKED_PATTERNS: &[&str] = &[
        "rm -rf /",
        "rm -rf ~",
        "mkfs",
        "dd if=",
        ":(){",
        ">(", // process substitution
    ];

    let lower = command.to_ascii_lowercase();
    for pattern in BLOCKED_PATTERNS {
        if lower.contains(pattern) {
            return Err(ToolValidationError::DangerousCommand(command.to_string()));
        }
    }

    Ok(())
}

fn collect_search_matches(
    root: &Path,
    pattern: &str,
    matches: &mut Vec<String>,
) -> Result<(), ToolRuntimeError> {
    if root.is_file() {
        let content = fs::read_to_string(root).unwrap_or_default();
        if root.display().to_string().contains(pattern) || content.contains(pattern) {
            matches.push(root.display().to_string());
        }
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
        if path.is_dir() {
            collect_search_matches(&path, pattern, matches)?;
        } else {
            let content = fs::read_to_string(&path).unwrap_or_default();
            if path.display().to_string().contains(pattern) || content.contains(pattern) {
                matches.push(path.display().to_string());
            }
        }
    }

    Ok(())
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
