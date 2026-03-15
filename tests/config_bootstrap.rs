mod common;

use anvil::config::EffectiveConfig;
use anvil::config::ReasoningVisibility;
use anvil::contracts::AppEvent;
use anvil::provider::{ProviderRuntimeContext, build_local_provider_client};
use std::collections::HashMap;

#[test]
fn effective_config_derives_workspace_and_session_paths() {
    let config = EffectiveConfig::load().expect("config should load");

    assert!(config.paths.workspace_dir.ends_with("workspace"));
    assert!(config.paths.session_dir.ends_with("sessions"));
    assert!(
        config
            .paths
            .session_file
            .extension()
            .is_some_and(|ext| ext == "json")
    );
    assert!(
        config
            .paths
            .session_file
            .file_name()
            .is_some_and(|name| name != "current.json")
    );
    assert!(config.mode.interactive);
    assert!(config.mode.approval_required);
    assert!(!config.mode.fresh_session);
    assert_eq!(config.runtime.provider_url, "http://127.0.0.1:11434");
    assert!(config.runtime.stream);
    assert_eq!(
        config.mode.reasoning_visibility,
        ReasoningVisibility::Summary
    );
    assert!(!config.mode.debug_logging);
}

#[test]
fn provider_runtime_context_bootstraps_capabilities() {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");

    assert!(provider.capabilities.streaming);
    assert!(provider.capabilities.tool_calling);
}

#[test]
fn local_provider_client_builds_from_effective_config() {
    let config = EffectiveConfig::load().expect("config should load");

    let client = build_local_provider_client(&config).expect("provider client should build");

    assert!(matches!(
        client,
        anvil::provider::LocalProviderClient::Ollama(_)
    ));
}

#[test]
fn openai_provider_bootstraps_from_config() {
    let mut config = EffectiveConfig::load().expect("config should load");
    config.runtime.provider = "openai".to_string();
    config.runtime.provider_url = "http://localhost:8080".to_string();

    let provider = ProviderRuntimeContext::bootstrap(&config).expect("openai should bootstrap");
    let client = build_local_provider_client(&config).expect("openai client should build");

    assert_eq!(provider.backend, anvil::provider::ProviderBackend::OpenAi);
    assert!(provider.capabilities.streaming);
    assert!(matches!(
        client,
        anvil::provider::LocalProviderClient::OpenAi(_)
    ));
}

#[test]
fn provider_bootstrap_rejects_unknown_backend() {
    let mut config = EffectiveConfig::load().expect("config should load");
    config.runtime.provider = "unknown".to_string();

    let err = ProviderRuntimeContext::bootstrap(&config).expect_err("unknown backend should fail");
    assert!(err.to_string().contains("unsupported provider backend"));
}

#[test]
fn startup_events_include_config_and_provider_bootstrap() {
    let app = common::build_app();

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
fn override_precedence_is_file_then_env_then_cli() {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");

    let mut file_values = HashMap::new();
    file_values.insert("model".to_string(), "file-model".to_string());
    file_values.insert("debug".to_string(), "false".to_string());

    let mut env_values = HashMap::new();
    env_values.insert("ANVIL_MODEL".to_string(), "env-model".to_string());
    env_values.insert("ANVIL_DEBUG".to_string(), "true".to_string());
    env_values.insert("ANVIL_STREAM".to_string(), "true".to_string());

    let mut cli_values = HashMap::new();
    cli_values.insert("ANVIL_MODEL".to_string(), "cli-model".to_string());
    cli_values.insert("ANVIL_STREAM".to_string(), "false".to_string());
    cli_values.insert("ANVIL_FRESH_SESSION".to_string(), "true".to_string());

    config
        .apply_overrides_for_test(&file_values, &env_values, &cli_values)
        .expect("override application should succeed");

    assert_eq!(config.runtime.model, "cli-model");
    assert!(config.mode.debug_logging);
    assert!(!config.runtime.stream);
    assert!(config.mode.fresh_session);
}

#[test]
fn max_agent_iterations_defaults_to_ten() {
    let config = EffectiveConfig::default_for_test().expect("config should load");
    assert_eq!(config.runtime.max_agent_iterations, 10);
}

#[test]
fn max_agent_iterations_configurable_via_map() {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    let mut map = HashMap::new();
    map.insert("ANVIL_MAX_AGENT_ITERATIONS".to_string(), "25".to_string());
    config
        .apply_overrides_for_test(&HashMap::new(), &map, &HashMap::new())
        .expect("should apply");
    assert_eq!(config.runtime.max_agent_iterations, 25);
}

#[test]
fn context_budget_configurable_via_map() {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    assert!(config.runtime.context_budget.is_none());

    let mut map = HashMap::new();
    map.insert("ANVIL_CONTEXT_BUDGET".to_string(), "4096".to_string());
    config
        .apply_overrides_for_test(&HashMap::new(), &map, &HashMap::new())
        .expect("should apply");
    assert_eq!(config.runtime.context_budget, Some(4096));
}

#[test]
fn invalid_numeric_config_value_returns_error() {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    let mut map = HashMap::new();
    map.insert(
        "ANVIL_CONTEXT_WINDOW".to_string(),
        "not_a_number".to_string(),
    );
    let result = config.apply_overrides_for_test(&map, &HashMap::new(), &HashMap::new());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not_a_number"));
}
