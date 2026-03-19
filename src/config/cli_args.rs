//! CLI argument definitions using `clap` derive macros.
//!
//! [`CliArgs`] is parsed in `main.rs` and passed to
//! [`super::EffectiveConfig::load_with_args`] for configuration override.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Default)]
#[command(name = "anvil", version, about = "Local coding agent powered by LLM")]
pub struct CliArgs {
    /// Provider (ollama|openai)
    #[arg(short = 'p', long)]
    pub provider: Option<String>,

    /// LLM model name
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    /// Provider URL
    #[arg(short = 'u', long = "provider-url")]
    pub provider_url: Option<String>,

    /// Sidecar model name
    #[arg(long = "sidecar-model")]
    pub sidecar_model: Option<String>,

    /// Context window size
    #[arg(long = "context-window")]
    pub context_window: Option<u32>,

    /// Context budget (token count)
    #[arg(long = "context-budget")]
    pub context_budget: Option<u32>,

    /// Max agent iterations
    #[arg(long = "max-iterations")]
    pub max_iterations: Option<usize>,

    /// Disable streaming
    #[arg(long = "no-stream")]
    pub no_stream: bool,

    /// Enable debug logging
    #[arg(long)]
    pub debug: bool,

    /// Skip tool execution approval
    #[arg(long = "no-approval")]
    pub no_approval: bool,

    /// Force new session
    #[arg(long = "fresh-session")]
    pub fresh_session: bool,

    /// Non-interactive mode (read prompt from stdin)
    #[arg(long, conflicts_with_all = ["exec", "exec_file"])]
    pub oneshot: bool,

    /// Execute a single prompt and exit (non-interactive)
    #[arg(long, conflicts_with_all = ["exec_file", "oneshot"])]
    pub exec: Option<String>,

    /// Execute prompt from a file and exit (non-interactive)
    #[arg(long = "exec-file", conflicts_with_all = ["exec", "oneshot"])]
    pub exec_file: Option<PathBuf>,

    /// Reasoning visibility level (hidden|summary)
    #[arg(long = "reasoning-visibility")]
    pub reasoning_visibility: Option<String>,
}
