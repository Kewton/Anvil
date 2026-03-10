use crate::agents::pm::{AgentRole, PmAgent};
use crate::roles::EffectiveModels;
use crate::runtime::engine::RuntimeEngine;
use crate::state::session::{DelegationRecord, EvidenceRecord, ResultRecord, SessionState};
use crate::util::clock::now_rfc3339;

#[derive(Debug, Default)]
pub struct RuntimeLoop;

impl RuntimeLoop {
    pub fn run_prompt(
        session: &mut SessionState,
        models: &EffectiveModels,
        pm: &PmAgent,
        runtime: &RuntimeEngine,
        context: &str,
        prompt: &str,
    ) -> anyhow::Result<String> {
        let outcome = pm.run_turn(models, prompt, context, runtime)?;

        if let Some(role) = outcome.delegated_role {
            session.recent_delegations.push(DelegationRecord {
                id: format!("delegation-{}", session.recent_delegations.len() + 1),
                role: role_label(role).to_string(),
                resolved_model: resolved_model_for(models, role).to_string(),
                inherited_from_pm: role_inherits(models, role),
                task: prompt.to_string(),
                requested_permission: None,
                created_at: now_rfc3339(),
            });
            trim_tail(&mut session.recent_delegations, 20);
        }

        if outcome.result.role != "pm" {
            let role = outcome
                .delegated_role
                .expect("non-pm results must come from delegated roles");
            let next_recommendation = outcome.result.next_recommendation.clone();
            let commands_run = outcome.result.commands_run.clone();
            let changed_files = outcome.result.changed_files.clone();
            let evidence = outcome
                .result
                .evidence
                .iter()
                .map(|(source_type, value)| EvidenceRecord {
                    source_type: source_type.clone(),
                    value: value.clone(),
                })
                .collect();
            session.recent_results.push(ResultRecord {
                role: outcome.result.role.clone(),
                model: resolved_model_for(models, role).to_string(),
                summary: outcome.result.summary.clone(),
                evidence,
                changed_files,
                commands_run,
                next_recommendation: next_recommendation.clone(),
                findings: Vec::new(),
            });
            trim_tail(&mut session.recent_results, 20);
            update_pending_steps(session, &outcome.result.role, next_recommendation);
        }
        session.working_summary = outcome.result.summary.clone();
        mark_completed_step(session, prompt);

        Ok(outcome.result.summary)
    }
}

fn trim_tail<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        let excess = items.len() - max;
        items.drain(0..excess);
    }
}

fn update_pending_steps(
    session: &mut SessionState,
    role: &str,
    next_recommendation: Option<String>,
) {
    if let Some(step) = next_recommendation {
        if matches_recent_completed_step(session, &step) {
            return;
        }
        remove_previous_recommendation_for_role(session, role, &step);
        session
            .pending_steps
            .retain(|existing| !same_step(existing, &step));
        session.pending_steps.push(step);
        trim_tail(&mut session.pending_steps, 20);
    }
}

fn mark_completed_step(session: &mut SessionState, prompt: &str) {
    let step = prompt.trim();
    if step.is_empty() {
        return;
    }

    session
        .completed_steps
        .retain(|existing| !same_step(existing, step));
    session.completed_steps.push(step.to_string());
    trim_tail(&mut session.completed_steps, 50);
    session
        .pending_steps
        .retain(|existing| !same_step(existing, step));
}

fn matches_recent_completed_step(session: &SessionState, step: &str) -> bool {
    session
        .completed_steps
        .iter()
        .rev()
        .take(5)
        .any(|existing| same_step(existing, step))
}

fn remove_previous_recommendation_for_role(session: &mut SessionState, role: &str, step: &str) {
    let previous = session
        .recent_results
        .iter()
        .rev()
        .skip(1)
        .find(|result| result.role == role)
        .and_then(|result| result.next_recommendation.as_deref());

    if let Some(previous) = previous {
        if !same_step(previous, step) {
            session
                .pending_steps
                .retain(|existing| !same_step(existing, previous));
        }
    }
}

