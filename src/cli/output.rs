use crate::roles::EffectiveModels;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::state::session::SessionState;

pub fn render_branding() -> &'static str {
    concat!(
        "    _              _ _\n",
        "   / \\   _ __   __(_) |\n",
        "  / _ \\ | '_ \\ / _` | |\n",
        " / ___ \\| | | | (_| | |\n",
        "/_/   \\_\\_| |_|\\__,_|_|\n",
        "\n",
        "Anvil  local-first coding agent\n"
    )
}

pub fn render_interactive_welcome(
    session: &SessionState,
    state_path: &str,
    models: &EffectiveModels,
    permission_mode: PermissionMode,
    network_policy: NetworkPolicy,
) -> String {
    let mut lines = vec![
        render_branding().trim_end().to_string(),
        format!("Session : {}", session.session_id),
        format!("State   : {state_path}"),
        format!("Mode    : {}", permission_mode_label(permission_mode)),
        format!("Network : {}", network_policy_label(network_policy)),
        String::new(),
        "Try one of these:".to_string(),
        "  inspect the repository layout".to_string(),
        "  /status".to_string(),
        "  /history".to_string(),
        "  /help".to_string(),
        "  /exit".to_string(),
        String::new(),
        "Type a task or command below.".to_string(),
    ];

    let snapshot = render_session_snapshot(session);
    if !snapshot.is_empty() {
        lines.push(String::new());
        lines.push(snapshot);
    }

    lines.push(String::new());
    lines.push(render_startup_summary(models, permission_mode, network_policy));
    lines.join("\n")
}

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

    if !session.active_plan_summary.is_empty() {
        lines.push(format!("Active plan: {}", session.active_plan_summary));
    }

    if !session.latest_evidence_summary.is_empty() {
        lines.push(format!("Latest evidence: {}", session.latest_evidence_summary));
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
        if !result.facts.is_empty() {
            let facts: Vec<String> = result
                .facts
                .iter()
                .take(3)
                .map(|fact| format!("{}={}", fact.key, fact.value))
                .collect();
            lines.push(format!("Facts: {}", facts.join(" | ")));
        }
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
    }

    if let Some(delegation) = session.recent_delegations.last() {
        lines.push(format!(
            "Last delegation: {} via {}",
            delegation.role, delegation.resolved_model
        ));
    }

    if let Some(pending) = &session.pending_confirmation {
        lines.push(format!(
            "Pending confirmation: {} - {}",
            pending.role, pending.reason
        ));
    }

    lines.join("\n")
}

