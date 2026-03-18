//! MCP (Model Context Protocol) client implementation.
//!
//! [D3-011] This module only depends on serde_json, std::process, std::io,
//! std::collections, and other standard library / existing external crates.
//! It does NOT import anvil internal modules (tooling, agent, app, config, etc.).

pub mod transport;

use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

pub use transport::{McpTransport, StdioTransport};

/// Default timeout for MCP server operations in seconds.
fn default_timeout() -> u64 {
    30
}

/// MCP server configuration (loaded from .anvil/mcp.json).
/// [D4-002] Debug trait is custom-implemented to REDACT env field values.
#[derive(Deserialize, Clone)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    // Phase 2+ extension point:
    // pub resource_limits: Option<ResourceLimits>,  // max_memory_mb, max_cpu_percent, etc.
}

/// [D4-002] Custom Debug implementation: REDACT env field values, show keys only.
/// Follows the existing RuntimeConfig Debug pattern (config/mod.rs) for api_key REDACTED.
impl fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_env: HashMap<&str, &str> = self
            .env
            .keys()
            .map(|k| (k.as_str(), "[REDACTED]"))
            .collect();
        f.debug_struct("McpServerConfig")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &redacted_env)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// MCP tool information returned by tools/list.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// MCP server connection state.
/// [D1-001] Focuses on MCP protocol layer. Transport details are delegated to Box<dyn McpTransport>.
/// [D1-002] Holds Box<dyn McpTransport> for swappable transport implementations (OCP).
/// [D1-006] Phase 1: no restart_count. request_id is u64 (KISS).
pub struct McpConnection {
    server_name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpToolInfo>,
    request_id: u64,
}

impl McpConnection {
    /// Create a new McpConnection with the given transport.
    pub fn new(server_name: String, transport: Box<dyn McpTransport>) -> Self {
        Self {
            server_name,
            transport,
            tools: Vec::new(),
            request_id: 0,
        }
    }

    /// Get the next request id and increment the counter.
    fn next_id(&mut self) -> u64 {
        self.request_id += 1;
        self.request_id
    }

    /// MCP initialization handshake.
    pub fn initialize(&mut self) -> Result<(), McpError> {
        let id = self.next_id();
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "anvil",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let _result = self
            .transport
            .send_request(id, "initialize", Some(params))
            .map_err(|e| McpError::InitializeFailed {
                server: self.server_name.clone(),
                reason: format!("{e:?}"),
            })?;

        // Send initialized notification
        self.transport
            .send_notification("notifications/initialized", None)
            .map_err(|e| McpError::InitializeFailed {
                server: self.server_name.clone(),
                reason: format!("{e:?}"),
            })?;

        Ok(())
    }

    /// Fetch tool list from the MCP server.
    /// [D4-006] Validates tool names:
    ///   1. Tool name must not be empty
    ///   2. Tool name must not contain control characters (\x00-\x1f) or NUL bytes
    ///   3. Tool name must not contain __ (double underscore)
    ///   4. Invalid tools are skipped with warning log
    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, McpError> {
        let id = self.next_id();
        let result = self.transport.send_request(id, "tools/list", None)?;

        let tools_array = result
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| McpError::JsonRpc("tools/list response missing 'tools' array".into()))?;

        let mut valid_tools = Vec::new();

        for tool_value in tools_array {
            let name = tool_value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");

            // [D4-006] Validate tool name
            if let Err(reason) = validate_tool_name(name) {
                tracing::warn!(
                    server = self.server_name,
                    tool = name,
                    reason = reason,
                    "Skipping MCP tool with invalid name"
                );
                continue;
            }

            let description = tool_value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();

            let input_schema = tool_value
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"}));

            valid_tools.push(McpToolInfo {
                name: name.to_string(),
                description,
                input_schema,
            });
        }

        self.tools = valid_tools.clone();
        Ok(valid_tools)
    }

    /// Call a tool on the MCP server.
    pub fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let id = self.next_id();
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let result = self
            .transport
            .send_request(id, "tools/call", Some(params))
            .map_err(|e| McpError::ToolCallFailed {
                server: self.server_name.clone(),
                tool: tool_name.to_string(),
                reason: format!("{e:?}"),
            })?;

        // Extract text content from MCP tool result
        extract_tool_result_text(&result)
    }

    /// Get the list of tools discovered from this server.
    pub fn get_tools(&self) -> &[McpToolInfo] {
        &self.tools
    }
}

