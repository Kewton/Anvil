use serde::{Deserialize, Serialize};

use crate::state::session::AgentModels;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffFile {
    pub format_version: String,
    pub created_by: String,
    pub source: String,
    pub session_id: String,
    pub objective: String,
    pub working_summary: String,
    pub pending_steps: Vec<String>,
    pub relevant_files: Vec<String>,
    pub pm_model: String,
    pub agent_models: AgentModels,
}
