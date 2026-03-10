use anyhow::bail;

use crate::cli::Cli;
use crate::roles::{public_model_override_roles, RoleRegistry};
use crate::state::session::{AgentModels, SessionState};

#[derive(Debug, Clone)]
pub struct EffectiveModels {
    pub pm_model: String,
    pub roles: Vec<EffectiveRoleModel>,
}

#[derive(Debug, Clone)]
pub struct EffectiveRoleModel {
    pub role_id: String,
    pub display_name: String,
    pub model: String,
    pub inherited: bool,
}

impl EffectiveModels {
    pub fn from_cli(cli: &Cli, registry: &RoleRegistry) -> anyhow::Result<Self> {
        Self::from_sources(
            cli,
            registry,
            "qwen-coder-14b".to_string(),
            AgentModels::default(),
        )
    }

    pub fn from_session(
        cli: &Cli,
        registry: &RoleRegistry,
        session: &SessionState,
    ) -> anyhow::Result<Self> {
        Self::from_sources(
            cli,
            registry,
            session.pm_model.clone(),
            session.agent_models.clone(),
        )
    }

    fn from_sources(
        cli: &Cli,
        registry: &RoleRegistry,
        default_pm_model: String,
        stored_models: AgentModels,
    ) -> anyhow::Result<Self> {
        if cli.model.is_some() && cli.pm_model.is_some() {
            bail!("--model and --pm-model must not be used together");
        }

        let pm_model = cli
            .pm_model
            .clone()
            .or_else(|| cli.model.clone())
            .unwrap_or(default_pm_model);

        let mut roles = Vec::new();
        for role in public_model_override_roles(registry) {
            let explicit = match role.id.as_str() {
                "reader" => cli.reader_model.clone(),
                "editor" => cli.editor_model.clone(),
                "tester" => cli.tester_model.clone(),
                "reviewer" => cli.reviewer_model.clone(),
                other => bail!("unsupported public role override: {other}"),
            };
            let stored = match role.id.as_str() {
                "reader" => stored_models.reader.clone(),
                "editor" => stored_models.editor.clone(),
                "tester" => stored_models.tester.clone(),
                "reviewer" => stored_models.reviewer.clone(),
                other => bail!("unsupported persisted public role override: {other}"),
            };
            let model = explicit
                .clone()
                .or(stored)
                .unwrap_or_else(|| pm_model.clone());
            let inherited = explicit.is_none() && model == pm_model;

            roles.push(EffectiveRoleModel {
                role_id: role.id.clone(),
                display_name: role.display_name.clone(),
                model,
                inherited,
            });
        }

        Ok(Self { pm_model, roles })
    }

    pub fn user_facing_roles(&self) -> impl Iterator<Item = (&str, &str, bool)> {
        self.roles.iter().map(|role| {
            (
                role.display_name.as_str(),
                role.model.as_str(),
                role.inherited,
            )
        })
    }

    pub fn agent_models(&self) -> AgentModels {
        let mut models = AgentModels::default();

        for role in &self.roles {
            let target = if role.inherited {
                None
            } else {
                Some(role.model.clone())
            };

            match role.role_id.as_str() {
                "reader" => models.reader = target,
                "editor" => models.editor = target,
                "tester" => models.tester = target,
                "reviewer" => models.reviewer = target,
                _ => {}
            }
        }

        models
    }
}
