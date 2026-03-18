mod common;

use anvil::config::{CliArgs, EffectiveConfig, ReasoningVisibility};
use clap::Parser;

// --- CliArgs parsing tests ---

#[test]
fn cli_args_default_has_all_none_and_false() {
    let args = CliArgs::default();
    assert!(args.provider.is_none());
    assert!(args.model.is_none());
    assert!(args.provider_url.is_none());
    assert!(args.sidecar_model.is_none());
    assert!(args.context_window.is_none());
    assert!(args.context_budget.is_none());
    assert!(args.max_iterations.is_none());
    assert!(!args.no_stream);
    assert!(!args.debug);
    assert!(!args.no_approval);
    assert!(!args.fresh_session);
    assert!(!args.oneshot);
    assert!(args.reasoning_visibility.is_none());
}

#[test]
fn cli_args_parse_model_long() {
    let args = CliArgs::try_parse_from(["anvil", "--model", "gpt-4"]).unwrap();
    assert_eq!(args.model.as_deref(), Some("gpt-4"));
}

#[test]
fn cli_args_parse_model_short() {
    let args = CliArgs::try_parse_from(["anvil", "-m", "gpt-4"]).unwrap();
    assert_eq!(args.model.as_deref(), Some("gpt-4"));
}

#[test]
fn cli_args_parse_provider_long() {
    let args = CliArgs::try_parse_from(["anvil", "--provider", "openai"]).unwrap();
    assert_eq!(args.provider.as_deref(), Some("openai"));
}

#[test]
fn cli_args_parse_provider_short() {
    let args = CliArgs::try_parse_from(["anvil", "-p", "openai"]).unwrap();
    assert_eq!(args.provider.as_deref(), Some("openai"));
}

#[test]
fn cli_args_parse_provider_url() {
    let args =
        CliArgs::try_parse_from(["anvil", "--provider-url", "http://localhost:8080"]).unwrap();
    assert_eq!(args.provider_url.as_deref(), Some("http://localhost:8080"));
}

#[test]
fn cli_args_parse_provider_url_short() {
    let args = CliArgs::try_parse_from(["anvil", "-u", "http://localhost:8080"]).unwrap();
    assert_eq!(args.provider_url.as_deref(), Some("http://localhost:8080"));
}

#[test]
fn cli_args_parse_sidecar_model() {
    let args = CliArgs::try_parse_from(["anvil", "--sidecar-model", "llama3"]).unwrap();
    assert_eq!(args.sidecar_model.as_deref(), Some("llama3"));
}

#[test]
fn cli_args_parse_context_window() {
    let args = CliArgs::try_parse_from(["anvil", "--context-window", "128000"]).unwrap();
    assert_eq!(args.context_window, Some(128000));
}

#[test]
fn cli_args_parse_context_budget() {
    let args = CliArgs::try_parse_from(["anvil", "--context-budget", "4096"]).unwrap();
    assert_eq!(args.context_budget, Some(4096));
}

#[test]
fn cli_args_parse_max_iterations() {
    let args = CliArgs::try_parse_from(["anvil", "--max-iterations", "20"]).unwrap();
    assert_eq!(args.max_iterations, Some(20));
}

#[test]
fn cli_args_parse_reasoning_visibility() {
    let args = CliArgs::try_parse_from(["anvil", "--reasoning-visibility", "hidden"]).unwrap();
    assert_eq!(args.reasoning_visibility.as_deref(), Some("hidden"));
}

#[test]
fn cli_args_parse_bool_flags() {
    let args = CliArgs::try_parse_from([
        "anvil",
        "--no-stream",
        "--debug",
        "--no-approval",
        "--fresh-session",
        "--oneshot",
    ])
    .unwrap();
    assert!(args.no_stream);
    assert!(args.debug);
    assert!(args.no_approval);
    assert!(args.fresh_session);
    assert!(args.oneshot);
}

#[test]
fn cli_args_rejects_unknown_argument() {
    let result = CliArgs::try_parse_from(["anvil", "--unknown-flag"]);
    assert!(result.is_err());
}

#[test]
fn cli_args_reasoning_visibility_requires_value() {
    let result = CliArgs::try_parse_from(["anvil", "--reasoning-visibility"]);
    assert!(result.is_err());
}

