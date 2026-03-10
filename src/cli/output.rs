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

    if let Some(step) = session.completed_steps.last() {
        lines.push(format!("Last completed step: {step}"));
    }

    if let Some(result) = session.recent_results.last() {
        lines.push(format!(
            "Last result: {} via {} - {}",
            result.role, result.model, result.summary
        ));
        if !result.commands_run.is_empty() {
            lines.push(format!("Commands run: {}", result.commands_run.join(" | ")));
        }
        if !result.changed_files.is_empty() {
            lines.push(format!(
                "Changed files: {}",
                result.changed_files.join(" | ")
            ));
        }
        if !result.evidence.is_empty() {
            let evidence: Vec<String> = result
                .evidence
                .iter()
                .take(2)
                .map(|record| format!("{}: {}", record.source_type, record.value))
                .collect();
            lines.push(format!("Evidence: {}", evidence.join(" | ")));
        }
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

#[cfg(test)]
mod tests {
    use super::render_session_snapshot;
    use crate::runtime::{NetworkPolicy, PermissionMode};
    use crate::state::session::{AgentModels, EvidenceRecord, ResultRecord, SessionState};

    #[test]
    fn session_snapshot_shows_recent_result_details() {
        let session = SessionState {
            session_id: "session-1".to_string(),
            pm_model: "pm-model".to_string(),
            permission_mode: PermissionMode::WorkspaceWrite,
            network_policy: NetworkPolicy::LocalOnly,
            agent_models: AgentModels::default(),
            objective: "objective".to_string(),
            working_summary: "working".to_string(),
            user_preferences_summary: String::new(),
            repository_summary: String::new(),
            active_constraints: Vec::new(),
            open_questions: Vec::new(),
            completed_steps: vec!["done".to_string()],
            pending_steps: vec!["next".to_string()],
            relevant_files: Vec::new(),
            recent_delegations: Vec::new(),
            recent_results: vec![ResultRecord {
                role: "editor".to_string(),
                model: "editor-model".to_string(),
                summary: "applied change".to_string(),
                evidence: vec![
                    EvidenceRecord {
                        source_type: "repo-file".to_string(),
                        value: "mutated src/main.rs".to_string(),
                    },
                    EvidenceRecord {
                        source_type: "tool-output".to_string(),
                        value: "stdout: ok".to_string(),
                    },
                ],
                changed_files: vec!["src/main.rs".to_string()],
                commands_run: vec!["cargo check".to_string()],
                next_recommendation: Some("run tests".to_string()),
                findings: Vec::new(),
            }],
        };

        let rendered = render_session_snapshot(&session);
        assert!(rendered.contains("Commands run: cargo check"));
        assert!(rendered.contains("Changed files: src/main.rs"));
        assert!(rendered.contains("Evidence: repo-file: mutated src/main.rs"));
        assert!(rendered.contains("tool-output: stdout: ok"));
    }
}
