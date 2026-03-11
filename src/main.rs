use std::path::PathBuf;

use anvil::agent::{Agent, OneShotRequest};
use anvil::cli::{Cli, PermissionModeArg, ProviderArg};
use anvil::config::{AppConfig, ProviderKind};
use anvil::policy::permissions::{
    ExecutionContext, InteractionMode, NonInteractiveBehavior, PermissionMode,
};
use anvil::ui::render::render_result_block;
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let permission_mode = match cli.permission_mode {
        PermissionModeArg::Ask => PermissionMode::Ask,
        PermissionModeArg::AcceptEdits => PermissionMode::AcceptEdits,
        PermissionModeArg::BypassPermissions => PermissionMode::BypassPermissions,
    };
    let provider = match cli.provider {
        ProviderArg::Ollama => ProviderKind::Ollama,
        ProviderArg::LmStudio => ProviderKind::LmStudio,
    };

    let execution_context = if cli.prompt.is_some() {
        ExecutionContext {
            interaction_mode: InteractionMode::NonInteractive,
            non_interactive_ask: NonInteractiveBehavior::Deny,
            non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
            non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
        }
    } else {
        ExecutionContext {
            interaction_mode: InteractionMode::Interactive,
            non_interactive_ask: NonInteractiveBehavior::Deny,
            non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
            non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
        }
    };

    let config = AppConfig {
        cwd,
        provider,
        model: cli.model,
        host: cli.host,
        permission_mode,
        execution_context,
        state_dir: cli
            .state_dir
            .unwrap_or_else(|| PathBuf::from(".anvil/state")),
    };

    let agent = Agent::new(config.clone()).await?;
    if let Some(prompt) = cli.prompt {
        let output = agent
            .run_one_shot(OneShotRequest {
                prompt,
                target_dir: config.cwd.clone(),
            })
            .await?;
        let details = output
            .written_files
            .iter()
            .map(|file| {
                file.strip_prefix(&config.cwd)
                    .map(|relative| format!("./{}", relative.display()))
                    .unwrap_or_else(|_| file.display().to_string())
            })
            .collect::<Vec<_>>();
        println!("{}", render_result_block(&output.final_message, &details));
        return Ok(());
    }

    agent.run_interactive().await
}