#[test]
fn cli_args_parse_multiple_options() {
    let args = CliArgs::try_parse_from([
        "anvil",
        "-p",
        "openai",
        "-m",
        "gpt-4",
        "-u",
        "https://api.openai.com",
        "--no-stream",
        "--debug",
    ])
    .unwrap();
    assert_eq!(args.provider.as_deref(), Some("openai"));
    assert_eq!(args.model.as_deref(), Some("gpt-4"));
    assert_eq!(args.provider_url.as_deref(), Some("https://api.openai.com"));
    assert!(args.no_stream);
    assert!(args.debug);
}

// --- apply_cli_args tests ---

#[test]
fn apply_cli_args_default_changes_nothing() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let original_model = config.runtime.model.clone();
    let original_stream = config.runtime.stream;
    let original_interactive = config.mode.interactive;

    config
        .apply_cli_args(&CliArgs::default())
        .expect("default args should apply cleanly");

    assert_eq!(config.runtime.model, original_model);
    assert_eq!(config.runtime.stream, original_stream);
    assert_eq!(config.mode.interactive, original_interactive);
}

#[test]
fn apply_cli_args_overrides_string_fields() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        provider: Some("openai".to_string()),
        model: Some("gpt-4".to_string()),
        provider_url: Some("https://api.openai.com".to_string()),
        sidecar_model: Some("llama3".to_string()),
        ..CliArgs::default()
    };

    config.apply_cli_args(&args).unwrap();

    assert_eq!(config.runtime.provider, "openai");
    assert_eq!(config.runtime.model, "gpt-4");
    assert_eq!(config.runtime.provider_url, "https://api.openai.com");
    assert_eq!(config.runtime.sidecar_model.as_deref(), Some("llama3"));
}

#[test]
fn apply_cli_args_overrides_numeric_fields() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        context_window: Some(128000),
        context_budget: Some(4096),
        max_iterations: Some(25),
        ..CliArgs::default()
    };

    config.apply_cli_args(&args).unwrap();

    assert_eq!(config.runtime.context_window, 128000);
    assert_eq!(config.runtime.context_budget, Some(4096));
    assert_eq!(config.runtime.max_agent_iterations, 25);
}

#[test]
fn apply_cli_args_bool_flag_inversion() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    // Defaults: stream=true, approval_required=true, interactive=true
    assert!(config.runtime.stream);
    assert!(config.mode.approval_required);
    assert!(config.mode.interactive);
    assert!(!config.mode.debug_logging);
    assert!(!config.mode.fresh_session);

    let args = CliArgs {
        no_stream: true,
        no_approval: true,
        oneshot: true,
        debug: true,
        fresh_session: true,
        ..CliArgs::default()
    };

    config.apply_cli_args(&args).unwrap();

    assert!(!config.runtime.stream, "no_stream should set stream=false");
    assert!(
        !config.mode.approval_required,
        "no_approval should set approval_required=false"
    );
    assert!(
        !config.mode.interactive,
        "oneshot should set interactive=false"
    );
    assert!(
        config.mode.debug_logging,
        "debug should set debug_logging=true"
    );
    assert!(
        config.mode.fresh_session,
        "fresh_session should set fresh_session=true"
    );
}

#[test]
fn apply_cli_args_bool_flags_false_changes_nothing() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        no_stream: false,
        no_approval: false,
        oneshot: false,
        debug: false,
        fresh_session: false,
        ..CliArgs::default()
    };

    config.apply_cli_args(&args).unwrap();

    // All defaults should remain
    assert!(config.runtime.stream);
    assert!(config.mode.approval_required);
    assert!(config.mode.interactive);
    assert!(!config.mode.debug_logging);
    assert!(!config.mode.fresh_session);
}

#[test]
fn apply_cli_args_reasoning_visibility() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        reasoning_visibility: Some("hidden".to_string()),
        ..CliArgs::default()
    };

    config.apply_cli_args(&args).unwrap();
    assert_eq!(
        config.mode.reasoning_visibility,
        ReasoningVisibility::Hidden
    );
}

#[test]
fn apply_cli_args_invalid_reasoning_visibility_errors() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        reasoning_visibility: Some("invalid_value".to_string()),
        ..CliArgs::default()
    };

    let result = config.apply_cli_args(&args);
    assert!(result.is_err());
}
