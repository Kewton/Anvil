use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{DeterministicFallbackMode, Engine};

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
    /// Override Ollama num_predict. Defaults to 2048 for legacy and 8192 for minimal.
    #[arg(long = "num-predict")]
    pub num_predict: Option<usize>,
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
    /// Ask the minimal engine to draft a step plan and save it under .anvil/plans/.
    #[arg(long = "plan-steps", value_name = "PROMPT")]
    pub plan_steps: Option<String>,
    /// Draft a step plan, save it, then run it immediately with the minimal engine.
    #[arg(long = "plan-run", value_name = "PROMPT")]
    pub plan_run: Option<String>,
    /// Run a previously saved minimal step plan.
    #[arg(long = "run-plan", value_name = "FILE")]
    pub run_plan: Option<PathBuf>,
    /// Draft a top-level phase plan for repeated minimal plan-runs.
    #[arg(long = "ultra-plan", value_name = "PROMPT")]
    pub ultra_plan: Option<String>,
    /// Draft a top-level phase plan, save it, then run each phase with /plan-run.
    #[arg(long = "ultra-plan-run", value_name = "PROMPT")]
    pub ultra_plan_run: Option<String>,
    /// Run a previously saved minimal ultra phase plan.
    #[arg(long = "run-ultra-plan", value_name = "FILE")]
    pub run_ultra_plan: Option<PathBuf>,
    /// Planning style for --ultra-plan / --ultra-plan-run: default, tdd, or test-hardening.
    #[arg(long = "ultra-style", value_name = "STYLE")]
    pub ultra_style: Option<String>,
    /// Contract/verifier profile for --ultra-plan / --ultra-plan-run.
    #[arg(long = "profile", alias = "ultra-profile", value_name = "PROFILE")]
    pub ultra_profile: Option<String>,
    #[arg(long = "offline")]
    pub offline: bool,
    /// Control deterministic recovery. `hint-only` only nudges the model,
    /// `minimal-patch` writes support files only, and `full-template` preserves
    /// legacy full template recovery. `support-only` and `full` remain aliases.
    #[arg(long = "deterministic-fallback", value_enum)]
    pub deterministic_fallback: Option<DeterministicFallbackMode>,
    #[arg(long = "engine", value_enum)]
    pub engine: Option<Engine>,
    /// Issue #634: experimental opt-in for specialized fallback paths
    /// (FastAPI scaffold / Python CSV / FizzBuzz / fixed arithmetic patch /
    /// qwen3.5 固有 deterministic edit). Default off. Template 系は
    /// `--deterministic-fallback full-template` との AND 条件で発火。
    #[arg(long = "experimental-specialized-fallback")]
    pub experimental_specialized_fallback: Option<bool>,
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
        let step_modes = self.plan_steps.is_some() as u8
            + self.plan_run.is_some() as u8
            + self.run_plan.is_some() as u8
            + self.ultra_plan.is_some() as u8
            + self.ultra_plan_run.is_some() as u8
            + self.run_ultra_plan.is_some() as u8;
        if step_modes > 1 {
            return Err(
                "--plan-steps, --plan-run, --run-plan, --ultra-plan, --ultra-plan-run, and --run-ultra-plan are mutually exclusive".to_string(),
            );
        }
        let planning_mode = self.plan_steps.is_some()
            || self.plan_run.is_some()
            || self.run_plan.is_some()
            || self.ultra_plan.is_some()
            || self.ultra_plan_run.is_some()
            || self.run_ultra_plan.is_some();
        if planning_mode && (self.prompt.is_some() || self.oneshot || resume_on) {
            return Err(
                "--plan-steps / --plan-run / --run-plan / --ultra-plan / --ultra-plan-run / --run-ultra-plan cannot be combined with --prompt, --oneshot, or --resume"
                    .to_string(),
            );
        }
        if self.ultra_style.is_some() && self.ultra_plan.is_none() && self.ultra_plan_run.is_none()
        {
            return Err("--ultra-style requires --ultra-plan or --ultra-plan-run".to_string());
        }
        if self.ultra_profile.is_some()
            && self.ultra_plan.is_none()
            && self.ultra_plan_run.is_none()
        {
            return Err("--profile requires --ultra-plan or --ultra-plan-run".to_string());
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
            num_predict: None,
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
            plan_steps: None,
            plan_run: None,
            run_plan: None,
            ultra_plan: None,
            ultra_plan_run: None,
            run_ultra_plan: None,
            ultra_style: None,
            ultra_profile: None,
            offline: false,
            deterministic_fallback: None,
            engine: None,
            experimental_specialized_fallback: None,
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
    fn validate_rejects_step_plan_conflicts() {
        let mut args = base_args();
        args.plan_steps = Some("build app".to_string());
        args.run_plan = Some(PathBuf::from(".anvil/plans/plan.yaml"));
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.plan_run = Some("build app".to_string());
        args.run_plan = Some(PathBuf::from(".anvil/plans/plan.yaml"));
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.ultra_plan_run = Some("build app".to_string());
        args.plan_run = Some("build app".to_string());
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.plan_steps = Some("build app".to_string());
        args.prompt = Some("hello".to_string());
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.ultra_style = Some("tdd".to_string());
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.ultra_style = Some("tdd".to_string());
        args.ultra_plan_run = Some("build app".to_string());
        assert!(args.validate().is_ok());

        let mut args = base_args();
        args.ultra_profile = Some("data-analysis".to_string());
        assert!(args.validate().is_err());

        let mut args = base_args();
        args.ultra_profile = Some("data-analysis".to_string());
        args.ultra_plan_run = Some("analyze data".to_string());
        assert!(args.validate().is_ok());
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
    fn engine_flag_defaults_none_and_parses_minimal() {
        let args = base_args();
        assert_eq!(args.engine, None);

        let parsed = CliArgs::parse_from(["anvil", "--engine", "minimal"]);
        assert_eq!(parsed.engine, Some(Engine::Minimal));
    }

    #[test]
    fn num_predict_flag_defaults_none_and_parses_value() {
        let args = base_args();
        assert_eq!(args.num_predict, None);

        let parsed = CliArgs::parse_from(["anvil", "--num-predict", "8192"]);
        assert_eq!(parsed.num_predict, Some(8192));
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
