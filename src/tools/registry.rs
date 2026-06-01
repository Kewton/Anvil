use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use serde_json::Value;

use crate::modes::plan_act::{ExecutionMode, PlanStage};
use crate::safety::path_guard::resolve_user_path;
use crate::tools::bash::BashExecutionOutcome;
use crate::tools::{bash, edit, glob, grep, read, write};
use crate::util::workspace_paths::WorkspacePolicy;

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
    /// Issue #458: when set, paths starting with `tmp-tests/<rel>` resolve
    /// against `<this>/files/` instead of `root`. `None` means tmp-tests is
    /// not yet bound (CLI startup, unit tests); the prefix is then rejected
    /// rather than flowing to the workspace root.
    pub tmp_tests_root: Option<std::path::PathBuf>,
    /// Issue #459 / DR1-014: while a Tester turn is in flight, Edit/Write
    /// must only target the session-scoped `tmp-tests/` namespace. The
    /// registry enforces this at dispatch time via
    /// [`ToolContext::enforce_tmp_tests_only_when_active`] before the raw
    /// path is resolved (keeping the check on the raw `tmp-tests/<rel>`
    /// prefix rather than the post-resolution absolute path).
    pub tester_active: bool,
    /// Shared workspace policy for hiding controller metadata from ordinary
    /// model reads/discovery. Log-analysis tasks may opt into protected
    /// metadata reads, but writes stay blocked elsewhere.
    pub workspace_policy: WorkspacePolicy,
}

impl ToolContext {
    /// Issue #459 / DR1-014: when `tester_active` is true, reject any
    /// Edit/Write whose raw path is not under the `tmp-tests/` prefix.
    /// The check runs on the raw textual prefix (DR3-002) so it must be
    /// invoked before `resolve_tmp_tests_path` — after resolution the
    /// path becomes the absolute session-scoped form
    /// (`<state_root>/sessions/<id>/tmp-tests/files/<rel>`) and a textual
    /// `tmp-tests/` prefix check would falsely reject valid writes.
    ///
    /// `tester_active = false` is a no-op so the existing tool dispatch
    /// path is unchanged for non-Tester turns.
    pub fn enforce_tmp_tests_only_when_active(&self, path: &std::path::Path) -> Result<(), String> {
        if !self.tester_active {
            return Ok(());
        }
        let as_str = path.to_string_lossy();
        if as_str.starts_with("tmp-tests/") {
            return Ok(());
        }
        Err("Tester Skill 起動中は tmp-tests/ 以外への Edit/Write は拒否されます".to_string())
    }
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
        // Issue #461 / DR3-002: For Bash, run preflight (typed `check_blocked_command`
        // helper) before mode/scope/approval. This makes the dispatch behavior
        // identical between `execute` and `execute_bash_with_outcome` (the agent
        // layer entry point), and lets the user see the policy block reason
        // before being asked to approve.
        if name == "Bash"
            && let Some(err) = preflight_bash_command(arguments)
        {
            return Err(err);
        }

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
                let path = if let Some(tmp_path) = resolve_tmp_tests_path(raw_path, context)? {
                    tmp_path
                } else {
                    resolve_plan_mode_write_target(
                        &context.root,
                        raw_path,
                        context.plan_path.as_deref(),
                    )?
                    .unwrap_or(resolve_user_path(&context.root, raw_path)?)
                };
                let start_line = get_optional_usize(arguments, "start_line");
                let end_line = get_optional_usize(arguments, "end_line");
                enforce_workspace_read_policy(&context.root, &path, context.workspace_policy)?;
                read::run(
                    &context.root,
                    &path,
                    start_line,
                    end_line,
                    context.workspace_policy,
                )
            }
            "Write" => {
                let raw_path = get_required_string(arguments, "path")?;
                // Issue #459 / DR1-014 / DR3-002: enforce on the raw textual
                // path BEFORE `resolve_write_path` — after `resolve_tmp_tests_path`
                // resolves to the absolute session-scoped form, the textual
                // `tmp-tests/` prefix is gone.
                context.enforce_tmp_tests_only_when_active(std::path::Path::new(raw_path))?;
                let path = resolve_write_path(&context.root, raw_path, context)?;
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
                let raw_path = get_required_string(arguments, "path")?;
                // Issue #459 / DR1-014 / DR3-002: see Write branch above.
                context.enforce_tmp_tests_only_when_active(std::path::Path::new(raw_path))?;
                let path = resolve_write_path(&context.root, raw_path, context)?;
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
                glob::run(&context.root, pattern, context.workspace_policy)
            }
            "Grep" => {
                let pattern = get_required_string(arguments, "pattern")?;
                let glob = arguments.get("glob").and_then(Value::as_str);
                let case_sensitive = arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                grep::run(
                    &context.root,
                    pattern,
                    glob,
                    case_sensitive,
                    context.workspace_policy,
                )
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
        // Issue #461 / DR3-001 / DR3-002 / DR2-004: command argument is the
        // first thing we extract so the typed `check_blocked_command` preflight
        // can run before mode/scope/approval checks. This means a `command`-
        // missing call now returns `MissingArgument` before `ModeOrScopeDenied`
        // (regression-pinned by `execute_bash_with_outcome_classifies_missing_argument_takes_precedence_over_mode`).
        let command = match get_required_string(arguments, "command") {
            Ok(c) => c,
            Err(err) => return (Err((err, BashErrorClass::MissingArgument)), None),
        };
        if let Some(reason) = bash::check_blocked_command(command) {
            return (
                Err((
                    bash::render_block_error(&reason),
                    BashErrorClass::DangerousBlock,
                )),
                None,
            );
        }
        if let Err(err) = enforce_mode("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ModeOrScopeDenied)), None);
        }
        if let Err(err) = enforce_plan_stage_scope("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ModeOrScopeDenied)), None);
        }
        if let Err(err) = maybe_confirm("Bash", arguments, context) {
            return (Err((err, BashErrorClass::ApprovalDenied)), None);
        }
        match bash::run_with_outcome(
            command,
            &context.root,
            context.cancel_flag.as_ref(),
            context.offline,
            // Issue #459: only the Tester smoke runner sets an explicit
            // timeout; the regular registry-driven Bash path keeps the
            // existing `likely_long_running_command` heuristic.
            None,
            // CB-003: registry-driven Bash inherits the parent env (existing
            // behaviour). Tester's smoke runner sets `TesterSanitized`.
            None,
        ) {
            Ok((text, outcome)) => (Ok(text), Some(outcome)),
            Err(err) => {
                let class = classify_bash_dispatch_err(&err);
                (Err((err, class)), None)
            }
        }
    }
}

