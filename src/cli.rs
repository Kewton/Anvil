use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Parser)]
#[command(name = "anvil")]
#[command(about = "Local-first coding agent for Ollama and local models")]
pub struct Cli {
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,
    #[arg(long, value_enum, default_value_t = ProviderArg::Ollama)]
    pub provider: ProviderArg,
    #[arg(long, default_value = "qwen3.5:35b")]
    pub model: String,
    #[arg(long, default_value = "http://127.0.0.1:11434")]
    pub host: String,
    #[arg(long, value_enum, default_value_t = PermissionModeArg::Ask)]
    pub permission_mode: PermissionModeArg,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PermissionModeArg {
    Ask,
    AcceptEdits,
    BypassPermissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderArg {
    Ollama,
    LmStudio,
}
