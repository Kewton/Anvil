use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

use crate::roles::RoleRegistry;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::util::json::validate_serializable;

pub const DEFAULT_TEXT_LIMIT: usize = 131_072;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub session_id: String,
    pub pm_model: String,
    pub permission_mode: PermissionMode,
    pub network_policy: NetworkPolicy,
    pub agent_models: AgentModels,
    pub objective: String,
    pub working_summary: String,
    pub user_preferences_summary: String,
    pub repository_summary: String,
    pub active_constraints: Vec<String>,
    pub open_questions: Vec<String>,
    pub completed_steps: Vec<String>,
    pub pending_steps: Vec<String>,
    pub relevant_files: Vec<String>,
    pub recent_delegations: Vec<DelegationRecord>,
    pub recent_results: Vec<ResultRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_confirmation: Option<PendingConfirmation>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModels {
    pub reader: Option<String>,
    pub editor: Option<String>,
    pub tester: Option<String>,
    pub reviewer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationRecord {
    pub id: String,
    pub role: String,
    pub resolved_model: String,
    pub inherited_from_pm: bool,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_permission: Option<PermissionMode>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub source_type: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: String,
    pub message: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    pub role: String,
    pub model: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands_run: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_recommendation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingConfirmation {
    pub role: String,
    pub task: String,
    pub summary: String,
    pub reason: String,
    pub action: PendingAction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PendingAction {
    Exec {
        program: String,
        args: Vec<String>,
        cwd: String,
        display: String,
    },
}

impl SessionState {
    pub fn validate(&self, registry: &RoleRegistry) -> anyhow::Result<()> {
        const SCHEMA: &str = include_str!("../../schemas/session-state.schema.json");
        let text_limit = DEFAULT_TEXT_LIMIT;

        ensure_non_empty("session_id", &self.session_id)?;
        ensure_non_empty("pm_model", &self.pm_model)?;
        bounded_len("objective", &self.objective, text_limit)?;
        bounded_len("working_summary", &self.working_summary, text_limit)?;
        bounded_len(
            "user_preferences_summary",
            &self.user_preferences_summary,
            text_limit,
        )?;
        bounded_len("repository_summary", &self.repository_summary, text_limit)?;
        validate_string_list("active_constraints", &self.active_constraints, 20, 300)?;
        validate_string_list("open_questions", &self.open_questions, 10, 300)?;
        validate_string_list("completed_steps", &self.completed_steps, 50, 300)?;
        validate_string_list("pending_steps", &self.pending_steps, 20, 300)?;
        validate_string_list("relevant_files", &self.relevant_files, 200, 400)?;
        self.agent_models.validate(registry)?;
        validate_role_records(registry, &self.recent_delegations, &self.recent_results)?;
        if let Some(pending) = &self.pending_confirmation {
            pending.validate(registry)?;
        }
        validate_serializable("session-state.schema.json", SCHEMA, "SessionState", self)
    }
}

impl AgentModels {
    pub fn validate(&self, registry: &RoleRegistry) -> anyhow::Result<()> {
        for role_id in ["reader", "editor", "tester", "reviewer"] {
            ensure!(
                registry.role(role_id).is_some(),
                "persisted agent model role {role_id} is missing from the role registry"
            );
        }

        for (field, value) in [
            ("reader", &self.reader),
            ("editor", &self.editor),
            ("tester", &self.tester),
            ("reviewer", &self.reviewer),
        ] {
            if let Some(model) = value {
                ensure_non_empty(field, model)?;
            }
        }

        Ok(())
    }
}

impl PendingConfirmation {
    fn validate(&self, registry: &RoleRegistry) -> anyhow::Result<()> {
        let text_limit = DEFAULT_TEXT_LIMIT;
        ensure!(
            registry.role(&self.role).is_some()
                && self.role != "pm"
                && self.role != "planner",
            "pending confirmation role {} is not a persisted subagent role",
            self.role
        );
        ensure_non_empty("pending_confirmation.task", &self.task)?;
        bounded_len("pending_confirmation.task", &self.task, text_limit)?;
        ensure_non_empty("pending_confirmation.summary", &self.summary)?;
        bounded_len("pending_confirmation.summary", &self.summary, text_limit)?;
        ensure_non_empty("pending_confirmation.reason", &self.reason)?;
        bounded_len("pending_confirmation.reason", &self.reason, 300)?;

        match &self.action {
            PendingAction::Exec {
                program,
                args,
                cwd,
                display,
            } => {
                ensure_non_empty("pending_confirmation.exec.program", program)?;
                ensure_non_empty("pending_confirmation.exec.cwd", cwd)?;
                ensure_non_empty("pending_confirmation.exec.display", display)?;
                ensure!(
                    args.len() <= 20,
                    "pending_confirmation.exec.args exceeds maximum size of 20"
                );
                for arg in args {
                    bounded_len("pending_confirmation.exec.args", arg, 300)?;
                }
            }
        }

        Ok(())
    }
}

fn validate_role_records(
    registry: &RoleRegistry,
    recent_delegations: &[DelegationRecord],
    recent_results: &[ResultRecord],
) -> anyhow::Result<()> {
    let text_limit = DEFAULT_TEXT_LIMIT;
    if recent_delegations.len() > 20 {
        bail!("recent_delegations exceeds maximum size of 20");
    }
    if recent_results.len() > 20 {
        bail!("recent_results exceeds maximum size of 20");
    }

    for record in recent_delegations {
        ensure_non_empty("delegation.id", &record.id)?;
        ensure_non_empty("delegation.resolved_model", &record.resolved_model)?;
        bounded_len("delegation.task", &record.task, text_limit)?;
        ensure!(
            registry.role(&record.role).is_some()
                && record.role != "pm"
                && record.role != "planner",
            "delegation role {} is not a persisted subagent role",
            record.role
        );
    }

    for result in recent_results {
        ensure_non_empty("result.model", &result.model)?;
        bounded_len("result.summary", &result.summary, text_limit)?;
        ensure!(
            registry.role(&result.role).is_some()
                && result.role != "pm"
                && result.role != "planner",
            "result role {} is not a persisted subagent role",
            result.role
        );
    }

    Ok(())
}

fn ensure_non_empty(field: &str, value: &str) -> anyhow::Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
}

fn bounded_len(field: &str, value: &str, max: usize) -> anyhow::Result<()> {
    ensure!(
        value.len() <= max,
        "{field} exceeds maximum length of {max}"
    );
    Ok(())
}

fn validate_string_list(
    field: &str,
    values: &[String],
    max_items: usize,
    max_len: usize,
) -> anyhow::Result<()> {
    ensure!(
        values.len() <= max_items,
        "{field} exceeds maximum size of {max_items}"
    );
    for value in values {
        ensure!(!value.is_empty(), "{field} entries must not be empty");
        ensure!(
            value.len() <= max_len,
            "{field} entry exceeds maximum length of {max_len}"
        );
    }
    Ok(())
}
