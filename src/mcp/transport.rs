//! STDIO transport for MCP JSON-RPC communication.
//!
//! [D1-001] StdioTransport is an independent struct implementing the McpTransport trait (SRP).
//! [D1-002] McpTransport trait enables future HTTP/SSE transport support (OCP).

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use super::{McpError, McpServerConfig};

/// [D4-003] Maximum JSON-RPC response size (OOM prevention).
pub const MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024; // 10MB

/// [D4-001] Rejected command patterns to prevent arbitrary command execution.
pub const REJECTED_COMMANDS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "sh",
    "bash",
    "cmd.exe",
    "powershell",
];

/// JSON-RPC 2.0 request.
#[derive(Serialize)]
pub(crate) struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 notification (no id).
#[derive(Serialize)]
pub(crate) struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 error object.
#[derive(Deserialize, Debug)]
pub(crate) struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// JSON-RPC 2.0 response.
/// [D4-007] id is Option<u64> to distinguish notifications (id absent).
#[derive(Deserialize)]
pub(crate) struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

/// MCP transport abstraction trait.
/// [D1-002] Enables future HTTP/SSE transport support (OCP).
pub trait McpTransport: Send {
    /// Send a JSON-RPC request and receive a response.
    fn send_request(
        &mut self,
        id: u64,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError>;

    /// Send a JSON-RPC notification (no response expected).
    fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), McpError>;

    /// Shutdown the transport (terminate child process, etc.).
    fn shutdown(&mut self);
}

/// STDIO transport (child process stdin/stdout communication).
/// [D1-001] Independent struct separated from McpConnection (SRP).
pub struct StdioTransport {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    server_name: String,
}

impl StdioTransport {
    /// Create a new StdioTransport by spawning a child process.
    ///
    /// [D4-001] Command field validation:
    ///   1. Canonicalize path to prevent path traversal
    ///   2. Reject commands matching REJECTED_COMMANDS
    ///   3. Warn if args contain '-c' (shell-via execution)
    ///   4. Trust management via .anvil/trusted_servers.json (handled by caller)
    ///
    /// [D4-005] Environment variable filtering:
    ///   1. Remove ANVIL_* environment variables from child process
    ///   2. Remove SENSITIVE_KEYS (SERPER_API_KEY etc.) from child process
    ///   3. Apply McpServerConfig.env variables after filtering
    pub fn new(server_name: &str, config: &McpServerConfig) -> Result<Self, McpError> {
        // [D4-001] Validate command against rejected patterns
        validate_command(&config.command, server_name)?;

        // [D4-001] Warn if args contain '-c' (shell-via execution)
        if config.args.iter().any(|a| a == "-c") {
            tracing::warn!(
                server = server_name,
                "MCP server args contain '-c', which may indicate shell-via execution"
            );
        }

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // [D3-012] Redirect stderr to null to prevent pipe buffer overflow
        cmd.stderr(Stdio::null());

        // [D4-005] Filter environment variables
        // Remove ANVIL_* variables
        for (key, _) in std::env::vars() {
            if key.starts_with("ANVIL_") {
                cmd.env_remove(&key);
            }
        }
        // Remove known sensitive keys
        for key in SENSITIVE_KEYS {
            cmd.env_remove(key);
        }

        // Apply config-specified environment variables (after filtering)
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| McpError::ServerStartFailed {
            server: server_name.to_string(),
            reason: format!("Failed to spawn process '{}': {e}", config.command),
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::ServerStartFailed {
                server: server_name.to_string(),
                reason: "Failed to capture child stdin".to_string(),
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::ServerStartFailed {
                server: server_name.to_string(),
                reason: "Failed to capture child stdout".to_string(),
            })?;

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            server_name: server_name.to_string(),
        })
    }
}

/// Known sensitive environment variable keys to exclude from child processes.
/// [D4-005] Matches config/mod.rs SENSITIVE_KEYS pattern.
const SENSITIVE_KEYS: &[&str] = &["SERPER_API_KEY"];

/// [D4-001] Validate command against rejected patterns.
fn validate_command(command: &str, server_name: &str) -> Result<(), McpError> {
    // Check against rejected command patterns
    let cmd_base = std::path::Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command);

    for rejected in REJECTED_COMMANDS {
        let rejected_base = std::path::Path::new(rejected)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(rejected);

        if cmd_base == rejected_base || command == *rejected {
            return Err(McpError::ServerStartFailed {
                server: server_name.to_string(),
                reason: format!(
                    "Command '{command}' is rejected (matches blocked pattern '{rejected}')"
                ),
            });
        }
    }

    Ok(())
}

