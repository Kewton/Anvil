//! Hooks lifecycle hook system.
//!
//! Provides [`HooksEngine`] for executing user-defined hooks at various
//! lifecycle points: PreToolUse, PostToolUse, PreCompact, and PostSession.
//!
//! All hook-related types are self-contained in this module (DR1-008).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum number of hook entries per hook point (DR4-007).
pub const MAX_ENTRIES_PER_HOOK_POINT: usize = 16;

/// Maximum stderr capture size in bytes (DR4-011).
const MAX_STDERR_BYTES: usize = 1024;

// ---------------------------------------------------------------------------
// HookPoint
// ---------------------------------------------------------------------------

/// Lifecycle hook points (DR1-002: HashMap-based extensible structure).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    #[serde(rename = "PreToolUse")]
    PreToolUse,
    #[serde(rename = "PostToolUse")]
    PostToolUse,
    #[serde(rename = "PreCompact")]
    PreCompact,
    #[serde(rename = "PostSession")]
    PostSession,
}

// ---------------------------------------------------------------------------
// HookEntry
// ---------------------------------------------------------------------------

/// Individual hook definition (DR1-013, DR4-002).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub command: String,
    #[serde(default = "default_hook_timeout_ms", alias = "timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_on_timeout")]
    pub on_timeout: String,
}

fn default_hook_timeout_ms() -> u64 {
    5000
}

fn default_on_timeout() -> String {
    "continue".to_string()
}

// ---------------------------------------------------------------------------
// HooksConfig
// ---------------------------------------------------------------------------

/// Hook configuration loaded from hooks.json (DR1-002).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    pub hooks: HashMap<HookPoint, Vec<HookEntry>>,
}

impl HooksConfig {
    /// Returns true if no hook entries are configured (DR1-002).
    pub fn is_empty(&self) -> bool {
        self.hooks.values().all(|entries| entries.is_empty())
    }

    /// Get entries for a specific hook point.
    pub fn get_entries(&self, point: &HookPoint) -> &[HookEntry] {
        self.hooks.get(point).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

// ---------------------------------------------------------------------------
// Event types (DR1-003)
// ---------------------------------------------------------------------------

/// PreToolUse event data sent to hook via stdin.
#[derive(Debug, Clone, Serialize)]
pub struct PreToolUseEvent {
    pub hook_point: &'static str,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_call_id: String,
}

/// PostToolUse event data sent to hook via stdin.
#[derive(Debug, Clone, Serialize)]
pub struct PostToolUseEvent {
    pub hook_point: &'static str,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_call_id: String,
    pub tool_result: HookToolResult,
}

/// Simplified tool result for hooks (DR1-006, DR2-012).
#[derive(Debug, Clone, Serialize)]
pub struct HookToolResult {
    pub status: String,
    pub summary: String,
}

/// PreCompact event data sent to hook via stdin.
#[derive(Debug, Clone, Serialize)]
pub struct PreCompactEvent {
    pub hook_point: &'static str,
    pub session_id: String,
    pub trigger: String,
    pub message_count: usize,
}

/// PostSession event data sent to hook via stdin (DR1-007, DR2-011).
#[derive(Debug, Clone, Serialize)]
pub struct PostSessionEvent {
    pub hook_point: &'static str,
    pub session_id: String,
    pub mode: String,
}

// ---------------------------------------------------------------------------
// Outcome types (DR1-003, DR1-010)
// ---------------------------------------------------------------------------

/// Internal outcome from HookRunner.execute().
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Continue,
    Block { reason: String, exit_code: i32 },
}

/// PreToolUse-specific outcome (DR1-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolUseOutcome {
    Continue,
    Block { reason: String, exit_code: i32 },
}

// ---------------------------------------------------------------------------
// HookError (DR1-009)
// ---------------------------------------------------------------------------

/// Structured hook execution errors.
#[derive(Debug)]
pub enum HookError {
    CommandParseFailed {
        command: String,
        reason: String,
    },
    CommandNotFound {
        command: String,
    },
    PermissionDenied {
        command: String,
        path: String,
    },
    Timeout {
        command: String,
        timeout_ms: u64,
    },
    ExecutionFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    Shutdown,
    ParseError {
        file: PathBuf,
        reason: String,
    },
}

