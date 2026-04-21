use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    /// Disable the fixed footer status bar (mode / token usage / log level).
    #[arg(long = "no-footer")]
    pub no_footer: bool,
    /// Resume the most recent workspace session (`--resume`) or a specific
    /// session id (`--resume <ID>`). The empty string sentinel (produced by
    /// clap's `default_missing_value`) means "latest workspace session".
    #[arg(long = "resume", num_args = 0..=1, default_missing_value = "", value_name = "ID")]
    pub resume: Option<String>,
    #[arg(long = "cwd", hide = true)]
    pub cwd: Option<PathBuf>,
    #[arg(long = "state-dir")]
    pub state_dir: Option<PathBuf>,

    /// `anvil sessions ...` subcommands. None → REPL / oneshot (従来動作).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Inspect / clean stored sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionsAction {
    /// List sessions in the current workspace (or all workspaces with --all).
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show a single session's metadata by id.
    Show {
        id: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Delete old session directories. Defaults to dry-run.
    Clean {
        /// Delete this specific session id (mutually exclusive with --older-than / --keep).
        id: Option<String>,
        /// Delete sessions older than N days (by session.json mtime).
        #[arg(long = "older-than", value_name = "DAYS")]
        older_than_days: Option<u32>,
        /// Keep the newest N sessions and delete the rest.
        #[arg(long, value_name = "N")]
        keep: Option<usize>,
        /// Include sessions from other workspaces.
        #[arg(long)]
        all: bool,
        /// Actually delete (required; otherwise dry-run).
        #[arg(long)]
        force: bool,
    },
}

impl CliArgs {
    /// Validate CLI-level mutual-exclusion constraints that clap cannot express
    /// declaratively. Returns `Err` with a user-facing message if violated.
    pub fn validate(&self) -> Result<(), String> {
        let resume_on = self.resume.is_some();
        if resume_on && self.fresh_session {
            return Err("--resume and --fresh-session are mutually exclusive".to_string());
        }
        if resume_on && (self.prompt.is_some() || self.oneshot) {
            return Err("--resume cannot be combined with --prompt / --oneshot".to_string());
        }
        Ok(())
    }
}

/// Describes how the caller asked the agent to pick up a prior session.
/// Converted from the raw `--resume` flag value at the CLI boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ResumeRequest {
    /// No resume requested.
    #[default]
    None,
    /// Resume the most recent workspace session.
    Latest,
    /// Resume an explicitly named session id.
    WithId(String),
}

impl ResumeRequest {
    pub fn from_flag(value: Option<String>) -> Self {
        match value {
            None => Self::None,
            Some(s) if s.is_empty() => Self::Latest,
            Some(s) => Self::WithId(s),
        }
    }

    pub fn is_some(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn explicit_id(&self) -> Option<&str> {
        if let Self::WithId(id) = self {
            Some(id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> CliArgs {
        CliArgs {
            prompt: None,
            model: None,
            sidecar_model: None,
            ollama_host: None,
            context_budget: None,
            max_iterations: None,
            chat_timeout_secs: None,
            chat_retries: None,
            verbose: false,
            trace: false,
            debug: false,
            stream: false,
            yes: false,
            fresh_session: false,
            oneshot: false,
            no_footer: false,
            resume: None,
            cwd: None,
            state_dir: None,
            command: None,
        }
    }

    #[test]
    fn validate_accepts_default() {
        assert!(base_args().validate().is_ok());
    }

    #[test]
    fn validate_rejects_resume_with_fresh_session() {
        let mut args = base_args();
        args.resume = Some(String::new());
        args.fresh_session = true;
        assert!(args.validate().is_err());
    }

    #[test]
    fn validate_rejects_resume_with_prompt() {
        let mut args = base_args();
        args.resume = Some("abc".to_string());
        args.prompt = Some("hello".to_string());
        assert!(args.validate().is_err());
    }

    #[test]
    fn validate_rejects_resume_with_oneshot() {
        let mut args = base_args();
        args.resume = Some(String::new());
        args.oneshot = true;
        assert!(args.validate().is_err());
    }

    #[test]
    fn resume_request_conversion() {
        assert_eq!(ResumeRequest::from_flag(None), ResumeRequest::None);
        assert_eq!(
            ResumeRequest::from_flag(Some(String::new())),
            ResumeRequest::Latest
        );
        assert_eq!(
            ResumeRequest::from_flag(Some("abc".into())),
            ResumeRequest::WithId("abc".into())
        );
    }

    #[test]
    fn no_footer_flag_defaults_false_and_parses_as_true() {
        // Default: not set
        let args = base_args();
        assert!(!args.no_footer);

        // CliArgs::parse_from to verify clap binding (AC14: --help carries the flag).
        let parsed = CliArgs::parse_from(["anvil", "--no-footer"]);
        assert!(parsed.no_footer);
    }

    #[test]
    fn resume_request_helpers() {
        assert!(!ResumeRequest::None.is_some());
        assert!(ResumeRequest::Latest.is_some());
        let r = ResumeRequest::WithId("x".to_string());
        assert_eq!(r.explicit_id(), Some("x"));
        assert!(r.is_some());
    }
}
