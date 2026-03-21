//! Offline mode policy checks.
//!
//! Provides [`check_offline_blocked`] which is called from both
//! `agentic.rs` (validate_and_approve_all) and `subagent.rs` (run_turn)
//! to enforce offline mode restrictions.

use crate::config::EffectiveConfig;
use crate::tooling::{ToolCallRequest, ToolInput, is_network_command};

/// Suffix appended to tool name when generating block summary messages.
pub const OFFLINE_BLOCK_SUMMARY_SUFFIX: &str = "is unavailable in offline mode";

/// Payload text returned to the LLM when a tool call is blocked.
pub const OFFLINE_BLOCK_PAYLOAD: &str =
    "offline mode does not allow network access. Use local tools instead.";

/// Check whether a tool call should be blocked in offline mode.
///
/// Returns `Some(summary)` if the tool is blocked, `None` otherwise.
/// Blocks `WebFetch`, `WebSearch`, and `Mcp` tool inputs when
/// `config.mode.offline` is `true`.
pub fn check_offline_blocked(config: &EffectiveConfig, call: &ToolCallRequest) -> Option<String> {
    if !config.mode.offline {
        return None;
    }
    match &call.input {
        ToolInput::WebFetch { .. }
        | ToolInput::WebSearch { .. }
        // Defense in Depth: MCP tools are normally unreachable in offline mode
        // (MCP initialization is skipped), but block explicitly as a safety net.
        | ToolInput::Mcp { .. } => Some(format!(
            "{} {}",
            call.tool_name, OFFLINE_BLOCK_SUMMARY_SUFFIX
        )),
        // Block shell commands that perform network access
        ToolInput::ShellExec { command } if is_network_command(command) => Some(format!(
            "{} {}",
            call.tool_name, OFFLINE_BLOCK_SUMMARY_SUFFIX
        )),
        // Git tools are local-only, never blocked in offline mode
        ToolInput::GitStatus {} | ToolInput::GitDiff { .. } | ToolInput::GitLog { .. } => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EffectiveConfig;
    use crate::tooling::ToolCallRequest;

    fn make_config(offline: bool) -> EffectiveConfig {
        let mut config = EffectiveConfig::default_for_test().unwrap();
        config.mode.offline = offline;
        config
    }

    fn make_call(tool_name: &str, input: ToolInput) -> ToolCallRequest {
        ToolCallRequest {
            tool_call_id: "call_1".to_string(),
            tool_name: tool_name.to_string(),
            input,
        }
    }

    #[test]
    fn offline_blocks_web_fetch() {
        let config = make_config(true);
        let call = make_call(
            "web.fetch",
            ToolInput::WebFetch {
                url: "https://example.com".to_string(),
            },
        );
        let result = check_offline_blocked(&config, &call);
        assert!(result.is_some());
        assert!(result.unwrap().contains("web.fetch"));
    }

    #[test]
    fn offline_blocks_web_search() {
        let config = make_config(true);
        let call = make_call(
            "web.search",
            ToolInput::WebSearch {
                query: "test".to_string(),
            },
        );
        let result = check_offline_blocked(&config, &call);
        assert!(result.is_some());
        assert!(result.unwrap().contains("web.search"));
    }

    #[test]
    fn offline_blocks_mcp() {
        let config = make_config(true);
        let call = make_call(
            "mcp__server__tool",
            ToolInput::Mcp {
                server: "server".to_string(),
                tool: "tool".to_string(),
                arguments: serde_json::Value::Null,
            },
        );
        let result = check_offline_blocked(&config, &call);
        assert!(result.is_some());
        assert!(result.unwrap().contains("mcp__server__tool"));
    }

    #[test]
    fn offline_allows_file_read() {
        let config = make_config(true);
        let call = make_call(
            "file.read",
            ToolInput::FileRead {
                path: "./test.rs".to_string(),
            },
        );
        assert!(check_offline_blocked(&config, &call).is_none());
    }

    #[test]
    fn offline_allows_shell_exec() {
        let config = make_config(true);
        let call = make_call(
            "shell.exec",
            ToolInput::ShellExec {
                command: "ls".to_string(),
            },
        );
        assert!(check_offline_blocked(&config, &call).is_none());
    }

    #[test]
    fn offline_allows_agent_explore() {
        let config = make_config(true);
        let call = make_call(
            "agent.explore",
            ToolInput::AgentExplore {
                prompt: "test".to_string(),
                scope: None,
            },
        );
        assert!(check_offline_blocked(&config, &call).is_none());
    }

    #[test]
    fn non_offline_allows_web_fetch() {
        let config = make_config(false);
        let call = make_call(
            "web.fetch",
            ToolInput::WebFetch {
                url: "https://example.com".to_string(),
            },
        );
        assert!(check_offline_blocked(&config, &call).is_none());
    }

    #[test]
    fn offline_blocks_network_shell_commands() {
        let config = make_config(true);
        for cmd in &[
            "curl https://example.com",
            "wget https://example.com",
            "ssh user@host",
            "ping 8.8.8.8",
        ] {
            let call = make_call(
                "shell.exec",
                ToolInput::ShellExec {
                    command: cmd.to_string(),
                },
            );
            let result = check_offline_blocked(&config, &call);
            assert!(
                result.is_some(),
                "Expected {cmd} to be blocked in offline mode"
            );
            assert!(result.unwrap().contains("shell.exec"));
        }
    }

    #[test]
    fn offline_allows_non_network_shell() {
        let config = make_config(true);
        for cmd in &["ls -la", "git log", "cargo test", "cat file.txt"] {
            let call = make_call(
                "shell.exec",
                ToolInput::ShellExec {
                    command: cmd.to_string(),
                },
            );
            assert!(
                check_offline_blocked(&config, &call).is_none(),
                "Expected {cmd} to be allowed in offline mode"
            );
        }
    }

    #[test]
    fn non_offline_allows_mcp() {
        let config = make_config(false);
        let call = make_call(
            "mcp__server__tool",
            ToolInput::Mcp {
                server: "server".to_string(),
                tool: "tool".to_string(),
                arguments: serde_json::Value::Null,
            },
        );
        assert!(check_offline_blocked(&config, &call).is_none());
    }
}
