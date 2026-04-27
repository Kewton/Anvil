use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use serde_json::Value;

use crate::modes::plan_act::{ExecutionMode, PlanStage};
use crate::safety::path_guard::resolve_user_path;
use crate::tools::bash::BashExecutionOutcome;
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
                let raw_path = get_required_string(arguments, "path")?;
                let path = resolve_plan_mode_write_target(
                    &context.root,
                    raw_path,
                    context.plan_path.as_deref(),
                )?
                .unwrap_or(resolve_user_path(&context.root, raw_path)?);
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
                if let Some(merged) =
                    plan_mode_merged_plan_contents("Write", &path, arguments, context)?
                {
                    write::run(&path, &merged)
                } else {
                    write::run(&path, content)
                }
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
                if let Some(merged) =
                    plan_mode_merged_plan_contents("Edit", &path, arguments, context)?
                {
                    write::run(&path, &merged)
                } else {
                    edit::run(&path, old, new, replace_all)
                }
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

    /// Bash-only execution path that exposes the structured
    /// `BashExecutionOutcome` alongside the formatted text result. Used by
    /// the agent layer (turn.rs) to drive FeedbackFrame generation
    /// (Issue #450 / CB-001) for Bash dispatches without changing the
    /// public `execute` contract.
    ///
    /// The `Err` arm now carries a `BashErrorClass` so the agent can
    /// distinguish the dangerous-snippet block (the only case that
    /// should be recorded as `UnsafeCommandBlocked` per design 5.2 / 11.2)
    /// from policy denials, missing arguments, and runtime failures
    /// (CB2-001).
    pub fn execute_bash_with_outcome(
        &self,
        arguments: &Value,
        context: &ToolContext,
    ) -> (
        Result<String, (String, BashErrorClass)>,
        Option<BashExecutionOutcome>,
    ) {
        if let Err(err) = enforce_mode("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ModeOrScopeDenied)), None);
        }
        if let Err(err) = enforce_plan_stage_scope("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ModeOrScopeDenied)), None);
        }
        if let Err(err) = maybe_confirm("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ApprovalDenied)), None);
        }
        let command = match get_required_string(arguments, "command") {
            Ok(c) => c,
            Err(err) => return (Err((err, BashErrorClass::MissingArgument)), None),
        };
        match bash::run_with_outcome(
            command,
            &context.root,
            context.cancel_flag.as_ref(),
            context.offline,
        ) {
            Ok((text, outcome)) => (Ok(text), Some(outcome)),
            Err(err) => {
                let class = classify_bash_dispatch_err(&err);
                (Err((err, class)), None)
            }
        }
    }
}

/// CB2-001: how a bash dispatch failed when no `BashExecutionOutcome` was
/// produced. Only `DangerousBlock` should be surfaced as
/// `FeedbackKind::UnsafeCommandBlocked` per design 5.2 / 11.2; the other
/// variants are policy / argument / runtime issues and must not be recorded
/// as security blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashErrorClass {
    /// `tools/bash.rs::is_dangerous_command` post-dispatch block (the
    /// "blocked dangerous command fragment: ..." Err string).
    DangerousBlock,
    /// `enforce_offline_policy` rejected a network / general / mutating
    /// command in offline mode.
    OfflinePolicy,
    /// `enforce_mode` or `enforce_plan_stage_scope` rejected the call.
    ModeOrScopeDenied,
    /// `maybe_confirm` rejected the call (user denial or non-interactive
    /// without `--yes`).
    ApprovalDenied,
    /// `command` argument was missing or not a string.
    MissingArgument,
    /// Anything else (failed `spawn`, failed `wait`, etc.). Not a security
    /// block.
    RuntimeFailure,
}

