use std::path::PathBuf;

use anvil::config::{
    LogLevel, PartialConfig, load_config_file, load_env_config, merge_partial_configs,
    parse_key_value_config,
};

#[test]
fn parses_key_value_config() {
    let map = parse_key_value_config(
        r#"
        model = qwen3:8b
        sidecar-model = qwen3:1.7b
        # comment
        yes_mode = true
        "#,
    );
    assert_eq!(map.get("model").unwrap(), "qwen3:8b");
    assert_eq!(map.get("sidecar_model").unwrap(), "qwen3:1.7b");
    assert_eq!(map.get("yes_mode").unwrap(), "true");
}

#[test]
fn merge_prefers_later_sources() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            model: Some("file-model".into()),
            yes_mode: Some(false),
            chat_timeout_secs: Some(120),
            ..PartialConfig::default()
        },
        PartialConfig {
            model: Some("env-model".into()),
            chat_timeout_secs: Some(240),
            ..PartialConfig::default()
        },
        PartialConfig {
            model: Some("cli-model".into()),
            yes_mode: Some(true),
            chat_retries: Some(3),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(merged.model.as_deref(), Some("cli-model"));
    assert_eq!(merged.yes_mode, Some(true));
    assert_eq!(merged.chat_timeout_secs, Some(240));
    assert_eq!(merged.chat_retries, Some(3));
}

#[test]
fn merge_state_dir_override_prefers_cli_over_env() {
    let env_config = PartialConfig {
        state_dir_override: Some(PathBuf::from("/env/anvil")),
        ..PartialConfig::default()
    };
    let cli_config = PartialConfig {
        state_dir_override: Some(PathBuf::from("/cli/anvil")),
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[env_config, cli_config]);
    assert_eq!(merged.state_dir_override, Some(PathBuf::from("/cli/anvil")));
}

#[test]
fn merge_state_dir_override_falls_back_to_env_when_cli_missing() {
    let env_config = PartialConfig {
        state_dir_override: Some(PathBuf::from("/env/anvil")),
        ..PartialConfig::default()
    };
    let cli_config = PartialConfig::default();
    let merged = merge_partial_configs(&[env_config, cli_config]);
    assert_eq!(merged.state_dir_override, Some(PathBuf::from("/env/anvil")));
}

// --- LogLevel tests ---

#[test]
fn log_level_display_is_lowercase() {
    assert_eq!(LogLevel::Info.to_string(), "info");
    assert_eq!(LogLevel::Verbose.to_string(), "verbose");
    assert_eq!(LogLevel::Trace.to_string(), "trace");
}

#[test]
fn log_level_default_is_info() {
    assert_eq!(LogLevel::default(), LogLevel::Info);
}

#[test]
fn log_level_ordering_info_lt_verbose_lt_trace() {
    assert!(LogLevel::Info < LogLevel::Verbose);
    assert!(LogLevel::Verbose < LogLevel::Trace);
}

#[test]
fn log_level_from_legacy_debug_true_is_trace() {
    assert_eq!(LogLevel::from_legacy_debug(true), LogLevel::Trace);
    assert_eq!(LogLevel::from_legacy_debug(false), LogLevel::Info);
}

#[test]
fn log_level_from_str_case_insensitive() {
    assert_eq!("INFO".parse::<LogLevel>().unwrap(), LogLevel::Info);
    assert_eq!("Verbose".parse::<LogLevel>().unwrap(), LogLevel::Verbose);
    assert_eq!("TRACE".parse::<LogLevel>().unwrap(), LogLevel::Trace);
}

#[test]
fn log_level_from_str_rejects_unknown() {
    assert!("quiet".parse::<LogLevel>().is_err());
    assert!("".parse::<LogLevel>().is_err());
}

// --- env_config / config_file tests ---
//
// env var tests share process state, so we serialize via mutex.

use std::sync::Mutex;
static ENV_GUARD: Mutex<()> = Mutex::new(());

fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
    let _g = ENV_GUARD.lock().unwrap();
    // snapshot
    let prev: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(k, _)| ((*k).to_string(), std::env::var(*k).ok()))
        .collect();
    // apply
    for (k, v) in vars {
        match v {
            Some(val) => unsafe { std::env::set_var(k, val) },
            None => unsafe { std::env::remove_var(k) },
        }
    }
    f();
    // restore
    for (k, v) in &prev {
        match v {
            Some(val) => unsafe { std::env::set_var(k, val) },
            None => unsafe { std::env::remove_var(k) },
        }
    }
}

#[test]
fn env_config_anvil_log_level_verbose_only() {
    with_env(
        &[("ANVIL_LOG_LEVEL", Some("verbose")), ("ANVIL_DEBUG", None)],
        || {
            let mut warnings: Vec<String> = Vec::new();
            let cfg = load_env_config(&mut warnings);
            assert_eq!(cfg.log_level, Some(LogLevel::Verbose));
            assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
        },
    );
}

#[test]
fn env_config_anvil_debug_true_only_maps_to_trace_with_warning() {
    with_env(
        &[("ANVIL_LOG_LEVEL", None), ("ANVIL_DEBUG", Some("true"))],
        || {
            let mut warnings: Vec<String> = Vec::new();
            let cfg = load_env_config(&mut warnings);
            assert_eq!(cfg.log_level, Some(LogLevel::Trace));
            assert!(
                warnings.iter().any(|w| w.contains("ANVIL_DEBUG")),
                "expected deprecation warning for ANVIL_DEBUG: {warnings:?}"
            );
        },
    );
}

#[test]
fn env_config_anvil_log_level_beats_anvil_debug() {
    with_env(
        &[
            ("ANVIL_LOG_LEVEL", Some("info")),
            ("ANVIL_DEBUG", Some("true")),
        ],
        || {
            let mut warnings: Vec<String> = Vec::new();
            let cfg = load_env_config(&mut warnings);
            // ANVIL_LOG_LEVEL wins → Info, no deprecation warning for ANVIL_DEBUG (not consulted)
            assert_eq!(cfg.log_level, Some(LogLevel::Info));
            assert!(
                !warnings.iter().any(|w| w.contains("ANVIL_DEBUG")),
                "unexpected ANVIL_DEBUG warning when ANVIL_LOG_LEVEL is set: {warnings:?}"
            );
        },
    );
}

#[test]
fn env_config_anvil_log_level_uppercase_trace() {
    with_env(
        &[("ANVIL_LOG_LEVEL", Some("TRACE")), ("ANVIL_DEBUG", None)],
        || {
            let mut warnings: Vec<String> = Vec::new();
            let cfg = load_env_config(&mut warnings);
            assert_eq!(cfg.log_level, Some(LogLevel::Trace));
        },
    );
}

#[test]
fn env_config_invalid_log_level_warns_and_falls_back_to_info() {
    with_env(
        &[("ANVIL_LOG_LEVEL", Some("quiet")), ("ANVIL_DEBUG", None)],
        || {
            let mut warnings: Vec<String> = Vec::new();
            let cfg = load_env_config(&mut warnings);
            assert_eq!(cfg.log_level, Some(LogLevel::Info));
            assert!(
                warnings.iter().any(|w| w.contains("ANVIL_LOG_LEVEL")),
                "expected warning for invalid ANVIL_LOG_LEVEL: {warnings:?}"
            );
        },
    );
}

#[test]
fn config_file_log_level_beats_legacy_debug() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "log_level=trace\ndebug=false\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.log_level, Some(LogLevel::Trace));
}

#[test]
fn config_file_invalid_log_level_warns_and_falls_back_to_info() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "log_level=quiet\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.log_level, Some(LogLevel::Info));
    assert!(
        warnings.iter().any(|w| w.contains("log_level")),
        "expected warning for invalid log_level: {warnings:?}"
    );
}

#[test]
fn config_file_legacy_debug_true_maps_to_trace_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "debug=true\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.log_level, Some(LogLevel::Trace));
    assert!(
        warnings.iter().any(|w| w.contains("debug")),
        "expected deprecation warning for legacy debug key: {warnings:?}"
    );
}

#[test]
fn merge_log_level_prefers_later_source() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            log_level: Some(LogLevel::Verbose),
            ..PartialConfig::default()
        },
        PartialConfig {
            log_level: Some(LogLevel::Trace),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(merged.log_level, Some(LogLevel::Trace));
}