pub fn render_session_history(session: &SessionState) -> String {
    let mut lines = Vec::new();

    if session.recent_results.is_empty() && session.recent_delegations.is_empty() {
        return "Session history is empty".to_string();
    }

    if !session.recent_results.is_empty() {
        lines.push("Recent results:".to_string());
        for result in session.recent_results.iter().rev().take(5) {
            lines.push(format!(
                "- {} via {}: {}",
                result.role, result.model, result.summary
            ));
            if !result.facts.is_empty() {
                let facts: Vec<String> = result
                    .facts
                    .iter()
                    .take(2)
                    .map(|fact| format!("{}={}", fact.key, fact.value))
                    .collect();
                lines.push(format!("  facts: {}", facts.join(" | ")));
            }
        }
    }

    if !session.recent_delegations.is_empty() {
        lines.push("Recent delegations:".to_string());
        for delegation in session.recent_delegations.iter().rev().take(5) {
            lines.push(format!(
                "- {} via {}: {}",
                delegation.role, delegation.resolved_model, delegation.task
            ));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        render_branding, render_interactive_welcome, render_session_history,
        render_session_snapshot,
    };
    use crate::roles::EffectiveModels;
    use crate::runtime::{NetworkPolicy, PermissionMode};
    use crate::state::session::{
        AgentModels, DelegationRecord, EvidenceRecord, FactRecord, PendingAction,
        PendingConfirmation, ResultRecord, SessionState,
    };

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
            active_plan_summary: "plan next".to_string(),
            latest_evidence_summary: "file.target=src/main.rs".to_string(),
            user_preferences_summary: String::new(),
            repository_summary: String::new(),
            active_constraints: Vec::new(),
            open_questions: Vec::new(),
            completed_steps: vec!["done".to_string()],
            pending_steps: vec!["next".to_string()],
            relevant_files: Vec::new(),
            recent_delegations: vec![DelegationRecord {
                id: "delegation-1".to_string(),
                role: "editor".to_string(),
                resolved_model: "editor-model".to_string(),
                inherited_from_pm: false,
                task: "apply change".to_string(),
                requested_permission: None,
                created_at: "2026-03-10T00:00:00Z".to_string(),
            }],
            recent_results: vec![ResultRecord {
                role: "editor".to_string(),
                model: "editor-model".to_string(),
                summary: "applied change".to_string(),
                facts: vec![FactRecord {
                    key: "file.target".to_string(),
                    value: "src/main.rs".to_string(),
                }],
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
            pending_confirmation: Some(PendingConfirmation {
                role: "tester".to_string(),
                task: "run a build".to_string(),
                summary: "Tester is waiting to run `cargo build`".to_string(),
                reason: "destructive commands require explicit user confirmation".to_string(),
                action: PendingAction::Exec {
                    program: "git".to_string(),
                    args: vec!["clean".to_string(), "-fd".to_string()],
                    cwd: "/tmp".to_string(),
                    display: "git clean -fd".to_string(),
                },
            }),
        };

        let rendered = render_session_snapshot(&session);
        assert!(rendered.contains("Active plan: plan next"));
        assert!(rendered.contains("Latest evidence: file.target=src/main.rs"));
        assert!(rendered.contains("Commands run: cargo check"));
        assert!(rendered.contains("Facts: file.target=src/main.rs"));
        assert!(rendered.contains("Changed files: src/main.rs"));
        assert!(rendered.contains("Evidence: repo-file: mutated src/main.rs"));
        assert!(rendered.contains("tool-output: stdout: ok"));
        assert!(rendered.contains("Last delegation: editor via editor-model"));
        assert!(rendered.contains("Pending confirmation: tester - destructive commands require explicit user confirmation"));
    }

    #[test]
    fn session_history_shows_recent_results_and_delegations() {
        let session = SessionState {
            session_id: "session-1".to_string(),
            pm_model: "pm-model".to_string(),
            permission_mode: PermissionMode::WorkspaceWrite,
            network_policy: NetworkPolicy::LocalOnly,
            agent_models: AgentModels::default(),
            objective: "objective".to_string(),
            working_summary: "working".to_string(),
            active_plan_summary: String::new(),
            latest_evidence_summary: String::new(),
            user_preferences_summary: String::new(),
            repository_summary: String::new(),
            active_constraints: Vec::new(),
            open_questions: Vec::new(),
            completed_steps: Vec::new(),
            pending_steps: Vec::new(),
            relevant_files: Vec::new(),
            recent_delegations: vec![DelegationRecord {
                id: "delegation-1".to_string(),
                role: "reader".to_string(),
                resolved_model: "pm-model".to_string(),
                inherited_from_pm: true,
                task: "inspect repo".to_string(),
                requested_permission: None,
                created_at: "2026-03-10T00:00:00Z".to_string(),
            }],
            recent_results: vec![ResultRecord {
                role: "reader".to_string(),
                model: "pm-model".to_string(),
                summary: "Reader inspected the repo".to_string(),
                facts: vec![FactRecord {
                    key: "repo.tracked_files".to_string(),
                    value: "42".to_string(),
                }],
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run: Vec::new(),
                next_recommendation: None,
                findings: Vec::new(),
            }],
            pending_confirmation: None,
        };

        let rendered = render_session_history(&session);
        assert!(rendered.contains("Recent results:"));
        assert!(rendered.contains("- reader via pm-model: Reader inspected the repo"));
        assert!(rendered.contains("facts: repo.tracked_files=42"));
        assert!(rendered.contains("Recent delegations:"));
        assert!(rendered.contains("- reader via pm-model: inspect repo"));
    }

    #[test]
    fn interactive_welcome_renders_branding_and_examples() {
        let session = SessionState {
            session_id: "session-1".to_string(),
            pm_model: "pm-model".to_string(),
            permission_mode: PermissionMode::WorkspaceWrite,
            network_policy: NetworkPolicy::Disabled,
            agent_models: AgentModels::default(),
            objective: "objective".to_string(),
            working_summary: "working".to_string(),
            active_plan_summary: String::new(),
            latest_evidence_summary: String::new(),
            user_preferences_summary: String::new(),
            repository_summary: String::new(),
            active_constraints: Vec::new(),
            open_questions: Vec::new(),
            completed_steps: Vec::new(),
            pending_steps: Vec::new(),
            relevant_files: Vec::new(),
            recent_delegations: Vec::new(),
            recent_results: Vec::new(),
            pending_confirmation: None,
        };
        let models = EffectiveModels {
            pm_model: "pm-model".to_string(),
            roles: Vec::new(),
        };

        let rendered = render_interactive_welcome(
            &session,
            "/tmp/session-1.json",
            &models,
            PermissionMode::WorkspaceWrite,
            NetworkPolicy::Disabled,
        );

        assert!(render_branding().contains("Anvil"));
        assert!(rendered.contains("Try one of these:"));
        assert!(rendered.contains("inspect the repository layout"));
        assert!(rendered.contains("Type a task or command below."));
    }
}
