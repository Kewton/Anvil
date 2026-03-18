//! MCP integration tests (Task 4.2).
//!
//! Tests for MCP tool name parsing, ToolInput::Mcp behaviour,
//! configuration loading, and MCP skip behaviour when unconfigured.

mod common;

use anvil::mcp::{McpConfigFile, McpServerConfig};
use anvil::tooling::{ToolInput, ToolKind};

// ---------------------------------------------------------------------------
// 1. parse_mcp_tool_name() tests (normal and abnormal cases)
// ---------------------------------------------------------------------------

#[test]
fn parse_mcp_tool_name_normal_github() {
    let result = anvil::tooling::parse_mcp_tool_name("mcp__github__create_issue");
    assert_eq!(
        result,
        Some(("github".to_string(), "create_issue".to_string()))
    );
}

#[test]
fn parse_mcp_tool_name_normal_slack() {
    let result = anvil::tooling::parse_mcp_tool_name("mcp__slack__post_message");
    assert_eq!(
        result,
        Some(("slack".to_string(), "post_message".to_string()))
    );
}

#[test]
fn parse_mcp_tool_name_tool_with_underscores() {
    // Tool name itself may contain underscores or double underscores after the third segment
    let result = anvil::tooling::parse_mcp_tool_name("mcp__server__my__tool");
    // splitn(3, "__") should capture "my__tool" as the third part
    assert_eq!(result, Some(("server".to_string(), "my__tool".to_string())));
}

#[test]
fn parse_mcp_tool_name_missing_tool() {
    // Only two segments: "mcp" and "server", no tool name
    let result = anvil::tooling::parse_mcp_tool_name("mcp__server");
    assert_eq!(result, None);
}

#[test]
fn parse_mcp_tool_name_not_mcp_prefix() {
    let result = anvil::tooling::parse_mcp_tool_name("file.read");
    assert_eq!(result, None);
}

#[test]
fn parse_mcp_tool_name_single_underscore() {
    let result = anvil::tooling::parse_mcp_tool_name("mcp_single_underscore_tool");
    assert_eq!(result, None);
}

#[test]
fn parse_mcp_tool_name_empty_server() {
    // "mcp____tool" → splitn(3, "__") → ["mcp", "", "tool"]
    // Should be None because server is empty
    let result = anvil::tooling::parse_mcp_tool_name("mcp____tool");
    assert_eq!(result, None);
}

#[test]
fn parse_mcp_tool_name_empty_tool() {
    // "mcp__server__" → splitn(3, "__") → ["mcp", "server", ""]
    // Should be None because tool is empty
    let result = anvil::tooling::parse_mcp_tool_name("mcp__server__");
    assert_eq!(result, None);
}

// ---------------------------------------------------------------------------
// 2. ToolInput::Mcp::kind() test
// ---------------------------------------------------------------------------

#[test]
fn tool_input_mcp_kind_returns_mcp() {
    let input = ToolInput::Mcp {
        server: "github".to_string(),
        tool: "create_issue".to_string(),
        arguments: serde_json::json!({"title": "test"}),
    };
    assert_eq!(input.kind(), ToolKind::Mcp);
}

// ---------------------------------------------------------------------------
// 3. ToolInput::from_json() MCP parse test
// ---------------------------------------------------------------------------

#[test]
fn tool_input_from_json_mcp_tool() {
    let value = serde_json::json!({
        "tool": "mcp__github__create_issue",
        "id": "call_001",
        "title": "test issue",
        "body": "description"
    });
    let result = ToolInput::from_json("mcp__github__create_issue", &value);
    assert!(result.is_ok());
    let input = result.unwrap();
    match input {
        ToolInput::Mcp {
            server,
            tool,
            arguments,
        } => {
            assert_eq!(server, "github");
            assert_eq!(tool, "create_issue");
            // arguments should be the full JSON value
            assert!(arguments.get("title").is_some());
        }
        _ => panic!("Expected ToolInput::Mcp"),
    }
}

#[test]
fn tool_input_from_json_unknown_non_mcp_tool_errors() {
    let value = serde_json::json!({"tool": "unknown_tool"});
    let result = ToolInput::from_json("unknown_tool", &value);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unsupported tool"));
}

#[test]
fn tool_input_from_json_invalid_mcp_format_errors() {
    // "mcp__server" without tool segment → should fail
    let value = serde_json::json!({"tool": "mcp__server"});
    let result = ToolInput::from_json("mcp__server", &value);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unsupported tool"));
}

// ---------------------------------------------------------------------------
// 4. MCP skip when unconfigured (load_mcp_config returns None)
// ---------------------------------------------------------------------------

#[test]
fn load_mcp_config_returns_none_when_file_missing() {
    let dir = common::unique_test_dir("mcp_skip");
    let config = common::build_config_in(dir);
    // mcp.json does not exist in the temp dir
    let result = anvil::config::load_mcp_config(&config.paths);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn load_mcp_config_returns_error_on_invalid_json() {
    let dir = common::unique_test_dir("mcp_bad_json");
    let anvil_dir = dir.join(".anvil");
    std::fs::create_dir_all(&anvil_dir).unwrap();
    std::fs::write(anvil_dir.join("mcp.json"), "not valid json").unwrap();

    let config = common::build_config_in(dir);
    let result = anvil::config::load_mcp_config(&config.paths);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("parse"));
}