fn same_step(left: &str, right: &str) -> bool {
    normalize_step(left) == normalize_step(right)
}

fn normalize_step(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn role_label(role: AgentRole) -> &'static str {
    match role {
        AgentRole::Reader => "reader",
        AgentRole::Editor => "editor",
        AgentRole::Tester => "tester",
        AgentRole::Reviewer => "reviewer",
    }
}

fn resolved_model_for(models: &EffectiveModels, role: AgentRole) -> &str {
    let role_id = role_label(role);
    models
        .roles
        .iter()
        .find(|entry| entry.role_id == role_id)
        .map(|entry| entry.model.as_str())
        .unwrap_or(models.pm_model.as_str())
}

fn role_inherits(models: &EffectiveModels, role: AgentRole) -> bool {
    let role_id = role_label(role);
    models
        .roles
        .iter()
        .find(|entry| entry.role_id == role_id)
        .map(|entry| entry.inherited)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{mark_completed_step, update_pending_steps};
    use crate::runtime::{NetworkPolicy, PermissionMode};
    use crate::state::session::{AgentModels, ResultRecord, SessionState};

    #[test]
    fn completed_step_removes_semantically_matching_pending_entry() {
        let mut session = sample_session();
        session.pending_steps = vec![
            "Run a focused tester pass against the mutated file".to_string(),
            "Inspect the validation output".to_string(),
        ];

        mark_completed_step(
            &mut session,
            "run a focused tester pass against the mutated file.",
        );

        assert_eq!(
            session.pending_steps,
            vec!["Inspect the validation output".to_string()]
        );
        assert_eq!(
            session.completed_steps,
            vec!["run a focused tester pass against the mutated file.".to_string()]
        );
    }

    #[test]
    fn pending_step_is_not_readded_when_recently_completed() {
        let mut session = sample_session();
        session.completed_steps = vec!["Inspect the validation output".to_string()];

        update_pending_steps(
            &mut session,
            "tester",
            Some("inspect the validation output!".to_string()),
        );

        assert!(session.pending_steps.is_empty());
    }

    #[test]
    fn pending_step_replaces_previous_recommendation_from_same_role() {
        let mut session = sample_session();
        session.pending_steps = vec![
            "Use the matched files to decide whether editing or review is needed".to_string(),
            "Run a focused tester pass against the mutated file".to_string(),
        ];
        session.recent_results = vec![
            ResultRecord {
                role: "reader".to_string(),
                model: "pm-model".to_string(),
                summary: "reader summary".to_string(),
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run: Vec::new(),
                next_recommendation: Some(
                    "Use the matched files to decide whether editing or review is needed"
                        .to_string(),
                ),
                findings: Vec::new(),
            },
            ResultRecord {
                role: "reader".to_string(),
                model: "pm-model".to_string(),
                summary: "reader summary 2".to_string(),
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run: Vec::new(),
                next_recommendation: Some("Summarize the matched files before editing".to_string()),
                findings: Vec::new(),
            },
        ];

        update_pending_steps(
            &mut session,
            "reader",
            Some("Summarize the matched files before editing".to_string()),
        );

        assert_eq!(
            session.pending_steps,
            vec![
                "Run a focused tester pass against the mutated file".to_string(),
                "Summarize the matched files before editing".to_string(),
            ]
        );
    }

    fn sample_session() -> SessionState {
        SessionState {
            session_id: "session-1".to_string(),
            pm_model: "pm-model".to_string(),
            permission_mode: PermissionMode::ReadOnly,
            network_policy: NetworkPolicy::Disabled,
            agent_models: AgentModels::default(),
            objective: "objective".to_string(),
            working_summary: String::new(),
            user_preferences_summary: String::new(),
            repository_summary: String::new(),
            active_constraints: Vec::new(),
            open_questions: Vec::new(),
            completed_steps: Vec::new(),
            pending_steps: Vec::new(),
            relevant_files: Vec::new(),
            recent_delegations: Vec::new(),
            recent_results: Vec::new(),
        }
    }
}