/// Extract text content from MCP tools/call result.
/// MCP tools/call returns content array with type: "text" entries.
fn extract_tool_result_text(result: &serde_json::Value) -> Result<String, McpError> {
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    item.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        Ok(texts.join("\n"))
    } else if let Some(s) = result.as_str() {
        Ok(s.to_string())
    } else {
        Ok(result.to_string())
    }
}

/// [D4-006] Validate MCP tool/server name.
/// Returns Ok(()) if valid, Err(reason) if invalid.
pub fn validate_tool_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("tool name is empty".to_string());
    }

    // Check for control characters
    if name.chars().any(|c| c.is_control()) {
        return Err("tool name contains control characters".to_string());
    }

    // Check for double underscore
    if name.contains("__") {
        return Err("tool name contains '__' (double underscore)".to_string());
    }

    Ok(())
}

/// MCP manager (lifecycle management for all MCP servers).
/// [D1-007] shutdown_flag is managed by App side. McpManager only receives shutdown commands (YAGNI).
/// [D1-009] Only provides get_tools() accessor. Conversion/generation is done by App/tooling/agent side (SRP).
pub struct McpManager {
    connections: HashMap<String, McpConnection>,
}

impl McpManager {
    /// Start all MCP servers from configuration.
    /// [D4-006] Server names are also validated (empty, control chars, __ check).
    /// [D4-001] Trust verification against .anvil/trusted_servers.json is handled by the caller.
    pub fn start_all(configs: HashMap<String, McpServerConfig>) -> Result<Self, McpError> {
        // [D3-008] Warn if server count exceeds recommended limit
        if configs.len() > 10 {
            tracing::warn!(
                count = configs.len(),
                "MCP server count exceeds recommended limit of 10. Startup time may be affected."
            );
        }

        let mut connections = HashMap::new();
        let mut errors = Vec::new();

        // [D4-006] Validate server names
        for server_name in configs.keys() {
            if let Err(reason) = validate_tool_name(server_name) {
                errors.push(format!("Invalid server name '{server_name}': {reason}"));
            }
        }

        if !errors.is_empty() {
            return Err(McpError::ConfigParse(errors.join("; ")));
        }

        // Start servers (sequentially in Phase 1 for simplicity; parallel via
        // std::thread::scope can be added later per design decision #6)
        for (name, config) in &configs {
            match Self::start_server(name, config) {
                Ok(conn) => {
                    connections.insert(name.clone(), conn);
                }
                Err(e) => {
                    tracing::warn!(
                        server = name.as_str(),
                        error = format!("{e:?}"),
                        "Failed to start MCP server, skipping"
                    );
                    // Graceful degradation: skip failed server, continue with others
                }
            }
        }

        Ok(Self { connections })
    }

    /// Start a single MCP server.
    fn start_server(name: &str, config: &McpServerConfig) -> Result<McpConnection, McpError> {
        let transport = StdioTransport::new(name, config)?;
        let mut conn = McpConnection::new(name.to_string(), Box::new(transport));
        conn.initialize()?;
        conn.list_tools()?;
        Ok(conn)
    }

    /// Get all tool information from all connections (read-only accessor).
    /// [D1-009] Caller (App/tooling) performs ToolSpec conversion, ToolRegistry registration,
    /// and system prompt text generation.
    pub fn get_tools(&self) -> HashMap<String, Vec<McpToolInfo>> {
        self.connections
            .iter()
            .map(|(name, conn)| (name.clone(), conn.get_tools().to_vec()))
            .collect()
    }

    /// Execute tools/call on a specific server.
    pub fn call_tool(
        &mut self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let conn = self
            .connections
            .get_mut(server)
            .ok_or_else(|| McpError::ToolCallFailed {
                server: server.to_string(),
                tool: tool.to_string(),
                reason: format!("MCP server '{server}' not found"),
            })?;
        conn.call_tool(tool, arguments)
    }

