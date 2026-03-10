use anyhow::ensure;
use serde::{Deserialize, Serialize};

use crate::roles::RoleRegistry;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::state::session::{
    AgentModels, EvidenceRecord, Finding, ResultRecord, SessionState, DEFAULT_TEXT_LIMIT,
};
use crate::util::json::validate_serializable;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffFile {
    pub format_version: String,
    pub created_by: String,
    pub source: String,
    pub session_id: String,
    pub objective: String,
    pub working_summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_summary: Option<String>,
    pub pending_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<String>,
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_results: Vec<HandoffResultRecord>,
    pub pm_model: String,
    pub agent_models: AgentModels,
    pub permission_mode: PermissionMode,
    pub network_policy: NetworkPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
}

impl HandoffFile {
    pub fn from_session(session: &SessionState, source: impl Into<String>) -> Self {
        let mut findings = Vec::new();
        for result in &session.recent_results {
            findings.extend(result.findings.iter().cloned());
        }

        Self {
            format_version: "2".to_string(),
            created_by: "anvil".to_string(),
            source: source.into(),
            session_id: session.session_id.clone(),
            objective: session.objective.clone(),
            working_summary: session.working_summary.clone(),
            repository_summary: Some(session.repository_summary.clone()),
            pending_steps: session.pending_steps.clone(),
            completed_steps: session.completed_steps.clone(),
            active_constraints: session.active_constraints.clone(),
            open_questions: session.open_questions.clone(),
            relevant_files: session.relevant_files.clone(),
            recent_findings: findings,
            recent_results: session
                .recent_results
                .iter()
                .cloned()
                .map(HandoffResultRecord::from)
                .collect(),
            pm_model: session.pm_model.clone(),
            agent_models: session.agent_models.clone(),
            permission_mode: session.permission_mode,
            network_policy: session.network_policy,
            exported_at: None,
        }
    }

    pub fn validate(&self, registry: &RoleRegistry) -> anyhow::Result<()> {
        const SCHEMA: &str = include_str!("../../schemas/handoff-file.schema.json");
        let text_limit = DEFAULT_TEXT_LIMIT;

        ensure!(
            self.format_version == "2",
            "handoff format_version must be 2"
        );
        ensure!(
            !self.created_by.trim().is_empty(),
            "created_by must not be empty"
        );
        ensure!(
            matches!(
                self.source.as_str(),
                "anvil-session-export" | "anvil-task-snapshot"
            ),
            "handoff source must be an allowed value"
        );
        ensure!(
            self.objective.len() <= text_limit,
            "handoff objective exceeds maximum length of {text_limit}"
        );
        ensure!(
            self.working_summary.len() <= text_limit,
            "handoff working_summary exceeds maximum length of {text_limit}"
        );
        if let Some(repository_summary) = &self.repository_summary {
            ensure!(
                repository_summary.len() <= text_limit,
                "handoff repository_summary exceeds maximum length of {text_limit}"
            );
        }
        self.agent_models.validate(registry)?;
        validate_serializable("handoff-file.schema.json", SCHEMA, "HandoffFile", self)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffResultRecord {
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
}

impl From<ResultRecord> for HandoffResultRecord {
    fn from(value: ResultRecord) -> Self {
        Self {
            role: value.role,
            model: value.model,
            summary: value.summary,
            evidence: value.evidence,
            changed_files: value.changed_files,
            commands_run: value.commands_run,
            next_recommendation: value.next_recommendation,
        }
    }
}
