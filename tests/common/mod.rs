#![allow(dead_code)]

use anvil::app::App;
use anvil::config::EffectiveConfig;
use anvil::provider::ProviderRuntimeContext;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn build_app() -> App {
    build_app_in(unique_test_dir("app"))
}

pub fn build_app_in(root: PathBuf) -> App {
    let config = build_config_in(root);
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    App::new(config, provider, shutdown_flag).expect("app should initialize")
}

pub fn build_config_in(root: PathBuf) -> EffectiveConfig {
    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    config.paths.cwd = root.clone();
    config.paths.workspace_dir = root.join("workspace");
    config.paths.config_file = root.join(".anvil").join("config");
    config.paths.state_dir = root.join(".anvil").join("state");
    config.paths.session_dir = root.join(".anvil").join("sessions");
    config.paths.session_file = config.paths.session_dir.join("session.json");
    config.paths.logs_dir = root.join(".anvil").join("logs");
    config
}

pub fn unique_test_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    std::env::temp_dir().join(format!("anvil_test_{label}_{nanos}"))
}
