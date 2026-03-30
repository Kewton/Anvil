//! Structured logging initialization using the `tracing` framework.
//!
//! Provides [`init_tracing`] for subscriber setup and [`LogGuard`] as an
//! opaque wrapper around the file-appender worker guard.

use std::path::Path;
use tracing_subscriber::Layer;

/// Log output format for the file layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable text format (default).
    #[default]
    Text,
    /// Machine-readable JSON format (for `jq` processing).
    Json,
}

/// Opaque wrapper around the file-appender worker guard.
///
/// Prevents callers from depending on `tracing-appender` directly.
/// The inner guard is dropped when this value goes out of scope,
/// flushing any buffered log output.
pub struct LogGuard(#[allow(dead_code)] Option<tracing_appender::non_blocking::WorkerGuard>);

/// Initialise the tracing subscriber.
///
/// Filter resolution:
/// 1. `log_filter` is `Some` -> use that value as the `EnvFilter` directive
/// 2. `debug_logging` is `true` -> use `"debug"`
/// 3. Otherwise -> use `"anvil=info,warn"`
///
/// File layer: always writes to `logs_dir/anvil-{session_id}.log`.
/// Stderr layer: enabled only when `debug_logging` is `true`.
///
/// Returns `Some(LogGuard)` on success, `None` when the log directory
/// cannot be created (graceful degradation).
pub fn init_tracing(
    log_filter: Option<&str>,
    debug_logging: bool,
    logs_dir: &Path,
    session_id: &str,
    log_format: LogFormat,
) -> Option<LogGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let directive = resolve_filter_directive(log_filter, debug_logging);

    let (non_blocking, guard) = build_file_writer(logs_dir, session_id)?;

    let file_filter = tracing_subscriber::EnvFilter::try_new(&directive)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    // File layer: JSON or text format based on log_format (Issue #206 E-1)
    let file_layer: Box<dyn Layer<_> + Send + Sync> = match log_format {
        LogFormat::Text => Box::new(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(file_filter),
        ),
        LogFormat::Json => Box::new(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .json()
                .with_filter(file_filter),
        ),
    };

    let registry = tracing_subscriber::registry().with(file_layer);

    if debug_logging {
        let stderr_filter = tracing_subscriber::EnvFilter::try_new(&directive)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(true)
            .with_filter(stderr_filter);
        let _ = registry.with(stderr_layer).try_init();
    } else {
        let _ = registry.try_init();
    }

    Some(LogGuard(Some(guard)))
}

fn resolve_filter_directive(log_filter: Option<&str>, debug_logging: bool) -> String {
    if let Some(filter) = log_filter {
        filter.to_string()
    } else if debug_logging {
        "debug".to_string()
    } else {
        "anvil=info,warn".to_string()
    }
}

fn build_file_writer(
    logs_dir: &Path,
    session_id: &str,
) -> Option<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    // Create the logs directory with restricted permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        if builder.create(logs_dir).is_err() {
            return None;
        }
    }
    #[cfg(not(unix))]
    {
        if std::fs::create_dir_all(logs_dir).is_err() {
            return None;
        }
    }

    let file_name = format!("anvil-{session_id}.log");
    let file_appender = tracing_appender::rolling::never(logs_dir, &file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Set log file permissions to 0o600 on Unix.
    #[cfg(unix)]
    {
        let log_path = logs_dir.join(&file_name);
        if log_path.exists() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    Some((non_blocking, guard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_filter_directive_default_is_anvil_info_warn() {
        let result = resolve_filter_directive(None, false);
        assert_eq!(result, "anvil=info,warn");
    }

    #[test]
    fn resolve_filter_directive_debug_mode() {
        let result = resolve_filter_directive(None, true);
        assert_eq!(result, "debug");
    }

    #[test]
    fn resolve_filter_directive_explicit_filter() {
        let result = resolve_filter_directive(Some("trace"), false);
        assert_eq!(result, "trace");
    }

    #[test]
    fn resolve_filter_directive_explicit_filter_overrides_debug() {
        let result = resolve_filter_directive(Some("error"), true);
        assert_eq!(result, "error");
    }

    #[test]
    fn log_format_default_is_text() {
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }

    #[test]
    fn log_format_variants_are_distinct() {
        assert_ne!(LogFormat::Text, LogFormat::Json);
    }

    #[test]
    fn log_format_debug_display() {
        assert_eq!(format!("{:?}", LogFormat::Text), "Text");
        assert_eq!(format!("{:?}", LogFormat::Json), "Json");
    }
}
