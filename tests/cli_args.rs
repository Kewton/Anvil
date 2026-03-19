mod common;

use anvil::config::{CliArgs, EffectiveConfig, PromptSource, ReasoningVisibility};
use clap::Parser;
use std::path::PathBuf;

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
    assert!(args.exec.is_none());
    assert!(args.exec_file.is_none());
    assert!(args.reasoning_visibility.is_none());
    assert!(!args.offline);
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
fn cli_args_parse_offline_flag() {
    let args = CliArgs::try_parse_from(["anvil", "--offline"]).unwrap();
    assert!(args.offline);
}

#[test]
fn cli_args_offline_default_is_false() {
    let args = CliArgs::try_parse_from(["anvil"]).unwrap();
    assert!(!args.offline);
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

#[test]
fn apply_cli_args_offline_sets_mode_offline() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    assert!(!config.mode.offline);
    let args = CliArgs {
        offline: true,
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();
    assert!(
        config.mode.offline,
        "offline flag should set mode.offline=true"
    );
}

#[test]
fn apply_cli_args_offline_false_preserves_default() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        offline: false,
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();
    assert!(!config.mode.offline);
}

// --- --exec / --exec-file CLI args parsing tests ---

#[test]
fn cli_args_exec_flag_parse() {
    let args = CliArgs::try_parse_from(["anvil", "--exec", "analyze this code"]).unwrap();
    assert_eq!(args.exec.as_deref(), Some("analyze this code"));
    assert!(args.exec_file.is_none());
    assert!(!args.oneshot);
}

#[test]
fn cli_args_exec_file_flag_parse() {
    let args = CliArgs::try_parse_from(["anvil", "--exec-file", "/tmp/prompt.txt"]).unwrap();
    assert_eq!(args.exec_file, Some(PathBuf::from("/tmp/prompt.txt")));
    assert!(args.exec.is_none());
    assert!(!args.oneshot);
}

#[test]
fn cli_args_exec_oneshot_conflict() {
    let result = CliArgs::try_parse_from(["anvil", "--exec", "hello", "--oneshot"]);
    assert!(result.is_err(), "exec and oneshot should conflict");
}

#[test]
fn cli_args_exec_exec_file_conflict() {
    let result = CliArgs::try_parse_from(["anvil", "--exec", "hello", "--exec-file", "/tmp/p.txt"]);
    assert!(result.is_err(), "exec and exec-file should conflict");
}

#[test]
fn cli_args_exec_file_oneshot_conflict() {
    let result = CliArgs::try_parse_from(["anvil", "--exec-file", "/tmp/p.txt", "--oneshot"]);
    assert!(result.is_err(), "exec-file and oneshot should conflict");
}

// --- PromptSource / apply_cli_args integration tests ---

#[test]
fn prompt_source_default_is_interactive() {
    let config = EffectiveConfig::default_for_test().unwrap();
    assert!(config.mode.prompt_source.is_interactive());
}

#[test]
fn apply_cli_args_exec_sets_non_interactive() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        exec: Some("do something".to_string()),
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();

    assert!(
        !config.mode.interactive,
        "exec should set interactive=false"
    );
    assert!(matches!(config.mode.prompt_source, PromptSource::Exec(_)));
}

#[test]
fn apply_cli_args_exec_sets_approval_false() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    assert!(config.mode.approval_required); // default is true
    let args = CliArgs {
        exec: Some("do something".to_string()),
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();

    assert!(
        !config.mode.approval_required,
        "exec should auto-disable approval"
    );
}

#[test]
fn apply_cli_args_exec_sets_fresh_session() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    assert!(!config.mode.fresh_session); // default is false
    let args = CliArgs {
        exec: Some("do something".to_string()),
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();

    assert!(
        config.mode.fresh_session,
        "exec should auto-set fresh_session=true"
    );
}

#[test]
fn apply_cli_args_exec_file_sets_non_interactive() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        exec_file: Some(PathBuf::from("/tmp/prompt.txt")),
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();

    assert!(!config.mode.interactive);
    assert!(!config.mode.approval_required);
    assert!(config.mode.fresh_session);
    assert!(matches!(
        config.mode.prompt_source,
        PromptSource::ExecFile(_)
    ));
}

#[test]
fn apply_cli_args_oneshot_sets_stdin_source() {
    let mut config = EffectiveConfig::default_for_test().unwrap();
    let args = CliArgs {
        oneshot: true,
        ..CliArgs::default()
    };
    config.apply_cli_args(&args).unwrap();

    assert!(!config.mode.interactive);
    assert!(!config.mode.approval_required);
    assert!(config.mode.fresh_session);
    assert!(matches!(config.mode.prompt_source, PromptSource::Stdin));
}

// --- AppError::exit_code tests ---

#[test]
fn app_error_exit_code_tool_execution() {
    let err = anvil::app::AppError::ToolExecution("test".to_string());
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn app_error_exit_code_config() {
    let err = anvil::app::AppError::Config(anvil::config::ConfigError::ValidationError(
        "test".to_string(),
    ));
    assert_eq!(err.exit_code(), 1);
}

// --- SessionRecord helper tests ---

#[test]
fn session_last_assistant_message_returns_last() {
    use anvil::session::{MessageRole, SessionMessage, SessionRecord};
    use std::path::PathBuf;

    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.push_message(SessionMessage::new(MessageRole::User, "you", "hello"));
    session.push_message(SessionMessage::new(
        MessageRole::Assistant,
        "anvil",
        "first reply",
    ));
    session.push_message(SessionMessage::new(MessageRole::User, "you", "follow up"));
    session.push_message(SessionMessage::new(
        MessageRole::Assistant,
        "anvil",
        "second reply",
    ));

    assert_eq!(session.last_assistant_message(), Some("second reply"));
}

#[test]
fn session_last_assistant_message_returns_none_when_empty() {
    use anvil::session::SessionRecord;
    use std::path::PathBuf;

    let session = SessionRecord::new(PathBuf::from("/tmp/test"));
    assert_eq!(session.last_assistant_message(), None);
}

#[test]
fn session_last_turn_tool_results_returns_tools_after_last_user() {
    use anvil::session::{MessageRole, SessionMessage, SessionRecord};
    use std::path::PathBuf;

    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    // First turn
    session.push_message(SessionMessage::new(MessageRole::User, "you", "first"));
    let mut tool1 = SessionMessage::new(MessageRole::Tool, "tool", "result1");
    tool1.is_error = false;
    session.push_message(tool1);
    // Second turn
    session.push_message(SessionMessage::new(MessageRole::User, "you", "second"));
    let mut tool2 = SessionMessage::new(MessageRole::Tool, "tool", "result2");
    tool2.is_error = true;
    session.push_message(tool2);

    let results: Vec<_> = session.last_turn_tool_results().collect();
    assert_eq!(results.len(), 1, "should only include tools from last turn");
    assert!(results[0].is_error);
}

#[test]
fn session_message_is_error_deserialize_compat() {
    // Verify that old JSON without is_error deserializes with default false
    let json = r#"{
        "id": "test_1",
        "role": "Tool",
        "author": "tool",
        "content": "result",
        "status": "Committed",
        "tool_call_id": null
    }"#;
    let msg: anvil::session::SessionMessage = serde_json::from_str(json).unwrap();
    assert!(!msg.is_error, "is_error should default to false");
}

// --- Spinner no-op test ---

#[test]
fn spinner_noop_in_non_interactive() {
    // Verify that Spinner::start with enabled=false does not panic
    // and can be stopped immediately.
    let spinner = anvil::spinner::Spinner::start("test message", false);
    spinner.stop(); // should be a no-op, no panic
}
