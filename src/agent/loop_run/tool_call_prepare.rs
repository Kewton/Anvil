//! Per-tool-call argument normalization extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts `prepare_tool_call`, the pre-dispatch hook invoked on every
//! assistant-produced `ToolCall` before it reaches
//! `tool_call_execution::execute_tool_call`:
//!
//! 1. `normalize_tool_call_arguments` runs the per-tool argument
//!    sanitiser (e.g. XML-fallback recovery for tool calls emitted as
//!    free-text).
//! 2. For `Read` / `Write` / `Edit`, the `path` argument is resolved
//!    against `work_root` via `resolve_user_path`; the model-visible
//!    argument is written back as a workspace-relative path so later
//!    turns do not learn absolute project paths from history.
//! 3. For `Read` specifically: when the active
//!    `EffectiveToolPolicy::focused_edit_policy()` declares a focused
//!    target and the resolved path is a directory that contains it,
//!    the focused target replaces the directory path so the read
//!    lands on the file the policy is actually waiting for.
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&Agent`, matching the `actor_loop_flow` / `reply_retry` /
//! earlier vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::tool_history::focused_read_target_for_directory;
use crate::ollama::xml_fallback::ToolCall;
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::safety::path_guard::resolve_user_path;
use std::path::Path;

pub(super) fn prepare_tool_call(agent: &Agent, mut tool_call: ToolCall) -> ToolCall {
    tool_call.arguments = normalize_tool_call_arguments(&tool_call.name, tool_call.arguments);
    if matches!(tool_call.name.as_str(), "Read" | "Write" | "Edit")
        && let Some(arguments) = tool_call.arguments.as_object_mut()
        && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
        && let Ok(resolved) = resolve_user_path(&agent.work_root, raw_path)
    {
        let resolved = if tool_call.name == "Read" {
            super::effective_tool_policy_flow::effective_tool_policy(agent)
                .focused_edit_policy()
                .and_then(|policy| {
                    focused_read_target_for_directory(&resolved, &policy.target)
                        .then_some(policy.target.clone())
                })
                .unwrap_or(resolved)
        } else {
            resolved
        };
        arguments.insert(
            "path".to_string(),
            serde_json::Value::String(model_visible_tool_path(&agent.work_root, &resolved)),
        );
    }
    tool_call
}

fn model_visible_tool_path(work_root: &Path, resolved: &Path) -> String {
    let canonical_root =
        std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    resolved
        .strip_prefix(&canonical_root)
        .or_else(|_| resolved.strip_prefix(work_root))
        .map(|relative| relative.to_string_lossy().to_string())
        .unwrap_or_else(|_| resolved.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::model_visible_tool_path;

    #[test]
    fn model_visible_tool_path_strips_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let work_root = temp.path();
        let resolved = work_root.join("tests/test_math_utils.py");

        assert_eq!(
            model_visible_tool_path(work_root, &resolved),
            "tests/test_math_utils.py"
        );
    }

    #[test]
    fn model_visible_tool_path_falls_back_for_external_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let external = std::path::Path::new("/outside/file.txt");

        assert_eq!(
            model_visible_tool_path(temp.path(), external),
            external.display().to_string()
        );
    }
}
