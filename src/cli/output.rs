use crate::cli::flags::{NetworkPolicyArg, PermissionModeArg};
use crate::roles::EffectiveModels;

pub fn render_startup_summary(
    models: &EffectiveModels,
    permission_mode: PermissionModeArg,
    network_policy: NetworkPolicyArg,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("PM: {}", models.pm_model));

    for (role, model, inherited) in models.user_facing_roles() {
        if inherited {
            lines.push(format!("{role}: {model} (inherited)"));
        } else {
            lines.push(format!("{role}: {model}"));
        }
    }

    lines.push(format!("Permission mode: {}", permission_mode_label(permission_mode)));
    lines.push(format!("Network: {}", network_policy_label(network_policy)));
    lines.join("\n")
}

fn permission_mode_label(mode: PermissionModeArg) -> &'static str {
    match mode {
        PermissionModeArg::ReadOnly => "read-only",
        PermissionModeArg::WorkspaceWrite => "workspace-write",
        PermissionModeArg::FullAccess => "full-access",
    }
}

fn network_policy_label(policy: NetworkPolicyArg) -> &'static str {
    match policy {
        NetworkPolicyArg::Disabled => "disabled",
        NetworkPolicyArg::LocalOnly => "local-only",
        NetworkPolicyArg::EnabledWithApproval => "enabled-with-approval",
    }
}
