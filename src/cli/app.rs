use clap::{Parser, Subcommand};

use crate::cli::flags::{NetworkPolicyArg, PermissionModeArg};

#[derive(Debug, Parser)]
#[command(name = "anvil", version, about = "Local-first coding agent runtime")]
pub struct Cli {
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long = "pm-model")]
    pub pm_model: Option<String>,
    #[arg(long = "reader-model")]
    pub reader_model: Option<String>,
    #[arg(long = "editor-model")]
    pub editor_model: Option<String>,
    #[arg(long = "tester-model")]
    pub tester_model: Option<String>,
    #[arg(long = "reviewer-model")]
    pub reviewer_model: Option<String>,
    #[arg(long, value_enum, default_value_t = PermissionModeArg::ReadOnly)]
    pub permission_mode: PermissionModeArg,
    #[arg(long = "network", value_enum, default_value_t = NetworkPolicyArg::Disabled)]
    pub network_policy: NetworkPolicyArg,
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
    Export {
        session_id: String,
    },
    Import {
        file: String,
    },
}