// ---------------------------------------------------------------------------
// 5. McpServerConfig Deserialize tests
// ---------------------------------------------------------------------------

#[test]
fn mcp_server_config_deserialize_full() {
    let json = r#"{
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"],
        "env": {"GITHUB_TOKEN": "$GITHUB_TOKEN"},
        "timeout_secs": 60
    }"#;
    let config: McpServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.command, "npx");
    assert_eq!(config.args.len(), 2);
    assert_eq!(config.env.get("GITHUB_TOKEN").unwrap(), "$GITHUB_TOKEN");
    assert_eq!(config.timeout_secs, 60);
}

#[test]
fn mcp_server_config_deserialize_defaults() {
    let json = r#"{"command": "node", "args": ["server.js"]}"#;
    let config: McpServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.command, "node");
    assert!(config.env.is_empty());
    assert_eq!(config.timeout_secs, 30); // default
}

#[test]
fn mcp_config_file_deserialize() {
    let json = r#"{
        "mcpServers": {
            "github": {
                "command": "npx",
                "args": ["-y", "server-github"],
                "env": {"GITHUB_TOKEN": "$GITHUB_TOKEN"}
            },
            "slack": {
                "command": "node",
                "args": ["slack-server.js"]
            }
        }
    }"#;
    let config: McpConfigFile = serde_json::from_str(json).unwrap();
    assert_eq!(config.mcp_servers.len(), 2);
    assert!(config.mcp_servers.contains_key("github"));
    assert!(config.mcp_servers.contains_key("slack"));
}

// ---------------------------------------------------------------------------
// 6. expand_env_vars() tests (via load_mcp_config integration)
// ---------------------------------------------------------------------------

#[test]
fn load_mcp_config_expands_env_vars() {
    let dir = common::unique_test_dir("mcp_env_expand");
    let anvil_dir = dir.join(".anvil");
    std::fs::create_dir_all(&anvil_dir).unwrap();

    // Set a known env var for the test
    let test_var = format!("ANVIL_TEST_MCP_{}", std::process::id());
    // SAFETY: This test runs sequentially and the env var name is unique per process.
    unsafe { std::env::set_var(&test_var, "resolved_value") };

    let mcp_json = format!(
        r#"{{
            "mcpServers": {{
                "test": {{
                    "command": "echo",
                    "args": ["hello"],
                    "env": {{"MY_VAR": "${}"}}
                }}
            }}
        }}"#,
        test_var
    );
    std::fs::write(anvil_dir.join("mcp.json"), &mcp_json).unwrap();

    let config = common::build_config_in(dir);
    let result = anvil::config::load_mcp_config(&config.paths);
    assert!(result.is_ok());
    let configs = result.unwrap().unwrap();
    let test_config = &configs["test"];
    assert_eq!(test_config.env.get("MY_VAR").unwrap(), "resolved_value");

    // Clean up env var
    // SAFETY: This test runs sequentially and the env var name is unique per process.
    unsafe { std::env::remove_var(&test_var) };
}

#[test]
fn load_mcp_config_errors_on_undefined_env_var() {
    let dir = common::unique_test_dir("mcp_env_undef");
    let anvil_dir = dir.join(".anvil");
    std::fs::create_dir_all(&anvil_dir).unwrap();

    let mcp_json = r#"{
        "mcpServers": {
            "test": {
                "command": "echo",
                "args": [],
                "env": {"MY_VAR": "$TOTALLY_UNDEFINED_VAR_XYZ_12345"}
            }
        }
    }"#;
    std::fs::write(anvil_dir.join("mcp.json"), mcp_json).unwrap();

    let config = common::build_config_in(dir);
    let result = anvil::config::load_mcp_config(&config.paths);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("TOTALLY_UNDEFINED_VAR_XYZ_12345"));
    assert!(err.contains("not set"));
}

// ---------------------------------------------------------------------------
// Additional: MCP tool serde roundtrip test (D3-010)
// ---------------------------------------------------------------------------

#[test]
fn tool_input_mcp_serde_roundtrip() {
    let input = ToolInput::Mcp {
        server: "github".to_string(),
        tool: "create_issue".to_string(),
        arguments: serde_json::json!({"title": "test", "body": "content"}),
    };
    let serialized = serde_json::to_string(&input).unwrap();
    let deserialized: ToolInput = serde_json::from_str(&serialized).unwrap();
    assert_eq!(input, deserialized);
}

// ---------------------------------------------------------------------------
// Additional: validate_tool_name tests (via public API)
// ---------------------------------------------------------------------------

#[test]
fn validate_tool_name_accepts_valid_names() {
    assert!(anvil::mcp::validate_tool_name("create_issue").is_ok());
    assert!(anvil::mcp::validate_tool_name("get-data").is_ok());
    assert!(anvil::mcp::validate_tool_name("a").is_ok());
}

#[test]
fn validate_tool_name_rejects_empty() {
    assert!(anvil::mcp::validate_tool_name("").is_err());
}

#[test]
fn validate_tool_name_rejects_control_chars() {
    assert!(anvil::mcp::validate_tool_name("tool\x00name").is_err());
    assert!(anvil::mcp::validate_tool_name("tool\nname").is_err());
}

#[test]
fn validate_tool_name_rejects_double_underscore() {
    assert!(anvil::mcp::validate_tool_name("bad__name").is_err());
}
