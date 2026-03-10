use clap::{Parser, Subcommand};

use crate::cli::flags::{NetworkPolicyArg, PermissionModeArg};

#[derive(Debug, Parser)]
#[command(name = "anvil", version, about = "Local-first coding agent runtime")]
pub struct Cli {
    #[arg(long, global = true)]
    pub model: Option<String>,
    #[arg(long = "pm-model", global = true)]
    pub pm_model: Option<String>,
    #[arg(long = "reader-model", global = true)]
    pub reader_model: Option<String>,
    #[arg(long = "editor-model", global = true)]
    pub editor_model: Option<String>,
    #[arg(long = "tester-model", global = true)]
    pub tester_model: Option<String>,
    #[arg(long = "reviewer-model", global = true)]
    pub reviewer_model: Option<String>,
    #[arg(long, value_enum, global = true)]
    pub permission_mode: Option<PermissionModeArg>,
    #[arg(long = "network", value_enum, global = true)]
    pub network_policy: Option<NetworkPolicyArg>,
    #[arg(short = 'p', long)]
    pub prompt: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Resume {
        session_id: String,
    },
    Handoff {
        #[command(subcommand)]
        action: HandoffAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum HandoffAction {
    Export { session_id: String },
    Import { file: String },
}