    /// Shutdown all MCP servers gracefully.
    /// [D4-009] Shutdown procedure:
    ///   1. Send JSON-RPC notification (notifications/shutdown) to each server
    ///   2. Send SIGTERM, wait up to 5 seconds
    ///   3. Force kill with SIGKILL if not terminated within 5 seconds
    pub fn shutdown_all(&mut self) {
        for (name, conn) in &mut self.connections {
            tracing::info!(server = name.as_str(), "Shutting down MCP server");
            conn.transport.shutdown();
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// MCP error type.
/// [D4-002] Display/Debug implementations never include env values. Only env key names.
#[derive(Debug)]
pub enum McpError {
    /// Configuration parsing error.
    ConfigParse(String),
    /// Failed to start MCP server.
    ServerStartFailed { server: String, reason: String },
    /// MCP initialization handshake failed.
    InitializeFailed { server: String, reason: String },
    /// MCP tool call failed.
    ToolCallFailed {
        server: String,
        tool: String,
        reason: String,
    },
    /// Server operation timed out.
    Timeout { server: String, timeout_secs: u64 },
    /// MCP server process crashed.
    ServerCrashed { server: String },
    /// JSON-RPC protocol error.
    JsonRpc(String),
    /// [D4-003] Response size exceeded limit.
    ResponseTooLarge {
        server: String,
        size: usize,
        limit: usize,
    },
    /// [D4-007] Response ID does not match request ID.
    ResponseIdMismatch {
        server: String,
        expected: u64,
        actual: u64,
    },
    /// [D4-006] Invalid tool name.
    InvalidToolName {
        server: String,
        tool: String,
        reason: String,
    },
    /// [D4-001] Server not trusted.
    UntrustedServer { server: String },
    /// I/O error.
    Io(std::io::Error),
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigParse(msg) => write!(f, "MCP config parse error: {msg}"),
            Self::ServerStartFailed { server, reason } => {
                write!(f, "MCP server '{server}' failed to start: {reason}")
            }
            Self::InitializeFailed { server, reason } => {
                write!(f, "MCP server '{server}' initialization failed: {reason}")
            }
            Self::ToolCallFailed {
                server,
                tool,
                reason,
            } => write!(f, "MCP tool call '{tool}' on '{server}' failed: {reason}"),
            Self::Timeout {
                server,
                timeout_secs,
            } => write!(f, "MCP server '{server}' timed out after {timeout_secs}s"),
            Self::ServerCrashed { server } => {
                write!(f, "MCP server '{server}' crashed")
            }
            Self::JsonRpc(msg) => write!(f, "JSON-RPC error: {msg}"),
            Self::ResponseTooLarge {
                server,
                size,
                limit,
            } => write!(
                f,
                "MCP server '{server}' response too large: {size} bytes (limit: {limit})"
            ),
            Self::ResponseIdMismatch {
                server,
                expected,
                actual,
            } => write!(
                f,
                "MCP server '{server}' response ID mismatch: expected {expected}, got {actual}"
            ),
            Self::InvalidToolName {
                server,
                tool,
                reason,
            } => write!(
                f,
                "MCP server '{server}' invalid tool name '{tool}': {reason}"
            ),
            Self::UntrustedServer { server } => {
                write!(f, "MCP server '{server}' is not trusted")
            }
            Self::Io(e) => write!(f, "MCP I/O error: {e}"),
        }
    }
}

impl std::error::Error for McpError {}

/// MCP configuration file structure.
/// Wraps the mcpServers field from .anvil/mcp.json.
#[derive(Deserialize)]
pub struct McpConfigFile {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_server_config_deserialize() {
        let json = r#"{
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
            "env": {"API_KEY": "secret123"},
            "timeout_secs": 60
        }"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "npx");
        assert_eq!(
            config.args,
            vec!["-y", "@modelcontextprotocol/server-filesystem"]
        );
        assert_eq!(config.env.get("API_KEY").unwrap(), "secret123");
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_mcp_server_config_default_timeout() {
        let json = r#"{"command": "npx", "args": []}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_mcp_server_config_default_env() {
        let json = r#"{"command": "npx", "args": []}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert!(config.env.is_empty());
    }

    #[test]
    fn test_mcp_server_config_debug_redacted() {
        let json = r#"{
            "command": "npx",
            "args": ["test"],
            "env": {"API_KEY": "super_secret_value", "TOKEN": "another_secret"}
        }"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        let debug_output = format!("{config:?}");

