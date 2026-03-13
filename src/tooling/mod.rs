use crate::contracts::ToolLogView;
use std::collections::HashMap;

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
        }
    }

    Ok(())
}
