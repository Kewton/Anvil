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
    pub name: String,
    pub kind: ToolKind,
    pub execution_class: ExecutionClass,
    pub permission_class: PermissionClass,
    pub execution_mode: ExecutionMode,
    pub plan_mode: PlanModePolicy,
    pub rollback_policy: RollbackPolicy,
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
        approval_mode: bool,
    ) -> Result<ToolExecutionRequest, ToolExecutionError> {
        if self.spec.permission_class == PermissionClass::Restricted {
            return Err(ToolExecutionError::RestrictedTool(self.spec.name.clone()));
        }

        if self.approval_required(approval_mode).is_some() && !self.approved {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionError {
    ApprovalRequired(String),
    RestrictedTool(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolValidationError {
    UnknownTool,
    InputKindMismatch,
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
        approval_mode: bool,
    ) -> Result<Self, ParallelExecutionPlanError> {
        for call in &calls {
            if approval_mode
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