        // Should NOT contain the actual secret values
        assert!(!debug_output.contains("super_secret_value"));
        assert!(!debug_output.contains("another_secret"));

        // Should contain [REDACTED]
        assert!(debug_output.contains("[REDACTED]"));

        // Should contain the key names
        assert!(debug_output.contains("API_KEY"));
        assert!(debug_output.contains("TOKEN"));

        // Should contain command and args
        assert!(debug_output.contains("npx"));
    }

    #[test]
    fn test_validate_tool_name_valid() {
        assert!(validate_tool_name("create_issue").is_ok());
        assert!(validate_tool_name("search").is_ok());
        assert!(validate_tool_name("get-data").is_ok());
        assert!(validate_tool_name("tool123").is_ok());
    }

    #[test]
    fn test_validate_tool_name_empty() {
        let result = validate_tool_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_validate_tool_name_control_chars() {
        let result = validate_tool_name("tool\x00name");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("control"));

        let result = validate_tool_name("tool\nname");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tool_name_double_underscore() {
        let result = validate_tool_name("tool__name");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("__"));
    }

    #[test]
    fn test_mcp_config_file_deserialize() {
        let json = r#"{
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": {"GITHUB_TOKEN": "$GITHUB_TOKEN"}
                }
            }
        }"#;
        let config: McpConfigFile = serde_json::from_str(json).unwrap();
        assert!(config.mcp_servers.contains_key("github"));
        let github = &config.mcp_servers["github"];
        assert_eq!(github.command, "npx");
    }

    #[test]
    fn test_mcp_error_display() {
        let err = McpError::ConfigParse("bad json".to_string());
        assert_eq!(format!("{err}"), "MCP config parse error: bad json");

        let err = McpError::ServerStartFailed {
            server: "test".to_string(),
            reason: "not found".to_string(),
        };
        assert!(format!("{err}").contains("test"));

        let err = McpError::Timeout {
            server: "slow".to_string(),
            timeout_secs: 30,
        };
        assert!(format!("{err}").contains("30"));

        let err = McpError::ResponseTooLarge {
            server: "big".to_string(),
            size: 20_000_000,
            limit: 10_485_760,
        };
        assert!(format!("{err}").contains("20000000"));

        let err = McpError::ResponseIdMismatch {
            server: "s".to_string(),
            expected: 1,
            actual: 2,
        };
        assert!(format!("{err}").contains("expected 1"));
        assert!(format!("{err}").contains("got 2"));
    }

    #[test]
    fn test_mcp_error_invalid_tool_name() {
        let err = McpError::InvalidToolName {
            server: "github".to_string(),
            tool: "bad__tool".to_string(),
            reason: "contains __".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("github"));
        assert!(msg.contains("bad__tool"));
    }

    #[test]
    fn test_mcp_error_untrusted() {
        let err = McpError::UntrustedServer {
            server: "evil".to_string(),
        };
        assert!(format!("{err}").contains("evil"));
        assert!(format!("{err}").contains("not trusted"));
    }

    #[test]
    fn test_extract_tool_result_text_content_array() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": "World"}
            ]
        });
        assert_eq!(extract_tool_result_text(&result).unwrap(), "Hello\nWorld");
    }

    #[test]
    fn test_extract_tool_result_text_string() {
        let result = serde_json::json!("simple result");
        assert_eq!(extract_tool_result_text(&result).unwrap(), "simple result");
    }

    #[test]
    fn test_extract_tool_result_text_fallback() {
        let result = serde_json::json!({"arbitrary": "data"});
        let text = extract_tool_result_text(&result).unwrap();
        assert!(text.contains("arbitrary"));
    }

    #[test]
    fn test_mcp_tool_info_debug() {
        let info = McpToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let debug = format!("{info:?}");
        assert!(debug.contains("test_tool"));
    }

    #[test]
    fn test_validate_server_name_same_rules() {
        // Server names use the same validation as tool names
        assert!(validate_tool_name("github").is_ok());
        assert!(validate_tool_name("my-server").is_ok());
        assert!(validate_tool_name("").is_err());
        assert!(validate_tool_name("bad__name").is_err());
    }
}
