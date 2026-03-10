use anyhow::Context;

use crate::cli::app::{Cli, Command, HandoffAction};
use crate::cli::output::render_startup_summary;
use crate::roles::{EffectiveModels, RoleRegistry};

pub fn execute(cli: Cli) -> anyhow::Result<()> {
    let registry = RoleRegistry::load_builtin().context("failed to load builtin role registry")?;
    let models = EffectiveModels::from_cli(&cli, &registry)?;

    match &cli.command {
        Some(Command::Resume { session_id }) => {
            println!("resuming session: {session_id}");
            println!("{}", render_startup_summary(&models, cli.permission_mode, cli.network_policy));
        }
        Some(Command::Handoff { action }) => match action {
            HandoffAction::Export { session_id } => {
                println!("handoff export requested for session: {session_id}");
            }
            HandoffAction::Import { file } => {
                println!("handoff import requested from: {file}");
            }
        },
        None => {
            if let Some(prompt) = &cli.prompt {
                println!("prompt mode");
                println!("prompt: {prompt}");
            } else {
                println!("interactive mode");
            }
            println!("{}", render_startup_summary(&models, cli.permission_mode, cli.network_policy));
        }
    }

    Ok(())
}
