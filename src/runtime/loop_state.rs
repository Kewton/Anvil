use crate::agents::pm::{AgentRole, PmAgent};
use crate::agents::tester::TesterAgent;
use crate::roles::EffectiveModels;
use crate::runtime::engine::RuntimeEngine;
use crate::state::session::{
    DelegationRecord, EvidenceRecord, PendingAction, ResultRecord, SessionState,
};
use crate::tools::exec::ExecRequest;
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
        Self::run_prompt_with_stream(session, models, pm, runtime, context, prompt, None)
    }

    pub fn run_prompt_with_stream(
        session: &mut SessionState,
        models: &EffectiveModels,
        pm: &PmAgent,
        runtime: &RuntimeEngine,
        context: &str,
        prompt: &str,
        on_chunk: Option<&mut dyn FnMut(&str)>,
    ) -> anyhow::Result<String> {
        let outcome = pm.run_turn_with_stream(models, prompt, context, runtime, on_chunk)?;
        session.pending_confirmation = None;

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
            let pending_confirmation = outcome.result.pending_confirmation.clone();
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
            session.pending_confirmation = pending_confirmation;
            update_pending_steps(session, &outcome.result.role, next_recommendation);
        }
        session.working_summary = outcome.user_response.clone();
        mark_completed_step(session, prompt);
        compact_pending_steps(session);

        Ok(outcome.user_response)
    }

    pub fn approve_pending(
        session: &mut SessionState,
        models: &EffectiveModels,
        runtime: &RuntimeEngine,
    ) -> anyhow::Result<Option<String>> {
        let Some(pending) = session.pending_confirmation.clone() else {
            return Ok(None);
        };

        let result = match pending.action {
            PendingAction::Exec {
                program,
                args,
                cwd,
                display,
            } if pending.role == "tester" => TesterAgent.approve_pending(
                runtime,
                &pending.task,
                ExecRequest {
                    program,
                    args,
                    cwd: cwd.into(),
                },
                &display,
            ),
            PendingAction::Exec { .. } => {
                session.pending_confirmation = None;
                return Ok(Some(format!(
                    "Pending confirmation for role {} is not executable yet",
                    pending.role
                )));
            }
        };

        session.pending_confirmation = None;
        session.working_summary = result.summary.clone();
        session.recent_results.push(ResultRecord {
            role: result.role.clone(),
            model: resolved_model_for_role_id(models, &result.role).to_string(),
            summary: result.summary.clone(),
            evidence: result
                .evidence
                .iter()
                .map(|(source_type, value)| EvidenceRecord {
                    source_type: source_type.clone(),
                    value: value.clone(),
                })
                .collect(),
            changed_files: result.changed_files.clone(),
            commands_run: result.commands_run.clone(),
            next_recommendation: result.next_recommendation.clone(),
            findings: Vec::new(),
        });
        trim_tail(&mut session.recent_results, 20);
        update_pending_steps(session, &result.role, result.next_recommendation);
        compact_pending_steps(session);

        Ok(Some(result.summary))
    }

    pub fn deny_pending(session: &mut SessionState) -> Option<String> {
        let pending = session.pending_confirmation.take()?;
        let summary = format!(
            "Declined pending confirmation for {}: {}",
            pending.role, pending.reason
        );
        session.working_summary = summary.clone();
        Some(summary)
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

fn compact_pending_steps(session: &mut SessionState) {
    let completed_tail: Vec<String> = session
        .completed_steps
        .iter()
        .rev()
        .take(10)
        .cloned()
        .collect();
    let mut compacted = Vec::new();

    for step in session.pending_steps.iter().rev() {
        if completed_tail.iter().any(|completed| same_step(completed, step)) {
            continue;
        }
        if compacted.iter().any(|existing: &String| same_step(existing, step)) {
            continue;
        }
        compacted.push(step.clone());
        if compacted.len() == 12 {
            break;
        }
    }

    compacted.sort_by_key(|step| step_sort_key(session, step));
    session.pending_steps = compacted;
}

fn step_sort_key(session: &SessionState, step: &str) -> (usize, usize) {
    let role_priority = latest_recommending_role(session, step)
        .map(role_priority)
        .unwrap_or(usize::MAX);
    let recency = latest_recommendation_index(session, step)
        .map(|index| session.recent_results.len().saturating_sub(index))
        .unwrap_or(usize::MAX);
    (role_priority, recency)
}

fn latest_recommending_role<'a>(session: &'a SessionState, step: &str) -> Option<&'a str> {
    session
        .recent_results
        .iter()
        .rev()
        .find(|result| {
            result
                .next_recommendation
                .as_deref()
                .is_some_and(|next| same_step(next, step))
        })
        .map(|result| result.role.as_str())
}

fn latest_recommendation_index(session: &SessionState, step: &str) -> Option<usize> {
    session.recent_results.iter().rposition(|result| {
        result
            .next_recommendation
            .as_deref()
            .is_some_and(|next| same_step(next, step))
    })
}

fn role_priority(role: &str) -> usize {
    match role {
        "editor" => 0,
        "tester" => 1,
        "reviewer" => 2,
        "reader" => 3,
        _ => 4,
    }
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
    resolved_model_for_role_id(models, role_id)
}

fn resolved_model_for_role_id<'a>(models: &'a EffectiveModels, role_id: &str) -> &'a str {
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
    use super::{compact_pending_steps, mark_completed_step, update_pending_steps};
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

    #[test]
    fn compact_pending_steps_deduplicates_drops_completed_and_sorts_by_role_priority() {
        let mut session = sample_session();
        session.completed_steps = vec!["Run cargo check".to_string()];
        session.pending_steps = vec![
            "Inspect matched files".to_string(),
            "run cargo check".to_string(),
            "Inspect matched files!".to_string(),
            "Review the changed files".to_string(),
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
                next_recommendation: Some("Inspect matched files".to_string()),
                findings: Vec::new(),
            },
            ResultRecord {
                role: "reviewer".to_string(),
                model: "pm-model".to_string(),
                summary: "reviewer summary".to_string(),
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run: Vec::new(),
                next_recommendation: Some("Review the changed files".to_string()),
                findings: Vec::new(),
            },
            ResultRecord {
                role: "tester".to_string(),
                model: "pm-model".to_string(),
                summary: "tester summary".to_string(),
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run: Vec::new(),
                next_recommendation: Some(
                    "Run a focused tester pass against the mutated file".to_string(),
                ),
                findings: Vec::new(),
            },
        ];

        compact_pending_steps(&mut session);

        assert_eq!(
            session.pending_steps,
            vec![
                "Run a focused tester pass against the mutated file".to_string(),
                "Review the changed files".to_string(),
                "Inspect matched files!".to_string(),
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
            pending_confirmation: None,
        }
    }
}
