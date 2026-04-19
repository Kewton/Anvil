use serde::Serialize;
use serde_json::Value;

use crate::modes::plan_act::ExecutionMode;
use crate::safety::path_guard::resolve_user_path;
use crate::tools::{bash, edit, glob, grep, read, write};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub root: std::path::PathBuf,
    pub mode: ExecutionMode,
    pub plan_path: Option<std::path::PathBuf>,
    pub auto_approve: bool,
    pub interactive_approval: bool,
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
    ) -> Result<String, String> {
        enforce_mode(name, arguments, context)?;
        maybe_confirm(name, arguments, context)?;

        match name {
            "Bash" => {
                let command = get_required_string(arguments, "command")?;
                bash::run(command, &context.root)
            }
            "Read" => {
                let path =
                    resolve_user_path(&context.root, get_required_string(arguments, "path")?)?;
                let start_line = get_optional_usize(arguments, "start_line");
                let end_line = get_optional_usize(arguments, "end_line");
                read::run(&path, start_line, end_line)
            }
            "Write" => {
                let path = resolve_write_path(
                    &context.root,
                    get_required_string(arguments, "path")?,
                    context,
                )?;
                let content = get_required_string(arguments, "content")?;
                write::run(&path, content)
            }
            "Edit" => {
                let path = resolve_write_path(
                    &context.root,
                    get_required_string(arguments, "path")?,
                    context,
                )?;
                let old = get_required_string(arguments, "old_string")?;
                let new = get_required_string(arguments, "new_string")?;
                let replace_all = arguments
                    .get("replace_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                edit::run(&path, old, new, replace_all)
            }
            "Glob" => {
                let pattern = get_required_string(arguments, "pattern")?;
                glob::run(&context.root, pattern)
            }
            "Grep" => {
                let pattern = get_required_string(arguments, "pattern")?;
                let glob = arguments.get("glob").and_then(Value::as_str);
                let case_sensitive = arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                grep::run(&context.root, pattern, glob, case_sensitive)
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

fn default_tool_specs() -> Vec<ToolSpec> {
    vec![
        tool(
            "Bash",
            "Run a shell command in the project directory.",
            serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }),
        ),
        tool(
            "Read",
            "Read a text file or list a directory. Absolute paths are preferred.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["path"]
            }),
        ),
        tool(
            "Write",
            "Create or overwrite a file. Absolute paths are preferred.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        ),
        tool(
            "Edit",
            "Replace exact text in an existing file. Absolute paths are preferred.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        ),
        tool(
            "Glob",
            "Find files by glob pattern.",
            serde_json::json!({
                "type": "object",
                "properties": { "pattern": { "type": "string" } },
                "required": ["pattern"]
            }),
        ),
        tool(
            "Grep",
            "Search repository text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "glob": { "type": "string" },
                    "case_sensitive": { "type": "boolean" }
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        kind: "function".to_string(),
        function: FunctionSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

fn get_required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field: {field}"))
}

fn get_optional_usize(value: &Value, field: &str) -> Option<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn enforce_mode(name: &str, arguments: &Value, context: &ToolContext) -> Result<(), String> {
    if context.mode == ExecutionMode::Act {
        return Ok(());
    }

    match name {
        "Read" | "Glob" | "Grep" => Ok(()),
        "Write" | "Edit" => {
            let raw_path = get_required_string(arguments, "path")?;
            let allowed_path = context
                .plan_path
                .as_ref()
                .ok_or_else(|| "plan mode write target is not set".to_string())?;
            let canonical_allowed = canonicalize_with_missing_tail(allowed_path);
            let input_path = std::path::Path::new(raw_path);
            let canonical_requested = if input_path.is_absolute() {
                canonicalize_with_missing_tail(input_path)
            } else {
                resolve_user_path(&context.root, raw_path)?
            };
            if canonical_requested == canonical_allowed {
                Ok(())
            } else {
                Err(format!(
                    "plan mode only allows writing the plan file: {}",
                    canonical_allowed.display()
                ))
            }
        }
        _ => Err("plan mode only allows Read, Glob, Grep, and plan file edits".to_string()),
    }
}

fn resolve_write_path(
    root: &std::path::Path,
    raw: &str,
    context: &ToolContext,
) -> Result<std::path::PathBuf, String> {
    if context.mode == ExecutionMode::Plan
        && let Some(allowed) = context.plan_path.as_ref()
    {
        let canonical_allowed = canonicalize_with_missing_tail(allowed);
        let input_path = std::path::Path::new(raw);
        let canonical_requested = if input_path.is_absolute() {
            canonicalize_with_missing_tail(input_path)
        } else {
            resolve_user_path(root, raw)?
        };
        if canonical_requested == canonical_allowed {
            return Ok(canonical_allowed);
        }
    }
    resolve_user_path(root, raw)
}

fn canonicalize_with_missing_tail(path: &std::path::Path) -> std::path::PathBuf {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        let Some(name) = cursor.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent;
    }
    let Ok(mut resolved) = std::fs::canonicalize(cursor) else {
        return path.to_path_buf();
    };
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    resolved
}

fn maybe_confirm(name: &str, arguments: &Value, context: &ToolContext) -> Result<(), String> {
    let needs_confirmation = matches!(name, "Bash" | "Write" | "Edit");
    if !needs_confirmation || context.auto_approve {
        return Ok(());
    }
    if !context.interactive_approval {
        return Err(format!("{name} requires --yes in non-interactive mode"));
    }

    let summary = match name {
        "Bash" => arguments
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("<missing command>"),
        _ => arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("<missing path>"),
    };

    println!("Approve {name}: {summary}? [y/N]");
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|err| format!("failed to read approval: {err}"))?;
    if matches!(input.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        Err(format!("{name} denied by user"))
    }
}

pub fn truncate_output(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n...[truncated]")
}
