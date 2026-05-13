use std::path::PathBuf;

use anvil::cli::CliArgs;
use anvil::config::{
    Config, DeterministicFallbackMode, LogLevel, PartialConfig, load_config_file, load_env_config,
    merge_partial_configs, parse_key_value_config,
};

#[test]
fn parses_key_value_config() {
    let map = parse_key_value_config(
        r#"
        model = qwen3:8b
        sidecar-model = qwen3:1.7b
        # comment
        yes_mode = true
        auto_plan = on
        "#,
    );
    assert_eq!(map.get("model").unwrap(), "qwen3:8b");
    assert_eq!(map.get("sidecar_model").unwrap(), "qwen3:1.7b");
    assert_eq!(map.get("yes_mode").unwrap(), "true");
    assert_eq!(map.get("auto_plan").unwrap(), "on");
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
fn merge_auto_plan_prefers_later_sources() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            auto_plan: Some(false),
            ..PartialConfig::default()
        },
        PartialConfig {
            auto_plan: Some(true),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(merged.auto_plan, Some(true));
}

#[test]
fn merge_offline_prefers_later_sources() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            offline: Some(false),
            ..PartialConfig::default()
        },
        PartialConfig {
            offline: Some(true),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(merged.offline, Some(true));
}

#[test]
fn deterministic_fallback_mode_parses_aliases() {
    assert_eq!(
        DeterministicFallbackMode::default(),
        DeterministicFallbackMode::MinimalPatch
    );
    assert_eq!(
        "off".parse::<DeterministicFallbackMode>().unwrap(),
        DeterministicFallbackMode::Off
    );
    assert_eq!(
        "hint-only".parse::<DeterministicFallbackMode>().unwrap(),
        DeterministicFallbackMode::HintOnly
    );
    assert_eq!(
        "support_only".parse::<DeterministicFallbackMode>().unwrap(),
        DeterministicFallbackMode::MinimalPatch
    );
    assert_eq!(
        "minimal-patch"
            .parse::<DeterministicFallbackMode>()
            .unwrap(),
        DeterministicFallbackMode::MinimalPatch
    );
    assert_eq!(
        "enabled".parse::<DeterministicFallbackMode>().unwrap(),
        DeterministicFallbackMode::FullTemplate
    );
    assert_eq!(
        "full-template"
            .parse::<DeterministicFallbackMode>()
            .unwrap(),
        DeterministicFallbackMode::FullTemplate
    );
    assert_eq!(
        DeterministicFallbackMode::MinimalPatch.to_string(),
        "minimal-patch"
    );
    assert!(!DeterministicFallbackMode::HintOnly.allows_support_recovery());
    assert!(!DeterministicFallbackMode::MinimalPatch.allows_template_completion());
    assert!(DeterministicFallbackMode::FullTemplate.allows_template_completion());
}

#[test]
fn merge_deterministic_fallback_prefers_later_sources() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            deterministic_fallback: Some(DeterministicFallbackMode::FullTemplate),
            ..PartialConfig::default()
        },
        PartialConfig {
            deterministic_fallback: Some(DeterministicFallbackMode::MinimalPatch),
            ..PartialConfig::default()
        },
        PartialConfig {
            deterministic_fallback: Some(DeterministicFallbackMode::Off),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(
        merged.deterministic_fallback,
        Some(DeterministicFallbackMode::Off)
    );
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
fn env_config_reads_offline_flag() {
    with_env(&[("ANVIL_OFFLINE", Some("true"))], || {
        let mut warnings: Vec<String> = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.offline, Some(true));
    });
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

// --- footer (issue #430 Phase A) ---

#[test]
fn config_file_footer_false_emits_some_false() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "footer=false\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.footer, Some(false));
}

#[test]
fn config_file_footer_true_emits_none_for_default_to_win() {
    // `footer=true` is "no opinion" relative to the default-true; only
    // explicit disable propagates so CLI/env can still override.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "footer=true\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.footer, None);
}

#[test]
fn config_file_missing_footer_key_is_none() {
    // Backward compatibility (DR3-003): legacy configs without `footer=` key
    // must not flip behavior.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "model=qwen3:8b\n").unwrap();
    let mut warnings: Vec<String> = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.footer, None);
}

#[test]
fn env_anvil_no_footer_nonempty_disables() {
    with_env(&[("ANVIL_NO_FOOTER", Some("1"))], || {
        let mut warnings: Vec<String> = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.footer, Some(false));
    });
}

#[test]
fn env_anvil_no_footer_empty_does_not_disable() {
    // POSIX `NO_COLOR` convention: empty value is treated as unset.
    with_env(&[("ANVIL_NO_FOOTER", Some(""))], || {
        let mut warnings: Vec<String> = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.footer, None);
    });
}

