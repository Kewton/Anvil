use anvil::app::App;
use anvil::config::EffectiveConfig;
use anvil::config::ReasoningVisibility;
use anvil::contracts::{AppEvent, RuntimeState};
use anvil::provider::ProviderRuntimeContext;
use anvil::state::{StateMachine, StateTransition};
use anvil::tui::Tui;
use std::collections::HashMap;

#[test]
fn initial_app_snapshot_is_ready() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config, provider);
    let snapshot = app.initial_snapshot().expect("initial snapshot should build");

    assert_eq!(snapshot.state, RuntimeState::Ready);
    assert_eq!(snapshot.last_event, Some(AppEvent::StartupCompleted));
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
    assert_eq!(
        app.state_machine().snapshot().last_event,
        Some(AppEvent::StateChanged)
    );
}

#[test]
fn tui_renders_status_line() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config, provider);
    let tui = Tui::new();

    let rendered = tui.render(&app.initial_snapshot().expect("initial snapshot should build"));

    assert!(rendered.contains("[A] anvil >"));
    assert!(rendered.contains("Ready."));
    assert!(rendered.contains("provider=ollama"));
}

#[test]
fn effective_config_derives_workspace_path() {
    let config = EffectiveConfig::load().expect("config should load");

    assert!(config.paths.workspace_dir.ends_with("workspace"));
    assert!(config.mode.interactive);
    assert!(config.mode.approval_required);
    assert_eq!(config.mode.reasoning_visibility, ReasoningVisibility::Summary);
    assert!(!config.mode.debug_logging);
}

#[test]
fn mock_thinking_snapshot_contains_plan_and_reasoning() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config, provider);

    let snapshot = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    assert_eq!(snapshot.state, RuntimeState::Thinking);
    assert!(snapshot.plan.is_some());
    assert!(!snapshot.reasoning_summary.is_empty());
}

#[test]
fn mock_approval_snapshot_represents_one_tool_call() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config, provider);
    let _ = app.mock_thinking_snapshot().expect("thinking snapshot should build");

    let snapshot = app
        .mock_approval_snapshot()
        .expect("approval snapshot should build");
    let approval = snapshot.approval.expect("approval should exist");

    assert_eq!(snapshot.state, RuntimeState::AwaitingApproval);
    assert_eq!(approval.tool_name, "Write");
    assert_eq!(approval.tool_call_id, "call_001");
}

#[test]
fn mock_interrupted_snapshot_exposes_next_actions() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config, provider);
    let _ = app.mock_thinking_snapshot().expect("thinking snapshot should build");

    let snapshot = app
        .mock_interrupted_snapshot()
        .expect("interrupted snapshot should build");
    let interrupt = snapshot.interrupt.expect("interrupt details should exist");

    assert_eq!(snapshot.state, RuntimeState::Interrupted);
    assert_eq!(interrupt.interrupted_what, "provider turn");
    assert!(!interrupt.next_actions.is_empty());
}

#[test]
fn tui_renders_approval_and_interrupt_sections() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = App::new(config.clone(), provider);
    let tui = Tui::new();

    let _ = app.mock_thinking_snapshot().expect("thinking snapshot should build");
    let approval_rendered = tui.render(
        &app.mock_approval_snapshot()
            .expect("approval snapshot should build"),
    );

    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut interrupted_app = App::new(config, provider);
    let _ = interrupted_app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");
    let interrupted_rendered = tui.render(
        &interrupted_app
            .mock_interrupted_snapshot()
            .expect("interrupted snapshot should build"),
    );

    assert!(approval_rendered.contains("[A] anvil > approval"));
    assert!(approval_rendered.contains("tool : Write"));
    assert!(interrupted_rendered.contains("[A] anvil > interrupted"));
    assert!(interrupted_rendered.contains("next :"));
}

#[test]
fn provider_runtime_context_bootstraps_capabilities() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");

    assert!(provider.capabilities.streaming);
    assert!(provider.capabilities.tool_calling);
}

#[test]
fn startup_events_include_config_and_provider_bootstrap() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let app = App::new(config, provider);

    assert_eq!(
        app.startup_events(),
        [
            AppEvent::ConfigLoaded,
            AppEvent::ProviderBootstrapped,
            AppEvent::StartupCompleted,
        ]
    );
}

#[test]
fn state_machine_allows_interrupt_from_awaiting_approval() {
    let mut machine = StateMachine::new();
    let thinking = anvil::contracts::AppStateSnapshot::new(RuntimeState::Thinking);
    machine
        .transition_to(thinking, StateTransition::StartThinking)
        .expect("ready -> thinking should be valid");

    let approval = anvil::contracts::AppStateSnapshot::new(RuntimeState::AwaitingApproval);
    machine
        .transition_to(approval, StateTransition::RequestApproval)
        .expect("thinking -> approval should be valid");

    let interrupted = anvil::contracts::AppStateSnapshot::new(RuntimeState::Interrupted);
    machine
        .transition_to(interrupted, StateTransition::Interrupt)
        .expect("approval -> interrupted should be valid");
}

#[test]
fn provider_bootstrap_rejects_unknown_backend() {
    let mut config = EffectiveConfig::load().expect("config should load");
    config.runtime.provider = "unknown".to_string();

    let err = ProviderRuntimeContext::bootstrap(&config).expect_err("unknown backend should fail");
    assert!(err.to_string().contains("unsupported provider backend"));
}

#[test]
fn override_precedence_is_file_then_env_then_cli() {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");

    let mut file_values = HashMap::new();
    file_values.insert("model".to_string(), "file-model".to_string());
    file_values.insert("debug".to_string(), "false".to_string());

    let mut env_values = HashMap::new();
    env_values.insert("ANVIL_MODEL".to_string(), "env-model".to_string());
    env_values.insert("ANVIL_DEBUG".to_string(), "true".to_string());

    let mut cli_values = HashMap::new();
    cli_values.insert("ANVIL_MODEL".to_string(), "cli-model".to_string());

    config
        .apply_overrides_for_test(&file_values, &env_values, &cli_values)
        .expect("override application should succeed");

    assert_eq!(config.runtime.model, "cli-model");
    assert!(config.mode.debug_logging);
}
