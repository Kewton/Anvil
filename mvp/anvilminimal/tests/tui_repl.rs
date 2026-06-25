use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Parser;

use anvilminimal::cli::Cli;
use anvilminimal::config::{Action, Config, Provider};
use anvilminimal::tui::slash::{parse_slash, parse_words};

fn config(root: PathBuf) -> Config {
    Config {
        workspace_root: root,
        state_dir: PathBuf::from("state"),
        eval_events_path: None,
        yes: true,
        offline: false,
        context_budget: 1000,
        model: "m".to_string(),
        provider: Provider::Ollama,
        planner_model: "pm".to_string(),
        planner_provider: Provider::Gemini,
        ollama_host: "http://localhost:11434".to_string(),
        num_predict: 100,
        max_iterations: 4,
        chat_timeout_secs: 1,
        chat_retries: 1,
        resume: None,
        fresh_session: false,
        no_footer: false,
        profile: "generic".to_string(),
        style: "default".to_string(),
        action: Action::Repl,
    }
}

#[test]
fn tui_non_tty_requires_action() {
    if std::io::stdin().is_terminal() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let err = anvilminimal::repl::run_repl(config(dir.path().to_path_buf()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("stdin is not a TTY"));
}

#[test]
fn slash_command_quotes_profile_style_and_goal() {
    let words = parse_words(r#"/ultra-plan-run --profile nextjs --style compact "3011 app""#);
    assert_eq!(
        words,
        vec![
            "/ultra-plan-run",
            "--profile",
            "nextjs",
            "--style",
            "compact",
            "3011 app"
        ]
    );
}

#[test]
fn cli_repl_action_parity() {
    let dir = tempfile::tempdir().unwrap();
    let base = config(dir.path().to_path_buf());
    let parsed = parse_slash(
        r#"/ultra-plan-run --profile nextjs --style compact "3011 port app""#,
        &base,
    )
    .unwrap();
    let cli = Cli::parse_from([
        "anvilminimal",
        "--provider",
        "ollama",
        "--model",
        "m",
        "--planner-provider",
        "gemini",
        "--planner-model",
        "pm",
        "--ultra-plan-run",
        "--profile",
        "nextjs",
        "--style",
        "compact",
        "3011 port app",
    ]);
    let config = Config::from_cli(cli).unwrap();
    assert_eq!(parsed.command, "/ultra-plan-run");
    assert_eq!(parsed.profile, config.profile);
    assert_eq!(parsed.style, config.style);
    assert!(matches!(config.action, Action::UltraPlanRun(goal) if goal == parsed.goal));
}

#[test]
fn no_footer_flag_reaches_config() {
    let cli = Cli::parse_from(["anvilminimal", "--no-footer"]);
    let config = Config::from_cli(cli).unwrap();
    assert!(config.no_footer);
}