/// Issue #461 / DR3-002: Shared preflight for the `execute` Bash path.
/// Returns `Some(err)` when the command should be blocked before any
/// mode/scope/approval check runs. The error string is rendered via
/// `bash::render_block_error` so it carries the `"blocked dangerous command
/// fragment: …"` prefix and is mapped back to
/// `BashErrorClass::DangerousBlock` by `classify_bash_dispatch_err` when
/// the agent layer routes through `execute_bash_with_outcome`.
fn preflight_bash_command(arguments: &Value) -> Option<String> {
    let command = arguments.get("command")?.as_str()?;
    let reason = bash::check_blocked_command(command)?;
    Some(bash::render_block_error(&reason))
}

fn enforce_workspace_read_policy(
    root: &std::path::Path,
    path: &std::path::Path,
    workspace_policy: WorkspacePolicy,
) -> Result<(), String> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let relative = path
        .strip_prefix(&canonical_root)
        .or_else(|_| path.strip_prefix(root));
    let Ok(relative) = relative else {
        return Ok(());
    };
    if relative.as_os_str().is_empty() || workspace_policy.allows_model_read_relative_path(relative)
    {
        return Ok(());
    }
    Err(format!(
        "protected workspace metadata rejected Read; explicit log-analysis permission is required: {}",
        relative.display()
    ))
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
    // Issue #458: tmp-tests/ prefix takes precedence over Plan mode plan-file
    // routing — tmp-tests is a session-scoped namespace independent of the
    // plan file constraint. Plan mode + tmp-tests/<rel> writes are still
    // routed to the session's tmp-tests root.
    if let Some(path) = resolve_tmp_tests_path(raw, context)? {
        return Ok(path);
    }
    if context.mode == ExecutionMode::Plan
        && let Some(path) = resolve_plan_mode_write_target(root, raw, context.plan_path.as_deref())?
    {
        return Ok(path);
    }
    resolve_user_path(root, raw)
}

