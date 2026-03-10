use std::path::{Path, PathBuf};

use crate::policy::command_classification::{classify_command, CommandClass};
use crate::policy::network_policy::NetworkAccessPolicy;
use crate::policy::path_policy::PathPolicy;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::tools::registry::ToolRequest;

#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    permission_mode: PermissionMode,
    network_policy: NetworkAccessPolicy,
    path_policy: PathPolicy,
}

impl SandboxPolicy {
    pub fn new(
        permission_mode: PermissionMode,
        network_policy: NetworkPolicy,
        workspace_root: PathBuf,
        writable_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            permission_mode,
            network_policy: NetworkAccessPolicy::new(network_policy),
            path_policy: PathPolicy::new(workspace_root, writable_roots),
        }
    }

    pub fn evaluate(&self, request: &ToolRequest) -> PermissionDecision {
        match request {
            ToolRequest::ReadFile { .. }
            | ToolRequest::Search { .. }
            | ToolRequest::InspectEnv
            | ToolRequest::Diff { .. } => PermissionDecision::Allowed,
            ToolRequest::WriteFile { path, .. } => {
                if self.permission_mode == PermissionMode::ReadOnly {
                    PermissionDecision::Blocked(
                        "write access is blocked in read-only mode".to_string(),
                    )
                } else if self.path_policy.allows_write(path) {
                    PermissionDecision::Allowed
                } else {
                    PermissionDecision::Blocked(format!(
                        "write path {} is outside the approved writable roots",
                        path.display()
                    ))
                }
            }
            ToolRequest::Exec { request } => {
                let class = classify_command(&request.program, &request.args);
                self.evaluate_exec_class(class)
            }
        }
    }

    pub fn workspace_root(&self) -> &Path {
        self.path_policy.workspace_root()
    }

    fn evaluate_exec_class(&self, class: CommandClass) -> PermissionDecision {
        match class {
            CommandClass::SafeRead => PermissionDecision::Allowed,
            CommandClass::LocalValidation => match self.permission_mode {
                PermissionMode::ReadOnly => PermissionDecision::Blocked(
                    "local validation commands require workspace-write or stronger".to_string(),
                ),
                PermissionMode::WorkspaceWrite | PermissionMode::FullAccess => {
                    PermissionDecision::Allowed
                }
            },
            CommandClass::Networked => match self.network_policy.policy() {
                NetworkPolicy::Disabled | NetworkPolicy::LocalOnly => {
                    PermissionDecision::NeedsConfirmation(
                        "networked commands require explicit approval".to_string(),
                    )
                }
                NetworkPolicy::EnabledWithApproval => PermissionDecision::NeedsConfirmation(
                    "networked commands remain confirmation-gated in the MVP".to_string(),
                ),
            },
            CommandClass::Destructive => PermissionDecision::NeedsConfirmation(
                "destructive commands require explicit user confirmation".to_string(),
            ),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PermissionDecision {
    Allowed,
    NeedsConfirmation(String),
    Blocked(String),
}