#[test]
fn env_anvil_no_footer_unset_is_none() {
    with_env(&[("ANVIL_NO_FOOTER", None)], || {
        let mut warnings: Vec<String> = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.footer, None);
    });
}

#[test]
fn merge_footer_disable_from_any_source_wins() {
    // AC16: `--no-footer` > `ANVIL_NO_FOOTER` > `.anvil/config footer=false` > default.
    // All disable signals collapse to `Some(false)`; the merged result is
    // `Some(false)` whenever any source disables.
    let file_disable = PartialConfig {
        footer: Some(false),
        ..PartialConfig::default()
    };
    let env_none = PartialConfig::default();
    let cli_none = PartialConfig::default();
    let merged = merge_partial_configs(&[file_disable, env_none, cli_none]);
    assert_eq!(merged.footer, Some(false));

    let file_none = PartialConfig::default();
    let env_disable = PartialConfig {
        footer: Some(false),
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[file_none, env_disable, PartialConfig::default()]);
    assert_eq!(merged.footer, Some(false));

    let cli_disable = PartialConfig {
        footer: Some(false),
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[
        PartialConfig::default(),
        PartialConfig::default(),
        cli_disable,
    ]);
    assert_eq!(merged.footer, Some(false));

    // No disable signal anywhere: footer stays None so the default-true wins.
    let merged = merge_partial_configs(&[
        PartialConfig::default(),
        PartialConfig::default(),
        PartialConfig::default(),
    ]);
    assert_eq!(merged.footer, None);
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

// --- photon config (issue #553) ---

fn minimal_args(cwd: &std::path::Path) -> CliArgs {
    CliArgs {
        cwd: Some(cwd.to_path_buf()),
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
        state_dir: None,
        command: None,
    }
}

const PHOTON_ENV_VARS: &[(&str, Option<&str>)] = &[
    ("ANVIL_PHOTON_ENABLED", None),
    ("ANVIL_PHOTON_URL", None),
    ("ANVIL_PHOTON_SHADOW_MODE", None),
    ("ANVIL_PHOTON_CANARY", None),
    ("ANVIL_PHOTON_TIMEOUT_MS", None),
    ("ANVIL_OFFLINE", None),
    // Issue #583: keep this entry at the tail so existing index-based
    // overrides (PHOTON_ENV_VARS[0..=4]) remain stable.
    ("ANVIL_PHOTON_RESPECT_WARNINGS", None),
];

#[test]
fn photon_disabled_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    with_env(PHOTON_ENV_VARS, || {
        let (cfg, _warnings) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(!cfg.photon_enabled);
        assert_eq!(cfg.photon_canary, 0);
        assert_eq!(cfg.photon_timeout_ms, 200);
        assert_eq!(cfg.photon_url, "http://127.0.0.1:3030");
    });
}

#[test]
fn photon_shadow_mode_default_true() {
    let tmp = tempfile::tempdir().unwrap();
    with_env(PHOTON_ENV_VARS, || {
        let (cfg, _) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(cfg.photon_shadow_mode);
    });
}

#[test]
fn env_photon_enabled_true() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars: Vec<(&str, Option<&str>)> = PHOTON_ENV_VARS.to_vec();
    vars[0] = ("ANVIL_PHOTON_ENABLED", Some("true"));
    with_env(&vars, || {
        let (cfg, warnings) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(cfg.photon_enabled);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    });
}

#[test]
fn env_photon_url_invalid_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars: Vec<(&str, Option<&str>)> = PHOTON_ENV_VARS.to_vec();
    vars[1] = ("ANVIL_PHOTON_URL", Some("not-a-url"));
    with_env(&vars, || {
        let result = Config::load(minimal_args(tmp.path()));
        assert!(result.is_err(), "expected Err for invalid URL");
    });
}

#[test]
fn env_photon_url_external_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars: Vec<(&str, Option<&str>)> = PHOTON_ENV_VARS.to_vec();
    vars[1] = ("ANVIL_PHOTON_URL", Some("http://example.com:3030"));
    with_env(&vars, || {
        let result = Config::load(minimal_args(tmp.path()));
        assert!(result.is_err(), "expected Err for external URL");
        let err = result.unwrap_err();
        assert!(
            err.contains("photon_url"),
            "expected 'photon_url' in error, got: {err}"
        );
    });
}

#[test]
fn env_photon_canary_out_of_range_warns_and_defaults() {
    with_env(&[("ANVIL_PHOTON_CANARY", Some("1001"))], || {
        let mut warnings = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.photon_canary, None, "out-of-range should be None");
        assert!(
            warnings.iter().any(|w| w.contains("ANVIL_PHOTON_CANARY")),
            "expected ANVIL_PHOTON_CANARY warning: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("using default (0)")),
            "expected default value in warning: {warnings:?}"
        );
    });
}

