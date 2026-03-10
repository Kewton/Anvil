use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    Ask,
    AcceptEdits,
    BypassPermissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionCategory {
    Read,
    Edit,
    ExecSafe,
    ExecSensitive,
    ExecDangerous,
    SubagentRead,
    SubagentWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRequirement {
    Allow,
    Ask,
    SoftConfirm,
    HardConfirm,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    Interactive,
    NonInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonInteractiveBehavior {
    Deny,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub interaction_mode: InteractionMode,
    pub non_interactive_ask: NonInteractiveBehavior,
    pub non_interactive_soft_confirm: NonInteractiveBehavior,
    pub non_interactive_hard_confirm: NonInteractiveBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub category: PermissionCategory,
}

impl PermissionPolicy {
    pub fn from_mode(mode: PermissionMode, category: PermissionCategory) -> Self {
        Self { mode, category }
    }

    pub fn base_requirement(&self) -> PermissionRequirement {
        match (self.mode, self.category) {
            (PermissionMode::Ask, PermissionCategory::Read) => PermissionRequirement::Allow,
            (PermissionMode::Ask, PermissionCategory::Edit) => PermissionRequirement::Ask,
            (PermissionMode::Ask, PermissionCategory::ExecSafe) => PermissionRequirement::Ask,
            (PermissionMode::Ask, PermissionCategory::ExecSensitive) => PermissionRequirement::Ask,
            (PermissionMode::Ask, PermissionCategory::ExecDangerous) => {
                PermissionRequirement::HardConfirm
            }
            (PermissionMode::Ask, PermissionCategory::SubagentRead) => PermissionRequirement::Ask,
            (PermissionMode::Ask, PermissionCategory::SubagentWrite) => PermissionRequirement::Ask,
            (PermissionMode::AcceptEdits, PermissionCategory::Read) => PermissionRequirement::Allow,
            (PermissionMode::AcceptEdits, PermissionCategory::Edit) => PermissionRequirement::Allow,
            (PermissionMode::AcceptEdits, PermissionCategory::ExecSafe) => {
                PermissionRequirement::Ask
            }
            (PermissionMode::AcceptEdits, PermissionCategory::ExecSensitive) => {
                PermissionRequirement::Ask
            }
            (PermissionMode::AcceptEdits, PermissionCategory::ExecDangerous) => {
                PermissionRequirement::HardConfirm
            }
            (PermissionMode::AcceptEdits, PermissionCategory::SubagentRead) => {
                PermissionRequirement::Ask
            }
            (PermissionMode::AcceptEdits, PermissionCategory::SubagentWrite) => {
                PermissionRequirement::Ask
            }
            (PermissionMode::BypassPermissions, PermissionCategory::Read) => {
                PermissionRequirement::Allow
            }
            (PermissionMode::BypassPermissions, PermissionCategory::Edit) => {
                PermissionRequirement::Allow
            }
            (PermissionMode::BypassPermissions, PermissionCategory::ExecSafe) => {
                PermissionRequirement::Allow
            }
            (PermissionMode::BypassPermissions, PermissionCategory::ExecSensitive) => {
                PermissionRequirement::SoftConfirm
            }
            (PermissionMode::BypassPermissions, PermissionCategory::ExecDangerous) => {
                PermissionRequirement::HardConfirm
            }
            (PermissionMode::BypassPermissions, PermissionCategory::SubagentRead) => {
                PermissionRequirement::Allow
            }
            (PermissionMode::BypassPermissions, PermissionCategory::SubagentWrite) => {
                PermissionRequirement::Ask
            }
        }
    }

    pub fn effective_requirement(&self, cx: ExecutionContext) -> PermissionRequirement {
        let base = self.base_requirement();
        match (cx.interaction_mode, base) {
            (InteractionMode::NonInteractive, PermissionRequirement::Ask) => {
                match cx.non_interactive_ask {
                    NonInteractiveBehavior::Deny => PermissionRequirement::Deny,
                    NonInteractiveBehavior::Allow => PermissionRequirement::Allow,
                }
            }
            (InteractionMode::NonInteractive, PermissionRequirement::SoftConfirm) => {
                match cx.non_interactive_soft_confirm {
                    NonInteractiveBehavior::Deny => PermissionRequirement::Deny,
                    NonInteractiveBehavior::Allow => PermissionRequirement::Allow,
                }
            }
            (InteractionMode::NonInteractive, PermissionRequirement::HardConfirm) => {
                PermissionRequirement::Deny
            }
            _ => base,
        }
    }
}
