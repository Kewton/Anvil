use crate::agents::pm::{AgentRole, PmAgent};
use crate::roles::EffectiveModels;
use crate::runtime::engine::RuntimeEngine;
use crate::state::session::{DelegationRecord, ResultRecord, SessionState};
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
            session.recent_results.push(ResultRecord {
                role: outcome.result.role.clone(),
                model: resolved_model_for(models, role).to_string(),
                summary: outcome.result.summary.clone(),
                evidence: Vec::new(),
                changed_files: Vec::new(),
                commands_run,
                next_recommendation: next_recommendation.clone(),
                findings: Vec::new(),
            });
            trim_tail(&mut session.recent_results, 20);
            update_pending_steps(session, next_recommendation);
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

fn update_pending_steps(session: &mut SessionState, next_recommendation: Option<String>) {
    if let Some(step) = next_recommendation {
        session.pending_steps.retain(|existing| existing != &step);
        session.pending_steps.push(step);
        trim_tail(&mut session.pending_steps, 20);
    }
}

fn mark_completed_step(session: &mut SessionState, prompt: &str) {
    let step = prompt.trim();
    if step.is_empty() {
        return;
    }

    session.completed_steps.retain(|existing| existing != step);
    session.completed_steps.push(step.to_string());
    trim_tail(&mut session.completed_steps, 50);
    session.pending_steps.retain(|existing| existing != step);
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
