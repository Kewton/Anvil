use crate::roles::EffectiveModels;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::state::session::SessionState;

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

pub fn render_session_snapshot(session: &SessionState) -> String {
    let mut lines = Vec::new();

    if !session.working_summary.is_empty() {
        lines.push(format!("Working summary: {}", session.working_summary));
    }

    if !session.pending_steps.is_empty() {
        lines.push(format!(
            "Pending steps: {}",
            session.pending_steps.join(" | ")
        ));
    }

    if let Some(result) = session.recent_results.last() {
        lines.push(format!(
            "Last result: {} via {} - {}",
            result.role, result.model, result.summary
        ));
        if let Some(next) = &result.next_recommendation {
            lines.push(format!("Next recommendation: {next}"));
        }
    } else if let Some(delegation) = session.recent_delegations.last() {
        lines.push(format!(
            "Last delegation: {} via {}",
            delegation.role, delegation.resolved_model
        ));
    }

    lines.join("\n")
}