impl Display for HookError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandParseFailed { command, reason } => {
                write!(f, "hook command parse failed: {command}: {reason}")
            }
            Self::CommandNotFound { command } => {
                write!(f, "hook command not found: {command}")
            }
            Self::PermissionDenied { command, path } => {
                write!(f, "hook permission denied: {command} ({path})")
            }
            Self::Timeout {
                command,
                timeout_ms,
            } => {
                write!(f, "hook timed out after {timeout_ms}ms: {command}")
            }
            Self::ExecutionFailed {
                command,
                exit_code,
                stderr,
            } => {
                write!(
                    f,
                    "hook execution failed: {command} (exit_code={exit_code:?}, stderr={stderr})"
                )
            }
            Self::Shutdown => write!(f, "hook execution aborted: shutdown requested"),
            Self::ParseError { file, reason } => {
                write!(f, "hook config parse error: {}: {reason}", file.display())
            }
        }
    }
}

impl std::error::Error for HookError {}

// ---------------------------------------------------------------------------
// HookRunner (DR1-001)
// ---------------------------------------------------------------------------

/// Single hook command execution engine.
///
/// Uses shlex for command parsing and std::process::Command for execution.
/// Supports timeout with 2-stage shutdown (DR4-003).
pub struct HookRunner {
    shutdown_flag: Arc<AtomicBool>,
}

impl HookRunner {
    pub fn new(shutdown_flag: Arc<AtomicBool>) -> Self {
        Self { shutdown_flag }
    }