#[test]
fn env_photon_timeout_zero_warns_and_defaults() {
    with_env(&[("ANVIL_PHOTON_TIMEOUT_MS", Some("0"))], || {
        let mut warnings = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.photon_timeout_ms, None);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("ANVIL_PHOTON_TIMEOUT_MS")),
            "expected ANVIL_PHOTON_TIMEOUT_MS warning: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("using default (200)")),
            "expected default value in warning: {warnings:?}"
        );
    });
}

#[test]
fn env_photon_timeout_too_large_warns_and_defaults() {
    with_env(&[("ANVIL_PHOTON_TIMEOUT_MS", Some("60001"))], || {
        let mut warnings = Vec::new();
        let cfg = load_env_config(&mut warnings);
        assert_eq!(cfg.photon_timeout_ms, None);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("ANVIL_PHOTON_TIMEOUT_MS")),
            "expected ANVIL_PHOTON_TIMEOUT_MS warning: {warnings:?}"
        );
    });
}

#[test]
fn config_file_photon_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "photon_enabled=true\n").unwrap();
    let mut warnings = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.photon_enabled, Some(true));
    assert!(warnings.is_empty());
}

#[test]
fn merge_photon_env_overrides_file() {
    let file_config = PartialConfig {
        photon_enabled: Some(false),
        ..PartialConfig::default()
    };
    let env_config = PartialConfig {
        photon_enabled: Some(true),
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[file_config, env_config]);
    assert_eq!(merged.photon_enabled, Some(true));
}

#[test]
fn merge_photon_invalid_higher_priority_leaves_lower() {
    // env has invalid canary (→ None from load_env_config); file has valid 500
    // After merge, the file's valid value should win because env contributed None
    let file_config = PartialConfig {
        photon_canary: Some(500),
        ..PartialConfig::default()
    };
    // simulate env producing None for invalid canary
    let env_config = PartialConfig {
        photon_canary: None,
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[file_config, env_config]);
    assert_eq!(merged.photon_canary, Some(500));
}

#[test]
fn offline_forces_photon_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars: Vec<(&str, Option<&str>)> = PHOTON_ENV_VARS.to_vec();
    vars[0] = ("ANVIL_PHOTON_ENABLED", Some("true"));
    vars.push(("ANVIL_OFFLINE", Some("true")));
    with_env(&vars, || {
        let (cfg, warnings) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(
            !cfg.photon_enabled,
            "offline should force photon_enabled=false"
        );
        assert!(
            warnings.iter().any(|w| w.contains("photon_enabled")),
            "expected offline override warning: {warnings:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Issue #583: photon_respect_warnings (CW-01〜CW-04)
// ---------------------------------------------------------------------------

/// CW-01: default value is `true` when no override is provided.
#[test]
fn cw01_photon_respect_warnings_default_true() {
    let tmp = tempfile::tempdir().unwrap();
    with_env(PHOTON_ENV_VARS, || {
        let (cfg, warnings) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(
            cfg.photon_respect_warnings,
            "photon_respect_warnings must default to true"
        );
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    });
}

/// CW-02: `ANVIL_PHOTON_RESPECT_WARNINGS=false` disables the filter.
#[test]
fn cw02_env_photon_respect_warnings_false() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars: Vec<(&str, Option<&str>)> = PHOTON_ENV_VARS.to_vec();
    // tail position is the respect_warnings slot (see PHOTON_ENV_VARS comment)
    let idx = vars.len() - 1;
    vars[idx] = ("ANVIL_PHOTON_RESPECT_WARNINGS", Some("false"));
    with_env(&vars, || {
        let (cfg, _warnings) = Config::load(minimal_args(tmp.path())).unwrap();
        assert!(
            !cfg.photon_respect_warnings,
            "ANVIL_PHOTON_RESPECT_WARNINGS=false must disable the filter"
        );
    });
}

/// CW-03: `.anvil/config` `photon_respect_warnings=false` is honoured.
#[test]
fn cw03_config_file_photon_respect_warnings_false() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config");
    std::fs::write(&path, "photon_respect_warnings=false\n").unwrap();
    let mut warnings = Vec::new();
    let cfg = load_config_file(&path, &mut warnings).unwrap();
    assert_eq!(cfg.photon_respect_warnings, Some(false));
    assert!(warnings.is_empty());
}

/// CW-04: env overrides config-file when both are present.
#[test]
fn cw04_merge_photon_respect_warnings_env_overrides_file() {
    let file_config = PartialConfig {
        photon_respect_warnings: Some(false),
        ..PartialConfig::default()
    };
    let env_config = PartialConfig {
        photon_respect_warnings: Some(true),
        ..PartialConfig::default()
    };
    let merged = merge_partial_configs(&[file_config, env_config]);
    assert_eq!(merged.photon_respect_warnings, Some(true));
}