/// Classify an Err string returned by `bash::run_with_outcome` (i.e. a
/// pre-spawn error path that produced no `BashExecutionOutcome`). The
/// dangerous-snippet block uses a stable `"blocked dangerous command
/// fragment: ..."` prefix; offline policy uses a stable `"offline mode "`
/// prefix. Everything else is treated as a generic runtime failure.
fn classify_bash_dispatch_err(err: &str) -> BashErrorClass {
    if err.starts_with("blocked dangerous command fragment") {
        BashErrorClass::DangerousBlock
    } else if err.starts_with("offline mode ") {
        BashErrorClass::OfflinePolicy
    } else {
        BashErrorClass::RuntimeFailure
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
    if context.mode == ExecutionMode::Plan
        && let Some(path) = resolve_plan_mode_write_target(root, raw, context.plan_path.as_deref())?
    {
        return Ok(path);
    }
    resolve_user_path(root, raw)
}

fn enforce_plan_stage_scope(
    name: &str,
    arguments: &Value,
    context: &ToolContext,
) -> Result<(), String> {
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
    if name == "Edit"
        && allowed_plan_sections(context.plan_stage)
            .iter()
            .any(|section| payload_mentions_plan_section(payload, section))
    {
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
        PlanStage::Stage1 => &["Goal", "Constraints"],
        PlanStage::Stage2 => &["First Action", "Verification"],
        PlanStage::Stage3 => &[],
        PlanStage::Ready => &[],
    }
}

fn disallowed_plan_sections(stage: PlanStage) -> Vec<&'static str> {
    let all = [
        "Goal",
        "Constraints",
        "First Action",
        "Verification",
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

fn plan_mode_merged_plan_contents(
    tool_name: &str,
    path: &std::path::Path,
    arguments: &Value,
    context: &ToolContext,
) -> Result<Option<String>, String> {
    if context.mode != ExecutionMode::Plan {
        return Ok(None);
    }
    let Some(plan_path) = context.plan_path.as_deref() else {
        return Ok(None);
    };
    if canonicalize_with_missing_tail(path) != canonicalize_with_missing_tail(plan_path) {
        return Ok(None);
    }

    let payload = match tool_name {
        "Write" => get_required_string(arguments, "content")?,
        "Edit" => get_required_string(arguments, "new_string")?,
        _ => return Ok(None),
    };

    let merged = merge_plan_payload_for_stage(path, payload, context.plan_stage)?;
    Ok(Some(merged))
}

fn merge_plan_payload_for_stage(
    path: &std::path::Path,
    payload: &str,
    stage: PlanStage,
) -> Result<String, String> {
    let allowed = allowed_plan_sections(stage);
    if allowed.is_empty() {
        return Err(
            "plan is already approval ready; no further stage-scoped edits are allowed".to_string(),
        );
    }

    let payload_sections = parse_plan_sections(payload);
    let staged_updates = allowed
        .iter()
        .filter_map(|section| {
            payload_sections
                .get(*section)
                .map(|body| ((*section).to_string(), body.clone()))
        })
        .collect::<Vec<_>>();
    if staged_updates.is_empty() {
        return Err(format!(
            "plan stage {} update must include one of these sections: {}",
            stage.label(),
            allowed.join(", ")
        ));
    }

    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let existing_sections = parse_plan_sections(&existing);
    let preamble = choose_plan_preamble(&existing, payload);
    let mut merged_sections = existing_sections;
    for (section, body) in staged_updates {
        merged_sections.insert(section, body);
    }

    Ok(render_plan_document(&preamble, &merged_sections))
}

fn choose_plan_preamble(existing: &str, payload: &str) -> String {
    let existing_preamble = plan_preamble(existing);
    if !existing_preamble.trim().is_empty() {
        return existing_preamble;
    }
    let payload_preamble = plan_preamble(payload);
    if !payload_preamble.trim().is_empty() {
        return payload_preamble;
    }
    "# Plan".to_string()
}

fn plan_preamble(contents: &str) -> String {
    let lines = contents
        .lines()
        .take_while(|line| !line.trim_start().starts_with("## "))
        .collect::<Vec<_>>();
    lines.join("\n").trim().to_string()
}

fn parse_plan_sections(contents: &str) -> std::collections::BTreeMap<String, String> {
    let mut sections = std::collections::BTreeMap::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = Vec::new();

    for line in contents.lines() {
        if let Some(raw_heading) = line.trim().strip_prefix("## ") {
            if let Some(heading) = current_heading.take() {
                let body = current_body.join("\n").trim().to_string();
                if !body.is_empty() {
                    sections.insert(heading, body);
                }
                current_body.clear();
            }
            current_heading = Some(normalize_plan_heading(raw_heading).to_string());
            continue;
        }
        if current_heading.is_some() {
            current_body.push(line);
        }
    }

    if let Some(heading) = current_heading.take() {
        let body = current_body.join("\n").trim().to_string();
        if !body.is_empty() {
            sections.insert(heading, body);
        }
    }

    sections
}

fn normalize_plan_heading(heading: &str) -> &str {
    match heading.trim() {
        "Next Step" | "First Step" | "Execution Plan" | "実行計画" | "実装計画"
        | "実装フェーズ" => "First Action",
        "Verification Plan" | "検証計画" => "Verification",
        "Risks/Fallbacks" => "Risks / Fallbacks",
        "リスク/フォールバック" | "リスク・フォールバック" | "リスク / フォールバック" => {
            "Risks / Fallbacks"
        }
        other => other,
    }
}

fn render_plan_document(
    preamble: &str,
    sections: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut blocks = Vec::new();
    let trimmed_preamble = preamble.trim();
    if !trimmed_preamble.is_empty() {
        blocks.push(trimmed_preamble.to_string());
    }

    for section in [
        "Goal",
        "Constraints",
        "First Action",
        "Verification",
        "Deliverables",
        "Acceptance Criteria",
        "Quality Bar",
        "Execution Plan",
        "Verification Plan",
        "Risks / Fallbacks",
    ] {
        if let Some(body) = sections.get(section) {
            blocks.push(format!("## {section}\n{}", body.trim()));
        }
    }

    format!("{}\n", blocks.join("\n\n"))
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
        ToolContext, ToolRegistry, canonicalize_with_missing_tail, enforce_plan_stage_scope,
        resolve_plan_mode_write_target,
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
    fn plan_mode_read_accepts_absolute_same_filename_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(root.join("plans")).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        std::fs::write(&plan_path, "hello plan\n").unwrap();
        let alias = root.join("plans").join("plan-123.md");
        let registry = ToolRegistry::default();
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
        let out = registry
            .execute(
                "Read",
                &json!({"path": alias.to_string_lossy().to_string()}),
                &context,
            )
            .unwrap();
        assert!(out.contains("hello plan"), "got: {out}");
    }

    #[test]
    fn plan_mode_write_target_rejects_wrong_filename() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        let resolved =
            resolve_plan_mode_write_target(&root, "plans/other.md", Some(&plan_path)).unwrap();
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
                "old_string": "## First Action\n- old\n",
                "new_string": "## First Action\n- Edit src/app/page.tsx first.\n\n## Verification\n- Run npm test.\n"
            }),
            &context,
        )
        .unwrap();
    }

    #[test]
    fn plan_mode_stage2_write_merges_without_dropping_stage1_sections() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        std::fs::write(
            &plan_path,
            "# Plan\n\n## Goal\n- build game\n\n## Constraints\n- keep next.js\n",
        )
        .unwrap();
        let context = ToolContext {
            root,
            mode: ExecutionMode::Plan,
            plan_path: Some(plan_path.clone()),
            plan_stage: PlanStage::Stage2,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        ToolRegistry::default()
            .execute(
                "Write",
                &json!({
                    "path": "plans/plan-123.md",
                    "content": "## First Action\n- Edit src/app/page.tsx first.\n\n## Verification\n- Run npm test.\n"
                }),
                &context,
            )
            .unwrap();
        let updated = std::fs::read_to_string(&plan_path).unwrap();
        assert!(updated.contains("## Goal\n- build game"));
        assert!(updated.contains("## Constraints\n- keep next.js"));
        assert!(updated.contains("## First Action\n- Edit src/app/page.tsx first."));
        assert!(updated.contains("## Verification\n- Run npm test."));
    }

    #[test]
    fn plan_mode_stage2_edit_salvages_allowed_sections_from_full_document() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-123.md");
        std::fs::write(
            &plan_path,
            "# Plan\n\n## Goal\n- build game\n\n## Constraints\n- keep next.js\n",
        )
        .unwrap();
        let context = ToolContext {
            root,
            mode: ExecutionMode::Plan,
            plan_path: Some(plan_path.clone()),
            plan_stage: PlanStage::Stage2,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        ToolRegistry::default()
            .execute(
                "Edit",
                &json!({
                    "path": "plan-123.md",
                    "old_string": "# Plan",
                    "new_string": "# Plan\n\n## Goal\n- overwritten goal\n\n## Constraints\n- overwritten constraints\n\n## Deliverables\n- overwritten deliverables\n\n## First Action\n- Edit src/app/page.tsx first.\n\n## Verification\n- Run npm test.\n\n## Quality Bar\n- later\n"
                }),
                &context,
            )
            .unwrap();
        let updated = std::fs::read_to_string(&plan_path).unwrap();
        assert!(updated.contains("## Goal\n- build game"));
        assert!(updated.contains("## Constraints\n- keep next.js"));
        assert!(updated.contains("## First Action\n- Edit src/app/page.tsx first."));
        assert!(updated.contains("## Verification\n- Run npm test."));
        assert!(!updated.contains("overwritten goal"));
        assert!(!updated.contains("## Quality Bar\n- later"));
    }

    /// CB-001: `execute_bash_with_outcome` exposes the structured
    /// `BashExecutionOutcome` alongside the formatted text result so the
    /// agent layer can record a FeedbackFrame without losing the existing
    /// `Result<String, String>` contract.
    #[test]
    fn execute_bash_with_outcome_returns_structured_result() {
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "printf hello"}), &context);
        let text = text_result.expect("ok");
        let outcome = outcome.expect("structured outcome");
        assert!(text.starts_with("exit_code=0"));
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("hello"));
    }

    /// CB-001: a dangerous bash command (`rm -rf /`) is rejected pre-spawn
    /// and the structured outcome is `None` (no child process ran). The
    /// caller in turn.rs treats that as `UnsafeCommandBlocked`.
    #[test]
    fn execute_bash_with_outcome_returns_no_outcome_for_dangerous_block() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "rm -rf /"}), &context);
        let (_msg, class) = text_result.expect_err("rm -rf / must be blocked");
        assert_eq!(class, BashErrorClass::DangerousBlock);
        assert!(outcome.is_none());
    }

    /// CB2-001: a missing `command` argument classifies as MissingArgument,
    /// not DangerousBlock — turn.rs must not record UnsafeCommandBlocked
    /// for malformed tool calls.
    #[test]
    fn execute_bash_with_outcome_classifies_missing_command_as_missing_argument() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let (text_result, outcome) = registry.execute_bash_with_outcome(&json!({}), &context);
        let (_msg, class) = text_result.expect_err("missing command must error");
        assert_eq!(class, BashErrorClass::MissingArgument);
        assert!(outcome.is_none());
    }

    /// CB2-001: an approval denial (non-interactive without --yes) classifies
    /// as ApprovalDenied — must not record UnsafeCommandBlocked.
    #[test]
    fn execute_bash_with_outcome_classifies_approval_denial() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: false,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "ls"}), &context);
        let (_msg, class) = text_result.expect_err("approval denial must error");
        assert_eq!(class, BashErrorClass::ApprovalDenied);
        assert!(outcome.is_none());
    }

    /// CB2-001: an offline-policy denial classifies as OfflinePolicy — must
    /// not record UnsafeCommandBlocked.
    #[test]
    fn execute_bash_with_outcome_classifies_offline_policy() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: true,
            cancel_flag: None,
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "curl example.com"}), &context);
        let (_msg, class) = text_result.expect_err("offline policy must block curl");
        assert_eq!(class, BashErrorClass::OfflinePolicy);
        assert!(outcome.is_none());
    }

    /// CB2-001: a Plan-mode Bash dispatch is rejected by `enforce_mode`
    /// (Plan mode only allows Read / Glob / Grep / plan-file edits).
    #[test]
    fn execute_bash_with_outcome_classifies_plan_mode_denial() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Plan,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "ls"}), &context);
        let (_msg, class) = text_result.expect_err("plan mode must reject Bash");
        assert_eq!(class, BashErrorClass::ModeOrScopeDenied);
        assert!(outcome.is_none());
    }
}
