use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use serde_json::Value;

use crate::modes::plan_act::{ExecutionMode, PlanStage};
use crate::safety::path_guard::resolve_user_path;
use crate::tools::{bash, edit, glob, grep, read, write};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub root: std::path::PathBuf,
    pub mode: ExecutionMode,
    pub plan_path: Option<std::path::PathBuf>,
    pub plan_stage: PlanStage,
    pub auto_approve: bool,
    pub interactive_approval: bool,
    pub offline: bool,
    pub cancel_flag: Option<Arc<AtomicBool>>,
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
        enforce_plan_stage_scope(name, arguments, context)?;
        maybe_confirm(name, arguments, context)?;

        match name {
            "Bash" => {
                let command = get_required_string(arguments, "command")?;
                bash::run(
                    command,
                    &context.root,
                    context.cancel_flag.as_ref(),
                    context.offline,
                )
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

pub(crate) fn resolve_plan_mode_write_target(
    root: &std::path::Path,
    raw_path: &str,
    plan_path: Option<&std::path::Path>,
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(allowed_path) = plan_path else {
        return Ok(None);
    };
    let canonical_allowed = canonicalize_with_missing_tail(allowed_path);
    let input_path = std::path::Path::new(raw_path);
    let requested = if input_path.is_absolute() {
        if path_has_same_filename(input_path, allowed_path) {
            canonical_allowed.clone()
        } else {
            canonicalize_with_missing_tail(input_path)
        }
    } else if path_has_same_filename(input_path, allowed_path) {
        canonical_allowed.clone()
    } else {
        resolve_user_path(root, raw_path)?
    };
    Ok((requested == canonical_allowed).then_some(canonical_allowed))
}

fn path_has_same_filename(lhs: &std::path::Path, rhs: &std::path::Path) -> bool {
    lhs.file_name()
        .zip(rhs.file_name())
        .is_some_and(|(lhs, rhs)| lhs == rhs)
}

fn default_tool_specs() -> Vec<ToolSpec> {
    vec![
        tool(
            "Bash",
            "Run a shell command in the project directory. Runtime classifies commands as read-only, build-test, or general, and offline mode blocks networked or general shell commands.",
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
            if resolve_plan_mode_write_target(
                &context.root,
                raw_path,
                context.plan_path.as_deref(),
            )?
            .is_some()
            {
                Ok(())
            } else {
                let allowed_path = context
                    .plan_path
                    .as_ref()
                    .ok_or_else(|| "plan mode write target is not set".to_string())?;
                Err(format!(
                    "plan mode only allows writing the plan file: {}",
                    canonicalize_with_missing_tail(allowed_path).display()
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
    if context.mode == ExecutionMode::Plan {
        if let Some(path) =
            resolve_plan_mode_write_target(root, raw, context.plan_path.as_deref())?
        {
            return Ok(path);
        }
    }
    resolve_user_path(root, raw)
}

fn enforce_plan_stage_scope(name: &str, arguments: &Value, context: &ToolContext) -> Result<(), String> {
    if context.mode != ExecutionMode::Plan || !matches!(name, "Write" | "Edit") {
        return Ok(());
    }

    let raw_path = get_required_string(arguments, "path")?;
    if resolve_plan_mode_write_target(&context.root, raw_path, context.plan_path.as_deref())?
        .is_none()
    {
        return Ok(());
    }

    let payload = match name {
        "Write" => get_required_string(arguments, "content")?,
        "Edit" => get_required_string(arguments, "new_string")?,
        _ => return Ok(()),
    };

    let disallowed = disallowed_plan_sections(context.plan_stage)
        .into_iter()
        .filter(|section| payload_mentions_plan_section(payload, section))
        .collect::<Vec<_>>();
    if disallowed.is_empty() {
        return Ok(());
    }

    let allowed = allowed_plan_sections(context.plan_stage).join(", ");
    Err(format!(
        "plan stage {} only allows updating these sections now: {}. Remove later sections from this {}: {}",
        context.plan_stage.label(),
        allowed,
        name,
        disallowed.join(", ")
    ))
}

fn allowed_plan_sections(stage: PlanStage) -> &'static [&'static str] {
    match stage {
        PlanStage::Stage1 => &["Goal", "Constraints", "Deliverables"],
        PlanStage::Stage2 => &["Acceptance Criteria", "Quality Bar"],
        PlanStage::Stage3 => &["Execution Plan", "Verification Plan", "Risks / Fallbacks"],
        PlanStage::Ready => &[],
    }
}

fn disallowed_plan_sections(stage: PlanStage) -> Vec<&'static str> {
    let all = [
        "Goal",
        "Constraints",
        "Deliverables",
        "Acceptance Criteria",
        "Quality Bar",
        "Execution Plan",
        "Verification Plan",
        "Risks / Fallbacks",
    ];
    all.into_iter()
        .filter(|section| !allowed_plan_sections(stage).contains(section))
        .collect()
}

fn payload_mentions_plan_section(payload: &str, section: &str) -> bool {
    let heading = format!("## {section}");
    payload.contains(&heading)
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

    println!("Approve {name}: {summary}? [yes/no]");
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

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_with_missing_tail, enforce_plan_stage_scope, resolve_plan_mode_write_target,
        ToolContext,
    };
    use crate::modes::plan_act::{ExecutionMode, PlanStage};
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn plan_mode_write_target_accepts_same_filename_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let resolved = resolve_plan_mode_write_target(&root, "plans/plan-123.md", Some(&plan_path))
            .unwrap()
            .unwrap();
        assert_eq!(
            canonicalize_with_missing_tail(&resolved),
            canonicalize_with_missing_tail(&plan_path)
        );
    }

    #[test]
    fn plan_mode_write_target_accepts_direct_basename_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let resolved = resolve_plan_mode_write_target(&root, "plan-123.md", Some(&plan_path))
            .unwrap()
            .unwrap();
        assert_eq!(
            canonicalize_with_missing_tail(&resolved),
            canonicalize_with_missing_tail(&plan_path)
        );
    }

    #[test]
    fn plan_mode_write_target_accepts_absolute_same_filename_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let alias = root.join("plan-123.md");
        let resolved =
            resolve_plan_mode_write_target(&root, alias.to_str().unwrap(), Some(&plan_path))
                .unwrap()
                .unwrap();
        assert_eq!(
            canonicalize_with_missing_tail(&resolved),
            canonicalize_with_missing_tail(&plan_path)
        );
    }

    #[test]
    fn plan_mode_write_target_rejects_wrong_filename() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let resolved = resolve_plan_mode_write_target(&root, "plans/other.md", Some(&plan_path))
            .unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn plan_stage_scope_rejects_later_sections_in_stage1_write() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let context = ToolContext {
            root,
            mode: ExecutionMode::Plan,
            plan_path: Some(plan_path),
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let err = enforce_plan_stage_scope(
            "Write",
            &json!({
                "path": "plan-123.md",
                "content": "# Plan\n\n## Goal\n- x\n\n## Acceptance Criteria\n- y\n"
            }),
            &context,
        )
        .unwrap_err();
        assert!(err.contains("Stage 1"), "got: {err}");
        assert!(err.contains("Acceptance Criteria"), "got: {err}");
    }

    #[test]
    fn plan_stage_scope_allows_stage_local_write() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let context = ToolContext {
            root,
            mode: ExecutionMode::Plan,
            plan_path: Some(plan_path),
            plan_stage: PlanStage::Stage2,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        enforce_plan_stage_scope(
            "Edit",
            &json!({
                "path": "plan-123.md",
                "old_string": "## Acceptance Criteria\n- old\n",
                "new_string": "## Acceptance Criteria\n- new\n\n## Quality Bar\n- anchored to src/app/page.tsx\n"
            }),
            &context,
        )
        .unwrap();
    }
}
