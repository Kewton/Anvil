use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRegistry {
    pub format_version: String,
    pub default_session_role: String,
    pub roles: Vec<RoleDefinition>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDefinition {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub enabled_in_mvp: bool,
    pub user_facing: bool,
    pub supports_model_override: bool,
    pub default_permission: String,
    pub capabilities: Vec<String>,
}

impl RoleRegistry {
    pub fn load_builtin() -> anyhow::Result<Self> {
        let raw = include_str!("../../schemas/role-registry.json");
        Ok(serde_json::from_str(raw)?)
    }

    pub fn role(&self, id: &str) -> Option<&RoleDefinition> {
        self.roles.iter().find(|role| role.id == id)
    }
}
