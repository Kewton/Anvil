use std::path::PathBuf;

use anyhow::bail;
use serde::Serialize;
use serde_json::{Value, json};

use crate::eval_events;
use crate::mode::ExecutionMode;

use super::args_recovery::recover_tool_arguments;
use super::path_guard::{
    normalize_absolute_workspace_glob, normalize_absolute_workspace_path, resolve_existing,
    resolve_for_create, resolve_optional_existing,
};
use super::workspace_policy::{WorkspacePolicy, ensure_tool_path_allowed};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub root: PathBuf,
    pub mode: ExecutionMode,
    pub auto_approve: bool,
    pub interactive_approval: bool,
    pub offline: bool,
    pub workspace_policy: WorkspacePolicy,
    pub eval_events_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ToolRegistry {
    specs: Vec<ToolSpec>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            specs: default_tool_specs(),
        }
    }
}

impl ToolRegistry {
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    pub fn execute(
        &self,
        name: &str,
        arguments: &Value,
        context: &ToolContext,
    ) -> anyhow::Result<String> {
        enforce_mode(name, context.mode)?;
        if is_mutating(name) && !context.auto_approve && !context.interactive_approval {
            bail!("approval required for {name}; rerun with --yes or use interactive approval");
        }
        let recovered = recover_tool_arguments(name, arguments.clone());
        let arguments = &recovered.arguments;
        match name {
            "Bash" => {
                let command = required_string(arguments, "command")?;
                crate::tools::bash::run(command, &context.root, context.offline)
            }
            "Read" => {
                let raw = required_string(arguments, "path")?;
                let normalized = normalize_path_arg(context, "Read", raw)?;
                let path = resolve_existing(&context.root, &normalized)?;
                ensure_tool_path_allowed(&context.root, &path, context.workspace_policy)?;
                crate::tools::read::run(
                    &context.root,
                    &path,
                    optional_usize(arguments, "start_line"),
                    optional_usize(arguments, "end_line"),
                    context.workspace_policy,
                )
            }
            "Write" => {
                let raw = required_string(arguments, "path")?;
                let normalized = normalize_path_arg(context, "Write", raw)?;
                let path = resolve_for_create(&context.root, &normalized)?;
                let content = required_string(arguments, "content")?;
                crate::tools::write::run(&path, content)
            }
            "Edit" => {
                let raw = required_string(arguments, "path")?;
                let normalized = normalize_path_arg(context, "Edit", raw)?;
                let path = resolve_optional_existing(&context.root, &normalized)?;
                let old = required_string(arguments, "old_string")?;
                let new = required_string(arguments, "new_string")?;
                let replace_all = arguments
                    .get("replace_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                crate::tools::edit::run(&path, old, new, replace_all)
            }
            "Glob" => {
                let pattern = required_string(arguments, "pattern")?;
                let normalized = normalize_glob_arg(context, "Glob", pattern)?;
                crate::tools::glob::run(&context.root, &normalized, context.workspace_policy)
            }
            "Grep" => {
                let pattern = required_string(arguments, "pattern")?;
                let glob = arguments
                    .get("glob")
                    .and_then(Value::as_str)
                    .map(|glob| normalize_glob_arg(context, "Grep", glob))
                    .transpose()?;
                let case_sensitive = arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                crate::tools::grep::run(
                    &context.root,
                    pattern,
                    glob.as_deref(),
                    case_sensitive,
                    context.workspace_policy,
                )
            }
            other => bail!("unknown tool: {other}"),
        }
    }
}

fn normalize_path_arg(context: &ToolContext, tool: &str, raw: &str) -> anyhow::Result<String> {
    match normalize_absolute_workspace_path(&context.root, raw)? {
        Some(normalized) => {
            emit_path_normalized(context, tool, raw, &normalized);
            Ok(normalized)
        }
        None => Ok(raw.to_string()),
    }
}

fn normalize_glob_arg(context: &ToolContext, tool: &str, raw: &str) -> anyhow::Result<String> {
    match normalize_absolute_workspace_glob(&context.root, raw)? {
        Some(normalized) => {
            emit_path_normalized(context, tool, raw, &normalized);
            Ok(normalized)
        }
        None => Ok(raw.to_string()),
    }
}

fn emit_path_normalized(context: &ToolContext, tool: &str, original: &str, normalized: &str) {
    eval_events::emit(
        context.eval_events_path.as_deref(),
        json!({
            "event": "tool_args_path_normalized",
            "tool": tool,
            "original": original,
            "normalized": normalized,
        }),
    );
}

fn enforce_mode(name: &str, mode: ExecutionMode) -> anyhow::Result<()> {
    if mode == ExecutionMode::Plan && !matches!(name, "Read" | "Glob" | "Grep") {
        bail!("Plan mode allows only Read, Glob, and Grep");
    }
    Ok(())
}

fn is_mutating(name: &str) -> bool {
    matches!(name, "Write" | "Edit" | "Bash")
}

pub fn required_string<'a>(value: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing string argument `{key}`"))
}

