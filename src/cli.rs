use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "anvil")]
#[command(about = "local-first coding agent for Ollama")]
pub struct CliArgs {
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,
    #[arg(long = "sidecar-model")]
    pub sidecar_model: Option<String>,
    #[arg(long = "ollama-host")]
    pub ollama_host: Option<String>,
    #[arg(long = "context-budget")]
    pub context_budget: Option<usize>,
    #[arg(long = "max-iterations")]
    pub max_iterations: Option<usize>,
    #[arg(long = "chat-timeout-secs")]
    pub chat_timeout_secs: Option<u64>,
    #[arg(long = "chat-retries")]
    pub chat_retries: Option<usize>,
    #[arg(long = "verbose")]
    pub verbose: bool,
    #[arg(long = "trace")]
    pub trace: bool,
    /// deprecated alias for `--trace` (hidden)
    #[arg(long = "debug", hide = true)]
    pub debug: bool,
    #[arg(long = "stream")]
    pub stream: bool,
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
    #[arg(long = "fresh-session")]
    pub fresh_session: bool,
    #[arg(long = "oneshot")]
    pub oneshot: bool,
    #[arg(long = "cwd", hide = true)]
    pub cwd: Option<PathBuf>,
    #[arg(long = "state-dir")]
    pub state_dir: Option<PathBuf>,
}
