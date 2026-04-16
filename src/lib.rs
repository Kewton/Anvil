pub mod agent;
pub mod cli;
pub mod config;
pub mod git;
pub mod logging;
pub mod mcp;
pub mod model_registry;
pub mod modes;
pub mod ollama;
pub mod safety;
pub mod session;
pub mod skills;
pub mod system_prompt;
pub mod testloop;
pub mod tools;
pub mod tui;
pub mod watch;

use std::io::{self, IsTerminal, Read};

use agent::Agent;
use cli::CliArgs;
use config::Config;
use model_registry::{RuntimeModels, select_models};
use ollama::client::OllamaClient;
use session::store::SessionStore;

pub fn run_cli(args: CliArgs) -> Result<(), String> {
    let config = Config::load(args)?;
    logging::init_logging(config.debug)?;
    config.ensure_state_dirs()?;
    let use_tui = config.tui;

    let client = OllamaClient::new(config.ollama_host.clone())?;
    let available_models = client.list_models()?;
    let models = select_models(
        config.requested_model.clone(),
        config.requested_sidecar_model.clone(),
        &available_models,
        model_registry::detect_total_memory_gib(),
    );

    let session_store = SessionStore::new(config.session_path());
    let session = session_store.load_or_new(config.fresh_session)?;
    let mut agent = Agent::new(config, models, client, session_store, session);

    if let Some(prompt) = agent.initial_prompt_from_cli_or_stdin()? {
        let reply = agent.run_oneshot(&prompt)?;
        println!("{reply}");
        return Ok(());
    }

    if use_tui {
        tui::run(&mut agent)
    } else {
        agent.run_repl()
    }
}

pub fn stdin_prompt() -> Result<Option<String>, String> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("failed to read stdin: {err}"))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

pub fn format_model_banner(models: &RuntimeModels) -> String {
    match &models.sidecar {
        Some(sidecar) => format!("main={} sidecar={sidecar}", models.main),
        None => format!("main={} sidecar=disabled", models.main),
    }
}
