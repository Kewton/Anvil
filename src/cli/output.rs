use crate::roles::EffectiveModels;
use crate::runtime::{NetworkPolicy, PermissionMode};

pub fn render_startup_summary(
    models: &EffectiveModels,
    permission_mode: PermissionMode,
    network_policy: NetworkPolicy,
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

    lines.push(format!(
        "Permission mode: {}",
        permission_mode_label(permission_mode)
    ));
    lines.push(format!("Network: {}", network_policy_label(network_policy)));
    lines.join("\n")
}

fn permission_mode_label(mode: PermissionMode) -> &'static str {
    mode.as_str()
}

fn network_policy_label(policy: NetworkPolicy) -> &'static str {
    policy.as_str()
}