pub fn tool_error_kind(err: &anyhow::Error) -> &'static str {
    let message = err.to_string();
    if message.starts_with("missing string argument `") {
        "missing_arg"
    } else if message.starts_with("unknown tool:") {
        "unknown_tool"
    } else if message.contains("path escapes workspace")
        || message.contains("path may not contain ..")
        || message.contains("absolute path is not allowed")
        || message.contains("path contains NUL byte")
    {
        "path_confinement_error"
    } else if message.contains("path_not_found_recoverable") {
        "path_not_found_recoverable"
    } else if message.contains("workspace_policy_blocked") {
        "workspace_policy_blocked"
    } else if message.contains("invalid glob pattern") {
        "invalid_glob"
    } else if message.contains("Is a directory") || message.contains("is a directory") {
        "read_directory"
    } else if message.contains("dangerous command blocked") {
        "dangerous_command"
    } else if message.contains("verify_command_policy_error") {
        "verify_command_policy_error"
    } else if message.contains("edit_anchor_not_found") {
        "edit_anchor_not_found"
    } else if message.contains("edit_noop") {
        "edit_noop"
    } else if message.contains("edit_ambiguous_anchor") {
        "edit_ambiguous_anchor"
    } else if message.contains("edit_already_applied") {
        "edit_already_applied"
    } else if message.contains("approval required") {
        "approval_required"
    } else {
        "tool_execution_error"
    }
}

pub fn recoverable_tool_error(err: &anyhow::Error) -> bool {
    matches!(
        tool_error_kind(err),
        "missing_arg"
            | "unknown_tool"
            | "path_not_found_recoverable"
            | "verify_command_policy_error"
            | "invalid_glob"
            | "read_directory"
            | "edit_anchor_not_found"
            | "edit_noop"
            | "edit_ambiguous_anchor"
            | "edit_already_applied"
    )
}

pub fn missing_arg_name(err: &anyhow::Error) -> Option<String> {
    let message = err.to_string();
    let rest = message.strip_prefix("missing string argument `")?;
    Some(rest.split('`').next()?.to_string())
}

