use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderArg {
    Ollama,
    Openai,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FooterArg {
    On,
    Off,
}

#[derive(Debug, Parser)]
#[command(name = "anvilminimal")]
#[command(about = "Minimal loop + YAML plan runner MVP")]
#[command(version = crate::build_info::VERSION)]
pub struct Cli {
    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,
    #[arg(long)]
    pub preset: Option<String>,
    #[arg(long)]
    pub context_budget: Option<usize>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long, value_enum)]
    pub provider: Option<ProviderArg>,
    #[arg(long)]
    pub planner_model: Option<String>,
    #[arg(long, value_enum)]
    pub planner_provider: Option<ProviderArg>,
    #[arg(long)]
    pub prompt: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub plan_steps: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub plan_run: bool,
    #[arg(long)]
    pub run_plan: Option<PathBuf>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub ultra_plan: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub ultra_plan_run: bool,
    #[arg(long)]
    pub run_ultra_plan: Option<PathBuf>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub setup_interaction_probe: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "List recent runs for the current workspace")]
    pub runs: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Run the offline presentation UX demo")]
    pub ux_demo: bool,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Run the bounded model behavior probe battery"
    )]
    pub model_probe: bool,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long, default_value = "default")]
    pub style: String,
    #[arg(long)]
    pub resume: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub offline: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Keep presentation narration quiet")]
    pub quiet: bool,
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_host: String,
    #[arg(long, default_value_t = 8_192)]
    pub num_predict: usize,
    #[arg(long, default_value_t = 12)]
    pub max_iterations: usize,
    #[arg(long)]
    pub chat_timeout_secs: Option<u64>,
    #[arg(long, default_value_t = 1)]
    pub chat_retries: usize,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub fresh_session: bool,
    #[arg(
        long,
        value_enum,
        value_name = "on|off",
        help = "Control the fixed TUI footer; off keeps scrollback breadcrumbs only"
    )]
    pub footer: Option<FooterArg>,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "footer",
        help = "Disable the fixed TUI footer"
    )]
    pub no_footer: bool,
    #[arg(long, hide = true)]
    pub completion_contract_json: Option<PathBuf>,
    #[arg(trailing_var_arg = true)]
    pub goal: Vec<String>,
}

impl Cli {
    pub fn trailing_goal(&self) -> Option<String> {
        let joined = self.goal.join(" ");
        let trimmed = joined.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_does_not_include_engine() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("--engine"));
    }

    #[test]
    fn help_includes_no_footer() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--no-footer"));
    }

    #[test]
    fn help_includes_footer_mode() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--footer"));
        assert!(help.contains("on|off"));
    }

    #[test]
    fn help_includes_runs() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--runs"));
    }

    #[test]
    fn help_includes_ux_demo() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--ux-demo"));
    }

    #[test]
    fn help_includes_model_probe() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--model-probe"));
    }

    #[test]
    fn help_includes_quiet() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("--quiet"));
    }

    #[test]
    fn help_hides_completion_contract_json() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("--completion-contract-json"));
    }

    #[test]
    fn version_includes_embedded_build_commit_or_unknown() {
        let version = Cli::command().render_version().to_string();
        assert!(version.contains(env!("CARGO_PKG_VERSION")), "{version}");
        assert!(
            version.contains(crate::build_info::COMMIT) || version.contains("unknown"),
            "{version}"
        );
        assert!(version.contains(crate::build_info::TIMESTAMP), "{version}");
    }

    #[test]
    fn sidecar_model_is_rejected_by_default() {
        let err = Cli::try_parse_from(["anvilminimal", "--sidecar-model", "x"]).unwrap_err();
        assert!(err.to_string().contains("unexpected argument"));
    }

    #[test]
    fn num_predict_defaults_to_source_minimal_budget() {
        let cli = Cli::parse_from(["anvilminimal"]);
        assert_eq!(cli.num_predict, 8_192);
    }

    #[test]
    fn config_preset_fields_are_absent_until_resolved() {
        let cli = Cli::parse_from(["anvilminimal"]);
        assert_eq!(cli.model, None);
        assert_eq!(cli.provider, None);
        assert_eq!(cli.context_budget, None);
    }

    #[test]
    fn chat_timeout_defaults_to_source_config() {
        let cli = Cli::parse_from(["anvilminimal"]);
        assert_eq!(cli.chat_timeout_secs, None);
    }

    #[test]
    fn ultra_plan_run_allows_profile_before_goal() {
        let cli = Cli::try_parse_from([
            "anvilminimal",
            "--provider",
            "ollama",
            "--model",
            "qwen3.6:27b-coding-nvfp4",
            "--planner-provider",
            "gemini",
            "--planner-model",
            "gemini-3.5-flash",
            "--ultra-plan-run",
            "--profile",
            "nextjs",
            "3011 port app",
        ])
        .unwrap();
        assert!(cli.ultra_plan_run);
        assert_eq!(cli.profile.as_deref(), Some("nextjs"));
        assert_eq!(cli.trailing_goal().as_deref(), Some("3011 port app"));
    }

    #[test]
    fn profile_absent_is_distinguishable_from_explicit_generic() {
        let implicit = Cli::parse_from(["anvilminimal"]);
        assert_eq!(implicit.profile, None);
        let explicit = Cli::parse_from(["anvilminimal", "--profile", "generic"]);
        assert_eq!(explicit.profile.as_deref(), Some("generic"));
    }
}