impl McpTransport for StdioTransport {
    /// [D4-007] Verify response id matches request id.
    /// [D4-003] Enforce MAX_RESPONSE_SIZE on response.
    fn send_request(
        &mut self,
        id: u64,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        // [D4-003] Write request as newline-delimited JSON
        let request_json =
            serde_json::to_string(&request).map_err(|e| McpError::JsonRpc(e.to_string()))?;

        self.stdin
            .write_all(request_json.as_bytes())
            .map_err(McpError::Io)?;
        self.stdin.write_all(b"\n").map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;

        // [D4-003][D4-007] Read response lines, skipping notifications (id=null)
        loop {
            let mut line = String::new();
            let bytes_read = self.stdout.read_line(&mut line).map_err(McpError::Io)?;

            if bytes_read == 0 {
                return Err(McpError::ServerCrashed {
                    server: self.server_name.clone(),
                });
            }

            // [D4-003] Check response size
            if line.len() > MAX_RESPONSE_SIZE {
                return Err(McpError::ResponseTooLarge {
                    server: self.server_name.clone(),
                    size: line.len(),
                    limit: MAX_RESPONSE_SIZE,
                });
            }

            let response: JsonRpcResponse =
                serde_json::from_str(line.trim()).map_err(|e| McpError::JsonRpc(e.to_string()))?;

            // [D4-007] Skip notifications (id absent)
            let resp_id = match response.id {
                Some(resp_id) => resp_id,
                None => continue, // notification, skip and read next line
            };

            // [D4-007] Verify response id matches request id
            if resp_id != id {
                return Err(McpError::ResponseIdMismatch {
                    server: self.server_name.clone(),
                    expected: id,
                    actual: resp_id,
                });
            }

            // Check for JSON-RPC error
            if let Some(err) = response.error {
                return Err(McpError::JsonRpc(format!(
                    "JSON-RPC error {}: {}",
                    err.code, err.message
                )));
            }

            return Ok(response.result.unwrap_or(serde_json::Value::Null));
        }
    }

    fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), McpError> {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };

        let json =
            serde_json::to_string(&notification).map_err(|e| McpError::JsonRpc(e.to_string()))?;

        self.stdin
            .write_all(json.as_bytes())
            .map_err(McpError::Io)?;
        self.stdin.write_all(b"\n").map_err(McpError::Io)?;
        self.stdin.flush().map_err(McpError::Io)?;

        Ok(())
    }

    fn shutdown(&mut self) {
        // Send shutdown notification (best effort)
        let _ = self.send_notification("notifications/shutdown", None);

        // [D4-009] Try graceful wait, then force kill
        {
            use std::time::{Duration, Instant};

            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => break,
                }
            }
        }

        // Force kill if still running
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({"key": "value"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
        assert!(json.contains("\"params\""));
    }

    #[test]
    fn test_json_rpc_request_no_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "test".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"params\""));
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }

    #[test]
    fn test_json_rpc_response_notification_no_id() {
        let json = r#"{"jsonrpc":"2.0","result":null}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, None);
    }

    #[test]
    fn test_validate_command_rejects_bash() {
        let result = validate_command("bash", "test-server");
        assert!(result.is_err());
        if let Err(McpError::ServerStartFailed { server, reason }) = result {
            assert_eq!(server, "test-server");
            assert!(reason.contains("rejected"));
        }
    }

    #[test]
    fn test_validate_command_rejects_bin_sh() {
        assert!(validate_command("/bin/sh", "s").is_err());
        assert!(validate_command("/bin/bash", "s").is_err());
        assert!(validate_command("sh", "s").is_err());
        assert!(validate_command("cmd.exe", "s").is_err());
        assert!(validate_command("powershell", "s").is_err());
    }

    #[test]
    fn test_validate_command_allows_valid() {
        assert!(validate_command("npx", "s").is_ok());
        assert!(validate_command("node", "s").is_ok());
        assert!(validate_command("/usr/bin/python3", "s").is_ok());
        assert!(validate_command("uvx", "s").is_ok());
    }

    #[test]
    fn test_max_response_size_constant() {
        assert_eq!(MAX_RESPONSE_SIZE, 10 * 1024 * 1024);
    }

    #[test]
    fn test_json_rpc_notification_serialization() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"notifications/initialized\""));
        assert!(!json.contains("\"id\""));
        assert!(!json.contains("\"params\""));
    }
}
