use clap::Parser;

use anvilminimal::cli::Cli;
use anvilminimal::config::{Action, Config};

#[test]
fn ultra_plan_run_cli_shape() {
    let cli = Cli::parse_from([
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
    ]);
    let config = Config::from_cli(cli).unwrap();
    assert!(matches!(config.action, Action::UltraPlanRun(_)));
    assert_eq!(config.profile, "nextjs");
}

#[test]
fn ux_demo_cli_shape() {
    let cli = Cli::parse_from(["anvilminimal", "--ux-demo"]);
    let config = Config::from_cli(cli).unwrap();
    assert!(matches!(config.action, Action::UxDemo));
}