fn optional_usize(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

fn default_tool_specs() -> Vec<ToolSpec> {
    vec![
        spec(
            "Bash",
            "Run a local build/test/read-only shell command in the workspace.",
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        ),
        spec(
            "Read",
            "Read a workspace-relative file path.",
            json!({"type":"object","properties":{"path":{"type":"string"},"start_line":{"type":"integer"},"end_line":{"type":"integer"}},"required":["path"]}),
        ),
        spec(
            "Write",
            "Write a workspace-relative file path. Parent directories are created automatically.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        spec(
            "Edit",
            "Replace an exact string in a workspace-relative file path.",
            json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}),
        ),
        spec(
            "Glob",
            "List files matching a workspace-relative glob pattern.",
            json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}),
        ),
        spec(
            "Grep",
            "Search text in workspace files.",
            json!({"type":"object","properties":{"pattern":{"type":"string"},"glob":{"type":"string"},"case_sensitive":{"type":"boolean"}},"required":["pattern"]}),
        ),
    ]
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        kind: "function".to_string(),
        function: FunctionSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_act_tool_allowlist() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Plan,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        assert!(
            registry
                .execute("Write", &json!({"path":"a","content":"b"}), &context)
                .is_err()
        );
    }

    #[test]
    fn write_parent_dir() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        registry
            .execute("Write", &json!({"path":"a/b.txt","content":"ok"}), &context)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn absolute_internal_write_is_normalized_and_audited() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: Some(events.clone()),
        };
        let absolute = dir.path().join("src/app/page.tsx");

        registry
            .execute(
                "Write",
                &json!({"path": absolute.display().to_string(), "content":"ok"}),
                &context,
            )
            .unwrap();

        assert_eq!(std::fs::read_to_string(&absolute).unwrap(), "ok");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains(r#""event":"tool_args_path_normalized""#));
        assert!(event_text.contains(r#""tool":"Write""#));
        assert!(event_text.contains(r#""normalized":"src/app/page.tsx""#));
    }

    #[test]
    fn absolute_outside_write_is_rejected_as_path_confinement() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let outside_path = outside.path().join("escape.txt");

        let err = registry
            .execute(
                "Write",
                &json!({"path": outside_path.display().to_string(), "content":"no"}),
                &context,
            )
            .unwrap_err();

        assert_eq!(tool_error_kind(&err), "path_confinement_error");
        assert!(err.to_string().contains("use workspace-relative paths"));
        assert!(!outside_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn absolute_internal_write_through_symlink_escape_is_rejected() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let raw = dir.path().join("out/escape.txt");

        let err = registry
            .execute(
                "Write",
                &json!({"path": raw.display().to_string(), "content":"no"}),
                &context,
            )
            .unwrap_err();

        assert_eq!(tool_error_kind(&err), "path_confinement_error");
        assert!(!outside.path().join("escape.txt").exists());
    }

    #[test]
    fn relative_write_behavior_does_not_emit_normalization_event() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: Some(events.clone()),
        };

        let output = registry
            .execute(
                "Write",
                &json!({"path":"notes/new.md","content":"ok"}),
                &context,
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes/new.md")).unwrap(),
            "ok"
        );
        assert_eq!(
            output,
            format!(
                "wrote {}",
                dir.path()
                    .canonicalize()
                    .unwrap()
                    .join("notes/new.md")
                    .display()
            )
        );
        assert!(
            !events.exists(),
            "relative paths must not be rewritten or audited as normalized"
        );
    }

    #[test]
    fn dot_dot_traversal_still_rejected() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };

        let err = registry
            .execute(
                "Write",
                &json!({"path":"safe/../escape.txt","content":"no"}),
                &context,
            )
            .unwrap_err();

        assert_eq!(tool_error_kind(&err), "path_confinement_error");
        assert!(err.to_string().contains("use workspace-relative paths"));
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn dangerous_command() {
        assert!(crate::tools::bash::blocked_reason("rm -rf /", false).is_some());
    }

    #[test]
    fn classifies_recoverable_validation_errors() {
        let err = required_string(&json!({}), "pattern").unwrap_err();
        assert_eq!(tool_error_kind(&err), "missing_arg");
        assert!(recoverable_tool_error(&err));
        assert_eq!(missing_arg_name(&err).as_deref(), Some("pattern"));
    }

    #[test]
    fn normal_workspace_policy_blocks_anvil_metadata_read() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".anvil")).unwrap();
        std::fs::write(dir.path().join(".anvil/session.json"), "{}").unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let err = registry
            .execute("Read", &json!({"path":".anvil/session.json"}), &context)
            .unwrap_err();
        assert_eq!(tool_error_kind(&err), "workspace_policy_blocked");
        assert!(!recoverable_tool_error(&err));
    }

    #[test]
    fn glob_uses_ignore_walker() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "").unwrap();
        std::fs::write(dir.path().join("my-node_modules-note.md"), "").unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Plan,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let output = registry
            .execute("Glob", &json!({"pattern":"**/*"}), &context)
            .unwrap();
        assert!(!output.contains("node_modules/pkg/index.js"));
        assert!(output.contains("my-node_modules-note.md"));
    }

    #[test]
    fn workdir_prefix_path_miss_is_recoverable_with_hint() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ok").unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let err = registry
            .execute("Read", &json!({"path":"workdir/a.txt"}), &context)
            .unwrap_err();
        assert_eq!(tool_error_kind(&err), "path_not_found_recoverable");
        assert!(recoverable_tool_error(&err));
    }

    #[test]
    fn absolute_path_escape_remains_hard_failure() {
        let err = crate::tools::path_guard::resolve_existing(
            tempfile::tempdir().unwrap().path(),
            "/etc/passwd",
        )
        .unwrap_err();
        assert!(!recoverable_tool_error(&err));
    }

    #[test]
    fn recoverable_provider_aliases_are_executed() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        registry
            .execute(
                "Write",
                &json!({"arguments":{"file":"notes.txt","body":"ok"}}),
                &context,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn unsafe_alias_path_is_not_recoverable() {
        let registry = ToolRegistry::default();
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            root: dir.path().to_path_buf(),
            mode: ExecutionMode::Act,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            workspace_policy: WorkspacePolicy::NormalTask,
            eval_events_path: None,
        };
        let err = registry
            .execute(
                "Write",
                &json!({"arguments":{"file":"../secret.txt","body":"no"}}),
                &context,
            )
            .unwrap_err();
        assert_eq!(tool_error_kind(&err), "path_confinement_error");
        assert!(!recoverable_tool_error(&err));
    }
}
