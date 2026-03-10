use serde::{Deserialize, Serialize};

use crate::runtime::{NetworkPolicy, PermissionMode};

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
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModels {
    pub reader: Option<String>,
    pub editor: Option<String>,
    pub tester: Option<String>,
    pub reviewer: Option<String>,
}
