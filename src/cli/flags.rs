use clap::ValueEnum;

use crate::runtime::{NetworkPolicy, PermissionMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum PermissionModeArg {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl From<PermissionModeArg> for PermissionMode {
    fn from(value: PermissionModeArg) -> Self {
        match value {
            PermissionModeArg::ReadOnly => PermissionMode::ReadOnly,
            PermissionModeArg::WorkspaceWrite => PermissionMode::WorkspaceWrite,
            PermissionModeArg::FullAccess => PermissionMode::FullAccess,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum NetworkPolicyArg {
    Disabled,
    LocalOnly,
    EnabledWithApproval,
}

impl From<NetworkPolicyArg> for NetworkPolicy {
    fn from(value: NetworkPolicyArg) -> Self {
        match value {
            NetworkPolicyArg::Disabled => NetworkPolicy::Disabled,
            NetworkPolicyArg::LocalOnly => NetworkPolicy::LocalOnly,
            NetworkPolicyArg::EnabledWithApproval => NetworkPolicy::EnabledWithApproval,
        }
    }
}
