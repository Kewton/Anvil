use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;

static LLM_IO_LOGGER: OnceLock<Mutex<File>> = OnceLock::new();
static LLM_IO_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init_logging(debug: bool, workspace_root: &Path) -> Result<(), String> {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|err| format!("failed to initialize logging: {err}"))?;

    if debug {
        let log_dir = workspace_root.join(".anvil").join("logs");
        fs::create_dir_all(&log_dir)
            .map_err(|err| format!("failed to create log dir {}: {err}", log_dir.display()))?;
        let log_path = log_dir.join("llm-io.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|err| format!("failed to open LLM I/O log {}: {err}", log_path.display()))?;
        let _ = LLM_IO_LOG_PATH.set(log_path);
        let _ = LLM_IO_LOGGER.set(Mutex::new(file));
    }

    Ok(())
}

pub fn llm_io_log_path() -> Option<&'static Path> {
    LLM_IO_LOG_PATH.get().map(PathBuf::as_path)
}

pub fn log_llm_event(event: &str, payload: Value) {
    let Some(logger) = LLM_IO_LOGGER.get() else {
        return;
    };

    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let record = json!({
        "ts_ms": ts_ms,
        "event": event,
        "payload": payload,
    });

    if let Ok(mut file) = logger.lock() {
        let _ = writeln!(file, "{record}");
    }
}
