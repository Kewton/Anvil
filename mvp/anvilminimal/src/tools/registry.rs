use std::path::PathBuf;

use anyhow::bail;
use serde::Serialize;
use serde_json::{Value, json};

use crate::mode::ExecutionMode;

use super::path_guard::{resolve_existing, resolve_for_create, resolve_optional_existing};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub root: PathBuf,
    pub mode: ExecutionMode,
    pub auto_approve: bool,
    pub interactive_approval: bool,
    pub offline: bool,
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
        match name {
            "Bash" => {
                let command = required_string(arguments, "command")?;
                crate::tools::bash::run(command, &context.root, context.offline)
            }
            "Read" => {
                let raw = required_string(arguments, "path")?;
                let path = resolve_existing(&context.root, raw)?;
                crate::tools::read::run(
                    &context.root,
                    &path,
                    optional_usize(arguments, "start_line"),
                    optional_usize(arguments, "end_line"),
                )
            }
            "Write" => {
                let raw = required_string(arguments, "path")?;
                let path = resolve_for_create(&context.root, raw)?;
                let content = required_string(arguments, "content")?;
                crate::tools::write::run(&path, content)
            }
            "Edit" => {
                let raw = required_string(arguments, "path")?;
                let path = resolve_optional_existing(&context.root, raw)?;
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
                crate::tools::glob::run(&context.root, pattern)
            }
            "Grep" => {
                let pattern = required_string(arguments, "pattern")?;
                let glob = arguments.get("glob").and_then(Value::as_str);
                let case_sensitive = arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                crate::tools::grep::run(&context.root, pattern, glob, case_sensitive)
            }
            other => bail!("unknown tool: {other}"),
        }
    }
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
    } else if message.contains("path escapes workspace") {
        "path_confinement_error"
    } else if message.contains("dangerous command blocked") {
        "dangerous_command"
    } else if message.contains("approval required") {
        "approval_required"
    } else {
        "tool_execution_error"
    }
}

pub fn recoverable_tool_error(err: &anyhow::Error) -> bool {
    matches!(tool_error_kind(err), "missing_arg" | "unknown_tool")
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
}
