use anyhow::bail;

use crate::cli::Cli;
use crate::roles::{public_model_override_roles, RoleRegistry};

#[derive(Debug, Clone)]
pub struct EffectiveModels {
    pub pm_model: String,
    pub roles: Vec<EffectiveRoleModel>,
}

#[derive(Debug, Clone)]
pub struct EffectiveRoleModel {
    pub role_id: String,
    pub model: String,
    pub inherited: bool,
}

impl EffectiveModels {
    pub fn from_cli(cli: &Cli, registry: &RoleRegistry) -> anyhow::Result<Self> {
        if cli.model.is_some() && cli.pm_model.is_some() {
            bail!("--model and --pm-model must not be used together");
        }

        let pm_model = cli
            .pm_model
            .clone()
            .or_else(|| cli.model.clone())
            .unwrap_or_else(|| "qwen-coder-14b".to_string());

        let mut roles = Vec::new();
        for role in public_model_override_roles(registry) {
            let explicit = match role.id.as_str() {
                "reader" => cli.reader_model.clone(),
                "editor" => cli.editor_model.clone(),
                "tester" => cli.tester_model.clone(),
                "reviewer" => cli.reviewer_model.clone(),
                other => bail!("unsupported public role override: {other}"),
            };

            roles.push(EffectiveRoleModel {
                role_id: role.display_name.clone(),
                model: explicit.clone().unwrap_or_else(|| pm_model.clone()),
                inherited: explicit.is_none(),
            });
        }

        Ok(Self { pm_model, roles })
    }

    pub fn user_facing_roles(&self) -> impl Iterator<Item = (&str, &str, bool)> {
        self.roles
            .iter()
            .map(|role| (role.role_id.as_str(), role.model.as_str(), role.inherited))
    }
}
