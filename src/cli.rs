use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::DeterministicFallbackMode;

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
    #[arg(long = "auto-plan")]
    pub auto_plan: bool,
    #[arg(long = "offline")]
    pub offline: bool,
    /// Control deterministic recovery. `hint-only` only nudges the model,
    /// `minimal-patch` writes support files only, and `full-template` preserves
    /// legacy full template recovery. `support-only` and `full` remain aliases.
    #[arg(long = "deterministic-fallback", value_enum)]
    pub deterministic_fallback: Option<DeterministicFallbackMode>,
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
    /// Manage temporary tests generated under
    /// `state_root/sessions/<id>/tmp-tests/` (Issue #458).
    TmpTests {
        #[command(subcommand)]
        action: TmpTestsAction,
    },
    /// Export fine-tuning training data from session logs (Issue #473).
    Export {
        /// Write JSONL output to FILE (default: stdout).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Include only successful turns (build_passed && tests_passed).
        #[arg(long, conflicts_with = "failed_only")]
        success_only: bool,
        /// Include only failed turns.
        #[arg(long, conflicts_with = "success_only")]
        failed_only: bool,
        /// Include sessions from all workspaces (default: current workspace).
        #[arg(long)]
        all: bool,
        /// Export a specific session by id.
        #[arg(long, value_name = "ID")]
        session: Option<String>,
    },
    /// Check Photon rollout readiness conditions (Issue #561).
    PhotonRolloutCheck {},
    /// Promote successful CaseRecord entries to photon seed format
    /// (Issue #593, Phase A — manual CLI + dry-run + JSONL local output).
    /// Exactly one of --session / --case-id / --all is required. --session
    /// is currently UNSUPPORTED in Phase A (CaseRecord does not carry the
    /// originating session id on disk).
    PhotonPromote {
        /// (Phase A: UNSUPPORTED, returns error). Reserved for Phase B
        /// where CaseRecord will carry the originating session id.
        #[arg(long)]
        session: Option<String>,
        /// Promote a single case by id.
        #[arg(long = "case-id")]
        case_id: Option<String>,
        /// Promote every successful CaseRecord in this workspace
        /// (explicit opt-in; required when neither --session nor
        /// --case-id is given).
        #[arg(long)]
        all: bool,
        /// Show what would be promoted without writing log or output file.
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Confirm promotion (required for non-dry-run mode).
        #[arg(long)]
        yes: bool,
        /// Print the full ActionSummary JSON for the matched case(s).
        #[arg(long = "print-summary")]
        print_summary: bool,
        /// Write JSONL output to FILE (0600 perm, new file only).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
}

/// `anvil sessions tmp-tests <list|promote|discard>` (Issue #458).
#[derive(Debug, Clone, Subcommand)]
pub enum TmpTestsAction {
    /// Promote a draft tmp-test into the workspace.
    Promote {
        /// Session id whose tmp-tests to promote from.
        #[arg(long)]
        session: String,
        /// Tmp-test id (`tmp_<sha-prefix>`) to promote.
        #[arg(long = "test-id")]
        test_id: String,
        /// Overwrite an existing workspace file (regular file only; symlinks
        /// are always rejected). NOTE: `--force` is the **collision** override
        /// (allow overwriting a pre-existing file at the destination), not the
        /// approval signal for the promote operation itself — that is `--yes`.
        #[arg(long)]
        force: bool,
        /// Approve the promote operation in non-interactive contexts (CI,
        /// piped stdin). When stdin is a TTY, anvil treats the user as having
        /// approved interactively. Without `--yes` and without a TTY, promote
        /// is rejected so a misconfigured cron / CI job cannot silently write
        /// to the workspace.
        #[arg(long = "yes", short = 'y')]
        yes: bool,
    },
    /// Discard a tmp-test (deletes both body and metadata).
    Discard {
        #[arg(long)]
        session: String,
        #[arg(long = "test-id")]
        test_id: String,
    },
    /// List tmp-tests for a session.
    List {
        #[arg(long)]
        session: String,
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
            auto_plan: false,
            offline: false,
            deterministic_fallback: None,
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
    fn auto_plan_flag_defaults_false_and_parses_as_true() {
        let args = base_args();
        assert!(!args.auto_plan);

        let parsed = CliArgs::parse_from(["anvil", "--auto-plan"]);
        assert!(parsed.auto_plan);
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