    /// Validate a command string for safety (DR4-001).
    ///
    /// (a) Parse with shlex::split
    /// (b) Reject paths containing ".."
    /// (c) Resolve relative paths to absolute
    /// (d) Check existence and executable permission
    fn validate_command(&self, command: &str) -> Result<Vec<String>, HookError> {
        let parts = shlex::split(command).ok_or_else(|| HookError::CommandParseFailed {
            command: command.to_string(),
            reason: "invalid shell quoting".to_string(),
        })?;

        if parts.is_empty() {
            return Err(HookError::CommandParseFailed {
                command: command.to_string(),
                reason: "empty command".to_string(),
            });
        }

        let cmd_path = &parts[0];

        // Reject paths containing ".." (path traversal prevention)
        if cmd_path.contains("..") {
            return Err(HookError::CommandNotFound {
                command: command.to_string(),
            });
        }

        // Resolve relative paths
        let resolved = if std::path::Path::new(cmd_path).is_relative() {
            // For relative paths that look like bare commands (no / or .), skip existence check
            // as they may be in PATH
            if !cmd_path.contains('/') {
                return Ok(parts);
            }
            let cwd = std::env::current_dir().unwrap_or_default();
            cwd.join(cmd_path)
        } else {
            PathBuf::from(cmd_path)
        };

        // Check existence
        if !resolved.exists() {
            return Err(HookError::CommandNotFound {
                command: command.to_string(),
            });
        }

        // Check executable permission (Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = resolved.metadata() {
                let permissions = metadata.permissions();
                if permissions.mode() & 0o111 == 0 {
                    return Err(HookError::PermissionDenied {
                        command: command.to_string(),
                        path: resolved.display().to_string(),
                    });
                }
            }
        }

        Ok(parts)
    }

    /// Execute a single hook command (DR4-003, DR4-011, DR4-012).
    ///
    /// - Parses command with shlex
    /// - Sends stdin_data as JSON to child process
    /// - Applies timeout with 2-stage shutdown (SIGTERM -> wait -> SIGKILL)
    /// - Inherits parent environment variables (intentional design)
    pub fn execute(
        &self,
        command: &str,
        stdin_data: &[u8],
        timeout_ms: u64,
        on_timeout: &str,
    ) -> Result<HookOutcome, HookError> {
        // Check shutdown before starting
        if self.shutdown_flag.load(Ordering::Relaxed) {
            return Err(HookError::Shutdown);
        }

        let parts = self.validate_command(command)?;
        let (cmd, args) = parts.split_first().unwrap(); // validated non-empty above

        let mut child = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    HookError::CommandNotFound {
                        command: command.to_string(),
                    }
                } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                    HookError::PermissionDenied {
                        command: command.to_string(),
                        path: cmd.to_string(),
                    }
                } else {
                    HookError::ExecutionFailed {
                        command: command.to_string(),
                        exit_code: None,
                        stderr: e.to_string(),
                    }
                }
            })?;

        // Write stdin data
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(stdin_data);
            // Drop stdin to signal EOF
        }

        // Wait with timeout
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let poll_interval = std::time::Duration::from_millis(50);

        loop {
            // Check shutdown
            if self.shutdown_flag.load(Ordering::Relaxed) {
                self.kill_child(&mut child);
                return Err(HookError::Shutdown);
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process finished
                    return self.process_child_output(command, status, &mut child);
                }
                Ok(None) => {
                    // Still running, check timeout
                    if std::time::Instant::now() >= deadline {
                        // Timeout: 2-stage shutdown (DR4-003)
                        self.kill_child(&mut child);

                        return if on_timeout == "block" {
                            Ok(HookOutcome::Block {
                                reason: format!("hook timed out after {timeout_ms}ms: {command}"),
                                exit_code: -1,
                            })
                        } else {
                            Err(HookError::Timeout {
                                command: command.to_string(),
                                timeout_ms,
                            })
                        };
                    }
                    std::thread::sleep(poll_interval);
                }
                Err(e) => {
                    return Err(HookError::ExecutionFailed {
                        command: command.to_string(),
                        exit_code: None,
                        stderr: e.to_string(),
                    });
                }
            }
        }
    }

    /// Kill a child process with 2-stage shutdown (DR4-003).
    /// SIGTERM -> wait 1s -> SIGKILL -> wait to reap zombie.
    fn kill_child(&self, child: &mut std::process::Child) {
        #[cfg(unix)]
        {
            // Send SIGTERM via the `kill` command (DR4-003).
            let pid = child.id();
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();

            // Wait up to 1 second for graceful shutdown
            let grace_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return, // Process exited
                    Ok(None) => {
                        if std::time::Instant::now() >= grace_deadline {
                            break; // Grace period expired
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }

            // SIGKILL if still running
            let _ = child.kill();
            let _ = child.wait(); // Reap zombie
        }

        #[cfg(not(unix))]
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Process child stdout/stderr and determine outcome.
    fn process_child_output(
        &self,
        command: &str,
        status: std::process::ExitStatus,
        child: &mut std::process::Child,
    ) -> Result<HookOutcome, HookError> {
        // Read stdout (for outcome parsing)
        let stdout = child
            .stdout
            .take()
            .map(|mut s| {
                let mut buf = String::new();
                use std::io::Read;
                let _ = s.read_to_string(&mut buf);
                buf
            })
            .unwrap_or_default();

        // Read stderr (for logging, capped at MAX_STDERR_BYTES)
        let stderr = child
            .stderr
            .take()
            .map(|mut s| {
                let mut buf = vec![0u8; MAX_STDERR_BYTES + 1];
                use std::io::Read;
                let n = s.read(&mut buf).unwrap_or(0);
                buf.truncate(n.min(MAX_STDERR_BYTES));
                String::from_utf8_lossy(&buf).to_string()
            })
            .unwrap_or_default();

        if !stderr.is_empty() {
            tracing::warn!(command, stderr = %stderr, "hook stderr output");
        }

        if status.success() {
            // Try to parse stdout as JSON for block outcome
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout)
                && let Some(decision) = parsed.get("decision").and_then(|d| d.as_str())
                && decision == "block"
            {
                let reason = parsed
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("blocked by hook")
                    .to_string();
                return Ok(HookOutcome::Block {
                    reason,
                    exit_code: 0,
                });
            }
            Ok(HookOutcome::Continue)
        } else {
            let exit_code = status.code();
            // Non-zero exit code with exit_code=2 means explicit block
            if exit_code == Some(2) {
                let reason = if stdout.trim().is_empty() {
                    format!("blocked by hook (exit code 2): {command}")
                } else {
                    stdout.trim().to_string()
                };
                return Ok(HookOutcome::Block {
                    reason,
                    exit_code: 2,
                });
            }
            Err(HookError::ExecutionFailed {
                command: command.to_string(),
                exit_code,
                stderr,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// HooksEngine (DR1-001, DR1-003)
// ---------------------------------------------------------------------------

/// Orchestrates hook execution for all lifecycle points.
pub struct HooksEngine {
    config: HooksConfig,
    runner: HookRunner,
}

impl HooksEngine {
    pub fn new(config: HooksConfig, shutdown_flag: Arc<AtomicBool>) -> Self {
        Self {
            config,
            runner: HookRunner::new(shutdown_flag),
        }
    }

    /// Run PreToolUse hooks (DR1-003).
    ///
    /// Returns Block on first blocking hook. Timeout with on_timeout="block"
    /// also produces a Block outcome (DR4-002).
    pub fn run_pre_tool_use(&self, event: PreToolUseEvent) -> Result<PreToolUseOutcome, HookError> {
        let entries = self.config.get_entries(&HookPoint::PreToolUse);
        let entries = Self::capped_entries(entries);

        let stdin_data = serde_json::to_vec(&event).unwrap_or_default();

        for entry in entries {
            match self.runner.execute(
                &entry.command,
                &stdin_data,
                entry.timeout_ms,
                &entry.on_timeout,
            ) {
                Ok(HookOutcome::Continue) => continue,
                Ok(HookOutcome::Block { reason, exit_code }) => {
                    return Ok(PreToolUseOutcome::Block { reason, exit_code });
                }
                Err(HookError::Timeout {
                    command,
                    timeout_ms,
                }) => {
                    // on_timeout=="continue" path (Timeout error only raised for continue)
                    tracing::warn!(command, timeout_ms, "PreToolUse hook timed out, continuing");
                    continue;
                }
                Err(HookError::Shutdown) => return Err(HookError::Shutdown),
                Err(err) => {
                    // Soft-fail: log and continue
                    tracing::warn!("PreToolUse hook error: {err}");
                    continue;
                }
            }
        }

        Ok(PreToolUseOutcome::Continue)
    }

    /// Run PostToolUse hooks (DR1-003, soft-fail).
    pub fn run_post_tool_use(&self, event: PostToolUseEvent) -> Result<(), HookError> {
        let stdin_data = serde_json::to_vec(&event).unwrap_or_default();
        self.run_soft_fail_hooks(&HookPoint::PostToolUse, &stdin_data, "PostToolUse")
    }

    /// Run PreCompact hooks (DR1-003, soft-fail).
    pub fn run_pre_compact(&self, event: PreCompactEvent) -> Result<(), HookError> {
        let stdin_data = serde_json::to_vec(&event).unwrap_or_default();
        self.run_soft_fail_hooks(&HookPoint::PreCompact, &stdin_data, "PreCompact")
    }

    /// Run PostSession hooks (DR1-003, soft-fail).
    pub fn run_post_session(&self, event: PostSessionEvent) -> Result<(), HookError> {
        let stdin_data = serde_json::to_vec(&event).unwrap_or_default();
        self.run_soft_fail_hooks(&HookPoint::PostSession, &stdin_data, "PostSession")
    }

    /// Run all entries for a hook point with soft-fail semantics.
    ///
    /// Errors are logged but not propagated (soft-fail pattern).
    /// Used by PostToolUse, PreCompact, and PostSession hooks.
    fn run_soft_fail_hooks(
        &self,
        point: &HookPoint,
        stdin_data: &[u8],
        label: &str,
    ) -> Result<(), HookError> {
        let entries = Self::capped_entries(self.config.get_entries(point));

        for entry in entries {
            if let Err(err) = self.runner.execute(
                &entry.command,
                stdin_data,
                entry.timeout_ms,
                &entry.on_timeout,
            ) {
                tracing::warn!("{label} hook error: {err}");
            }
        }
        Ok(())
    }

    /// Cap entries to MAX_ENTRIES_PER_HOOK_POINT (DR4-007).
    fn capped_entries(entries: &[HookEntry]) -> &[HookEntry] {
        if entries.len() > MAX_ENTRIES_PER_HOOK_POINT {
            tracing::warn!(
                count = entries.len(),
                max = MAX_ENTRIES_PER_HOOK_POINT,
                "hook entries exceed limit, using first {} entries",
                MAX_ENTRIES_PER_HOOK_POINT
            );
            &entries[..MAX_ENTRIES_PER_HOOK_POINT]
        } else {
            entries
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_point_serde() {
        let json = r#""PreToolUse""#;
        let point: HookPoint = serde_json::from_str(json).unwrap();
        assert_eq!(point, HookPoint::PreToolUse);

        let json = r#""PostSession""#;
        let point: HookPoint = serde_json::from_str(json).unwrap();
        assert_eq!(point, HookPoint::PostSession);
    }

    #[test]
    fn test_hooks_config_parse_full() {
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    { "command": "/usr/bin/test", "timeout_ms": 3000 }
                ],
                "PostToolUse": [],
                "PreCompact": [],
                "PostSession": [
                    { "command": "/usr/bin/cleanup", "timeout_ms": 10000 }
                ]
            }
        }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert!(!config.is_empty());
        assert_eq!(config.get_entries(&HookPoint::PreToolUse).len(), 1);
        assert_eq!(config.get_entries(&HookPoint::PostToolUse).len(), 0);
        assert_eq!(config.get_entries(&HookPoint::PostSession).len(), 1);
    }

    #[test]
    fn test_hooks_config_empty() {
        let json = r#"{ "hooks": {} }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn test_hooks_config_all_empty_vecs() {
        let json = r#"{ "hooks": { "PreToolUse": [], "PostSession": [] } }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn test_hook_entry_defaults() {
        let json = r#"{ "command": "/usr/bin/test" }"#;
        let entry: HookEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.timeout_ms, 5000);
        assert_eq!(entry.on_timeout, "continue");
    }

    #[test]
    fn test_hook_entry_timeout_alias() {
        let json = r#"{ "command": "/usr/bin/test", "timeout": 3000 }"#;
        let entry: HookEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.timeout_ms, 3000);
    }

    #[test]
    fn test_hook_entry_on_timeout_block() {
        let json = r#"{ "command": "/usr/bin/test", "on_timeout": "block" }"#;
        let entry: HookEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.on_timeout, "block");
    }

    #[test]
    fn test_get_entries_missing_point() {
        let json = r#"{ "hooks": { "PreToolUse": [{ "command": "test" }] } }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.get_entries(&HookPoint::PostSession).len(), 0);
    }

    #[test]
    fn test_hook_error_display() {
        let err = HookError::Timeout {
            command: "test.sh".to_string(),
            timeout_ms: 5000,
        };
        assert!(err.to_string().contains("5000ms"));
        assert!(err.to_string().contains("test.sh"));
    }

    #[test]
    fn test_pre_tool_use_event_serialize() {
        let event = PreToolUseEvent {
            hook_point: "PreToolUse",
            tool_name: "file.read".to_string(),
            tool_input: serde_json::json!({"path": "/tmp/test"}),
            tool_call_id: "call_1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("file.read"));
    }

    #[test]
    fn test_post_session_event_serialize() {
        let event = PostSessionEvent {
            hook_point: "PostSession",
            session_id: "session_123".to_string(),
            mode: "interactive".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("interactive"));
    }

    #[test]
    fn test_pre_tool_use_outcome_variants() {
        let cont = PreToolUseOutcome::Continue;
        assert_eq!(cont, PreToolUseOutcome::Continue);

        let block = PreToolUseOutcome::Block {
            reason: "test".to_string(),
            exit_code: 2,
        };
        if let PreToolUseOutcome::Block { reason, exit_code } = block {
            assert_eq!(reason, "test");
            assert_eq!(exit_code, 2);
        } else {
            panic!("expected Block");
        }
    }

    #[test]
    fn test_capped_entries() {
        let entries: Vec<HookEntry> = (0..20)
            .map(|i| HookEntry {
                command: format!("cmd_{i}"),
                timeout_ms: 5000,
                on_timeout: "continue".to_string(),
            })
            .collect();
        let capped = HooksEngine::capped_entries(&entries);
        assert_eq!(capped.len(), MAX_ENTRIES_PER_HOOK_POINT);
    }

    #[test]
    fn test_capped_entries_within_limit() {
        let entries = vec![HookEntry {
            command: "test".to_string(),
            timeout_ms: 5000,
            on_timeout: "continue".to_string(),
        }];
        let capped = HooksEngine::capped_entries(&entries);
        assert_eq!(capped.len(), 1);
    }
}
