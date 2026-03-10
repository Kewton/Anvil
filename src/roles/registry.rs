use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::runtime::PermissionMode;
use crate::util::json::validate_embedded_json;

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
    pub default_permission: PermissionMode,
    pub capabilities: Vec<RoleCapability>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoleCapability {
    UserInteraction,
    RepositoryInspection,
    Planning,
    FileEdit,
    CommandExecution,
    Review,
    StateManagement,
}

impl RoleRegistry {
    pub fn load_builtin() -> anyhow::Result<Self> {
        let schema_raw = include_str!("../../schemas/role-registry.schema.json");
        let raw = include_str!("../../schemas/role-registry.json");
        validate_embedded_json(
            "role-registry.schema.json",
            schema_raw,
            "role-registry.json",
            raw,
        )?;
        let registry: Self =
            serde_json::from_str(raw).context("failed to deserialize builtin role registry")?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn role(&self, id: &str) -> Option<&RoleDefinition> {
        self.roles.iter().find(|role| role.id == id)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.roles.is_empty() {
            bail!("role registry must define at least one role");
        }

        let mut ids: Vec<&str> = self.roles.iter().map(|role| role.id.as_str()).collect();
        ids.sort_unstable();
        for window in ids.windows(2) {
            if window[0] == window[1] {
                bail!("role registry contains duplicate role id: {}", window[0]);
            }
        }

        if self.role(&self.default_session_role).is_none() {
            bail!(
                "default session role {} is not present in the role registry",
                self.default_session_role
            );
        }

        Ok(())
    }
}