/// Issue #458: resolve a `tmp-tests/<rel>` request to the session-scoped
/// tmp-tests files root. Returns:
///   * `Ok(Some(path))` when `raw` starts with `tmp-tests/` and a tmp-tests
///     root is bound (`context.tmp_tests_root.is_some()`).
///   * `Err(...)` when the prefix is used but no root is bound, OR when the
///     literal `"tmp-tests"` (no trailing slash) is passed (which would
///     otherwise be ambiguous with a workspace-root `tmp-tests` entry).
///   * `Ok(None)` when `raw` is unrelated to tmp-tests; the caller falls
///     through to its existing path resolution.
pub(crate) fn resolve_tmp_tests_path(
    raw: &str,
    context: &ToolContext,
) -> Result<Option<std::path::PathBuf>, String> {
    if let Some(rel) = raw.strip_prefix("tmp-tests/") {
        let tmp_root = context
            .tmp_tests_root
            .as_ref()
            .ok_or_else(|| "tmp-tests root is unavailable in this context".to_string())?;
        let files_root = tmp_root.join("files");
        std::fs::create_dir_all(&files_root).map_err(|err| {
            format!(
                "failed to prepare tmp-tests files dir {}: {err}",
                files_root.display()
            )
        })?;
        let resolved = resolve_user_path(&files_root, rel)?;
        return Ok(Some(resolved));
    }
    if raw == "tmp-tests" {
        return Err("tmp-tests/ must be followed by a relative path".to_string());
    }
    Ok(None)
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
    use crate::util::workspace_paths::WorkspacePolicy;
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "ls"}), &context);
        let (_msg, class) = text_result.expect_err("plan mode must reject Bash");
        assert_eq!(class, BashErrorClass::ModeOrScopeDenied);
        assert!(outcome.is_none());
    }

    // ----- Issue #458: resolve_tmp_tests_path / tmp-tests prefix routing -----

    fn act_context_with_tmp(
        root: &std::path::Path,
        tmp_tests_root: Option<&std::path::Path>,
    ) -> ToolContext {
        ToolContext {
            root: root.to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: tmp_tests_root.map(|p| p.to_path_buf()),
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        }
    }

    #[test]
    fn resolve_tmp_tests_path_returns_none_for_unrelated_paths() {
        let temp = tempdir().unwrap();
        let ctx = act_context_with_tmp(temp.path(), None);
        let out = super::resolve_tmp_tests_path("src/main.rs", &ctx).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn resolve_tmp_tests_path_routes_prefix_to_files_root() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = act_context_with_tmp(&workspace, Some(&tmp_tests));
        let out = super::resolve_tmp_tests_path("tmp-tests/src/test_foo.rs", &ctx)
            .unwrap()
            .expect("Some(path) for prefixed input");
        let expected = std::fs::canonicalize(tmp_tests.join("files")).unwrap();
        assert!(out.starts_with(&expected), "got {}", out.display());
        assert!(out.ends_with("src/test_foo.rs"));
    }

    #[test]
    fn resolve_tmp_tests_path_rejects_when_root_unbound() {
        let temp = tempdir().unwrap();
        let ctx = act_context_with_tmp(temp.path(), None);
        let err = super::resolve_tmp_tests_path("tmp-tests/x.rs", &ctx).unwrap_err();
        assert!(err.contains("unavailable"), "got: {err}");
    }

    #[test]
    fn resolve_tmp_tests_path_rejects_bare_prefix_without_slash() {
        let temp = tempdir().unwrap();
        let tmp_tests = temp.path().join("tmp-tests");
        let ctx = act_context_with_tmp(temp.path(), Some(&tmp_tests));
        let err = super::resolve_tmp_tests_path("tmp-tests", &ctx).unwrap_err();
        assert!(err.contains("relative path"), "got: {err}");
    }

    #[test]
    fn resolve_tmp_tests_path_rejects_path_traversal() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = act_context_with_tmp(&workspace, Some(&tmp_tests));
        let err = super::resolve_tmp_tests_path("tmp-tests/../escape.rs", &ctx).unwrap_err();
        assert!(err.contains("escape") || err.contains(".."), "got: {err}");
    }

    #[test]
    fn write_with_tmp_tests_prefix_does_not_leak_to_workspace() {
        // Issue #458 acceptance #4 (create-direction regression):
        // a Write tmp-tests/<rel> must NOT create a file under workspace root.
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("state/tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: workspace.clone(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: Some(tmp_tests.clone()),
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        registry
            .execute(
                "Write",
                &json!({"path": "tmp-tests/src/test_foo.rs", "content": "#[test] fn it(){}"}),
                &context,
            )
            .unwrap();
        // workspace is clean
        assert!(!workspace.join("src/test_foo.rs").exists());
        assert!(!workspace.join("tmp-tests").exists());
        // tmp-tests has the body
        assert!(tmp_tests.join("files/src/test_foo.rs").exists());
    }

    #[test]
    fn write_with_tmp_tests_prefix_when_root_is_none_is_rejected() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: workspace.clone(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let err = registry
            .execute(
                "Write",
                &json!({"path": "tmp-tests/x.rs", "content": "x"}),
                &context,
            )
            .unwrap_err();
        assert!(err.contains("unavailable"), "got: {err}");
    }

    #[test]
    fn read_with_tmp_tests_prefix_reads_from_files_root() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("state/tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(tmp_tests.join("files/src")).unwrap();
        std::fs::write(tmp_tests.join("files/src/foo.rs"), b"hello tmp").unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: workspace,
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: Some(tmp_tests),
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let out = registry
            .execute("Read", &json!({"path": "tmp-tests/src/foo.rs"}), &context)
            .unwrap();
        assert!(out.contains("hello tmp"), "got: {out}");
    }

    // ----- Issue #459: tester_active path confinement ---------------------

    fn act_context_with_tester(
        root: &std::path::Path,
        tmp_tests_root: &std::path::Path,
        tester_active: bool,
    ) -> ToolContext {
        ToolContext {
            root: root.to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: Some(tmp_tests_root.to_path_buf()),
            tester_active,
            workspace_policy: WorkspacePolicy::default(),
        }
    }

    /// `tester_active = true` rejects an Edit/Write whose raw path is not
    /// under `tmp-tests/`.
    #[test]
    fn enforce_tmp_tests_only_when_active_rejects_workspace_path() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, true);
        let err = ctx
            .enforce_tmp_tests_only_when_active(std::path::Path::new("src/foo.rs"))
            .unwrap_err();
        assert!(err.contains("tmp-tests/"), "got: {err}");
    }

    /// `tester_active = true` accepts an Edit/Write under `tmp-tests/<rel>`.
    #[test]
    fn enforce_tmp_tests_only_when_active_accepts_tmp_tests_prefix() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, true);
        ctx.enforce_tmp_tests_only_when_active(std::path::Path::new("tmp-tests/files/smoke.rs"))
            .expect("tmp-tests prefix must be accepted while Tester is active");
    }

    /// `tester_active = false` is a no-op even for non-tmp-tests paths,
    /// preserving existing behaviour for normal turns.
    #[test]
    fn enforce_tmp_tests_only_when_active_noop_when_inactive() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, false);
        ctx.enforce_tmp_tests_only_when_active(std::path::Path::new("src/foo.rs"))
            .expect("inactive Tester must not block normal writes");
    }

    /// End-to-end: with `tester_active = true`, `registry.execute("Write", …)`
    /// rejects a non-`tmp-tests/` path before it even resolves the path.
    #[test]
    fn execute_write_rejects_non_tmp_tests_when_tester_active() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let registry = ToolRegistry::default();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, true);
        let err = registry
            .execute(
                "Write",
                &json!({"path": "src/foo.rs", "content": "x"}),
                &ctx,
            )
            .unwrap_err();
        assert!(err.contains("tmp-tests/"), "got: {err}");
        // workspace must remain clean (no file leaked through dispatch).
        assert!(!workspace.join("src/foo.rs").exists());
    }

    /// End-to-end: with `tester_active = true`, `registry.execute("Write", …)`
    /// still routes a `tmp-tests/<rel>` write to the session-scoped files
    /// root via `resolve_tmp_tests_path`.
    #[test]
    fn execute_write_allows_tmp_tests_prefix_when_tester_active() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("state/tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        let registry = ToolRegistry::default();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, true);
        registry
            .execute(
                "Write",
                &json!({
                    "path": "tmp-tests/smoke.rs",
                    "content": "#[test] fn smoke() {}",
                }),
                &ctx,
            )
            .expect("tmp-tests/<rel> must succeed while Tester is active");
        // Resolved into <tmp_tests>/files/<rel>. macOS prefixes /var with
        // /private during canonicalization, so compare via canonicalize.
        let expected = std::fs::canonicalize(tmp_tests.join("files"))
            .unwrap()
            .join("smoke.rs");
        assert!(
            expected.is_file(),
            "expected {} to exist after Write",
            expected.display()
        );
        // Workspace untouched.
        assert!(!workspace.join("smoke.rs").exists());
    }

    /// `Read` is unaffected by `tester_active`: the confinement is for
    /// Edit/Write only (Tester still needs to read repo state to construct
    /// its smoke prompt).
    #[test]
    fn execute_read_unaffected_by_tester_active() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let tmp_tests = temp.path().join("tmp-tests");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("README.md"), b"hello readme").unwrap();
        let registry = ToolRegistry::default();
        let ctx = act_context_with_tester(&workspace, &tmp_tests, true);
        let out = registry
            .execute("Read", &json!({"path": "README.md"}), &ctx)
            .expect("Read must work even when Tester is active");
        assert!(out.contains("hello readme"));
    }

    // ---- Issue #461: shared preflight regression tests --------------

    /// Issue #461: a new destructive pattern (`shutdown`) is blocked by
    /// the shared preflight in `execute_bash_with_outcome` and surfaces
    /// as `BashErrorClass::DangerousBlock`, even when approval would
    /// otherwise be required.
    #[test]
    fn execute_bash_with_outcome_preflights_blocked_command_before_approval() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            mode: ExecutionMode::Act,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            // auto_approve = false so that without the preflight the
            // call would otherwise return ApprovalDenied. The preflight
            // must run first and return DangerousBlock instead.
            auto_approve: false,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "shutdown -h now"}), &context);
        let (msg, class) = text_result.expect_err("shutdown must be blocked");
        assert_eq!(class, BashErrorClass::DangerousBlock);
        assert!(outcome.is_none());
        assert!(
            msg.starts_with("blocked dangerous command fragment: "),
            "got: {msg}"
        );
        assert!(msg.contains("shutdown"));
    }

    /// Issue #461 / DR3-001: preflight signature contract — when the
    /// preflight matches, the result is `Err((rendered, DangerousBlock))`
    /// and the outcome is `None`.
    #[test]
    fn execute_bash_with_outcome_preflight_returns_dangerous_block_class_with_none_outcome() {
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let (text_result, outcome) =
            registry.execute_bash_with_outcome(&json!({"command": "iptables -F"}), &context);
        let (_msg, class) = text_result.expect_err("iptables blocked");
        assert_eq!(class, BashErrorClass::DangerousBlock);
        assert!(outcome.is_none());
    }

    /// Issue #461 / DR3-002: `ToolRegistry::execute` Bash path also
    /// rejects new destructive patterns via the same shared preflight
    /// (no policy drift between `execute` and `execute_bash_with_outcome`).
    #[test]
    fn execute_bash_path_blocks_destructive_command() {
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
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let err = registry
            .execute("Bash", &json!({"command": "reboot"}), &context)
            .expect_err("reboot blocked via execute path");
        assert!(
            err.starts_with("blocked dangerous command fragment: "),
            "got: {err}"
        );
        assert!(err.contains("reboot"));
    }

    /// Issue #461 / DR2-004: preflight insertion moved `command` extraction
    /// before mode/scope checks. A Plan-mode call missing the `command`
    /// argument now returns `MissingArgument` first, not
    /// `ModeOrScopeDenied`. This pin documents the intentional behavior.
    #[test]
    fn execute_bash_with_outcome_classifies_missing_argument_takes_precedence_over_mode() {
        use super::BashErrorClass;
        let temp = tempdir().unwrap();
        let registry = ToolRegistry::default();
        let context = ToolContext {
            root: temp.path().to_path_buf(),
            // Plan-mode would normally reject Bash dispatch with
            // ModeOrScopeDenied, but the missing-argument path runs
            // before mode/scope after Issue #461.
            mode: ExecutionMode::Plan,
            plan_path: None,
            plan_stage: PlanStage::Stage1,
            auto_approve: true,
            interactive_approval: false,
            offline: false,
            cancel_flag: None,
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy: WorkspacePolicy::default(),
        };
        let (text_result, outcome) = registry.execute_bash_with_outcome(&json!({}), &context);
        let (_msg, class) = text_result.expect_err("missing command must error");
        assert_eq!(class, BashErrorClass::MissingArgument);
        assert!(outcome.is_none());
    }
}
