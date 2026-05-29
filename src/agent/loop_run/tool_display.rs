//! Per-iteration progress line rendering extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts `ProgressDisplay`, `progress_path_display`, `tool_display` (entry
//! point) and the per-tool projection helpers (`tool_display_{write, edit,
//! read, plan_read, workspace_read, bash, search, default}`, plus
//! `tool_display_preview_note`, `tool_display_str_arg`, `summarize_plan_read`,
//! `read_line_suffix`, `text_preview`, `read_preview`, `compact_progress_path`).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use crate::modes::plan_act::PlanStage;
use crate::safety::path_guard::resolve_user_path;

use super::lifecycle;
use super::progress_text::{sanitize_for_progress, truncate};
use super::turn::join_sections_for_progress;

/// Returns `(display_str, extra)` for the progress line. `display_str` is the
/// main single-line description (path / command / pattern); `extra` is an
/// optional parenthesized suffix (e.g. `"5B"` for Write byte count). Paths are
/// made relative to `work_root` when possible. All model-derived strings pass
/// through `sanitize_for_progress` to prevent terminal injection.
///
/// `arg_budget` caps the Bash command display length (issue #432). Other tool
/// arms currently ignore this budget; the uniform signature lets the caller
/// compute the budget once via `progress_available_width`.
pub(super) struct ProgressDisplay {
    pub(super) action: String,
    pub(super) path: Option<String>,
    pub(super) note: Option<String>,
    pub(super) status: Option<String>,
}

pub(super) fn progress_path_display(
    raw_path: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
    max_chars: usize,
) -> String {
    if raw_path.is_empty() {
        return "<missing path>".to_string();
    }
    if super::actor_loop_flow::plan_path_matches(raw_path, work_root, plan_path) {
        return compact_progress_path(
            &sanitize_for_progress(&plan_path.unwrap().display().to_string()),
            max_chars,
        );
    }
    let relative = std::path::Path::new(raw_path)
        .strip_prefix(work_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| raw_path.to_string());
    compact_progress_path(&sanitize_for_progress(&relative), max_chars)
}

pub(super) fn tool_display(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &std::path::Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
    arg_budget: usize,
) -> ProgressDisplay {
    let raw_path = tool_display_str_arg(arguments, "path");
    let path_display = progress_path_display(raw_path, work_root, plan_path, arg_budget.max(48));
    match tool_name {
        "Write" => tool_display_write(
            arguments,
            raw_path,
            path_display,
            work_root,
            plan_path,
            current_stage,
        ),
        "Edit" => tool_display_edit(
            arguments,
            raw_path,
            path_display,
            work_root,
            plan_path,
            current_stage,
        ),
        "Read" => tool_display_read(
            arguments,
            raw_path,
            path_display,
            work_root,
            plan_path,
            current_stage,
            arg_budget,
        ),
        "Bash" => tool_display_bash(arguments, arg_budget),
        "Glob" | "Grep" => tool_display_search(arguments, arg_budget),
        _ => tool_display_default(tool_name, arg_budget),
    }
}

fn tool_display_str_arg<'a>(arguments: &'a serde_json::Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn tool_display_write(
    arguments: &serde_json::Value,
    raw_path: &str,
    path_display: String,
    work_root: &Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> ProgressDisplay {
    let content = tool_display_str_arg(arguments, "content");
    let (action, note, status) =
        if super::actor_loop_flow::plan_path_matches(raw_path, work_root, plan_path) {
            let summary = super::actor_loop_flow::summarize_plan_write(
                "Write",
                raw_path,
                content,
                work_root,
                plan_path,
                current_stage,
            );
            (summary.action, summary.note, summary.status)
        } else {
            (
                "Write file".to_string(),
                tool_display_preview_note(content),
                Some(format!("{}B", content.len())),
            )
        };
    ProgressDisplay {
        action,
        path: Some(path_display),
        note,
        status,
    }
}

fn tool_display_edit(
    arguments: &serde_json::Value,
    raw_path: &str,
    path_display: String,
    work_root: &Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> ProgressDisplay {
    let new_text = tool_display_str_arg(arguments, "new_string");
    let (action, note, status) =
        if super::actor_loop_flow::plan_path_matches(raw_path, work_root, plan_path) {
            let summary = super::actor_loop_flow::summarize_plan_write(
                "Edit",
                raw_path,
                new_text,
                work_root,
                plan_path,
                current_stage,
            );
            (summary.action, summary.note, summary.status)
        } else {
            (
                "Revise file".to_string(),
                tool_display_preview_note(new_text),
                None,
            )
        };
    ProgressDisplay {
        action,
        path: Some(path_display),
        note,
        status,
    }
}

fn tool_display_preview_note(contents: &str) -> Option<String> {
    let preview = text_preview(contents, 72);
    (!preview.is_empty()).then(|| format!("Preview: {preview}"))
}

fn tool_display_read(
    arguments: &serde_json::Value,
    raw_path: &str,
    path_display: String,
    work_root: &Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
    arg_budget: usize,
) -> ProgressDisplay {
    let line_suffix = read_line_suffix(arguments);
    let path = format!("{path_display}{line_suffix}");
    let (action, note, status) =
        if super::actor_loop_flow::plan_path_matches(raw_path, work_root, plan_path) {
            tool_display_plan_read(plan_path, current_stage)
        } else {
            tool_display_workspace_read(raw_path, work_root, &line_suffix)
        };
    ProgressDisplay {
        action,
        path: Some(compact_progress_path(&path, arg_budget.max(48))),
        note,
        status,
    }
}

fn tool_display_plan_read(
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> (String, Option<String>, Option<String>) {
    let contents = plan_path
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let (action, note) = summarize_plan_read(&contents);
    let status = if lifecycle::current_plan_stage(&contents) == PlanStage::Ready {
        Some("Approval ready".to_string())
    } else {
        let next = lifecycle::plan_next_stage_sections(&contents);
        (!next.is_empty()).then(|| {
            format!(
                "Current phase: {}",
                super::actor_loop_flow::plan_phase_from_sections(&next, current_stage, false)
            )
        })
    };
    (action, note, status)
}

fn tool_display_workspace_read(
    raw_path: &str,
    work_root: &Path,
    line_suffix: &str,
) -> (String, Option<String>, Option<String>) {
    let (preview, extra) = read_preview(raw_path, work_root);
    let action = if line_suffix.is_empty() {
        "Read file".to_string()
    } else {
        format!("Read lines {}", line_suffix.trim_start_matches(':'))
    };
    (
        action,
        preview.map(|preview| format!("Preview: {preview}")),
        extra,
    )
}

fn tool_display_bash(arguments: &serde_json::Value, arg_budget: usize) -> ProgressDisplay {
    let sanitized = sanitize_for_progress(tool_display_str_arg(arguments, "command"));
    ProgressDisplay {
        action: format!("Run {}", truncate(&sanitized, arg_budget.saturating_sub(4))),
        path: None,
        note: None,
        status: None,
    }
}

fn tool_display_search(arguments: &serde_json::Value, arg_budget: usize) -> ProgressDisplay {
    ProgressDisplay {
        action: format!(
            "Search {}",
            truncate(
                &sanitize_for_progress(tool_display_str_arg(arguments, "pattern")),
                arg_budget.saturating_sub(7)
            )
        ),
        path: None,
        note: None,
        status: None,
    }
}

fn tool_display_default(tool_name: &str, arg_budget: usize) -> ProgressDisplay {
    ProgressDisplay {
        action: truncate(&sanitize_for_progress(tool_name), arg_budget),
        path: None,
        note: None,
        status: None,
    }
}

fn summarize_plan_read(contents: &str) -> (String, Option<String>) {
    let missing = lifecycle::plan_missing_sections(contents);
    if missing.is_empty() {
        (
            "Review completed plan".to_string(),
            Some("Approval ready; review the final plan before /approve".to_string()),
        )
    } else {
        let next = lifecycle::plan_next_stage_sections(contents);
        let note = if !next.is_empty() {
            format!("Next focus: {}", join_sections_for_progress(&next))
        } else {
            format!("Missing: {}", join_sections_for_progress(&missing))
        };
        ("Review plan draft".to_string(), Some(note))
    }
}

fn read_line_suffix(arguments: &serde_json::Value) -> String {
    let start = arguments
        .get("start_line")
        .and_then(serde_json::Value::as_u64);
    let end = arguments
        .get("end_line")
        .and_then(serde_json::Value::as_u64);
    match (start, end) {
        (Some(start), Some(end)) if start == end => format!(":{start}"),
        (Some(start), Some(end)) => format!(":{start}-{end}"),
        (Some(start), None) => format!(":{start}-"),
        (None, Some(end)) => format!(":1-{end}"),
        (None, None) => String::new(),
    }
}

fn text_preview(text: &str, max_chars: usize) -> String {
    let collapsed = sanitize_for_progress(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&collapsed, max_chars)
}

fn read_preview(raw_path: &str, work_root: &Path) -> (Option<String>, Option<String>) {
    if raw_path.is_empty() {
        return (None, None);
    }
    let Ok(path) = resolve_user_path(work_root, raw_path) else {
        return (None, None);
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return (None, None);
    };
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return (None, None);
    }
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return (None, None);
    };
    let preview = text_preview(&contents, 50);
    (
        (!preview.is_empty()).then_some(preview),
        Some(format!("{}B", metadata.len())),
    )
}

fn compact_progress_path(path: &str, max_chars: usize) -> String {
    let char_count = path.chars().count();
    if char_count <= max_chars {
        return path.to_string();
    }
    if let Some((_, suffix)) = path.rsplit_once("/plans/") {
        let collapsed = format!(".../plans/{suffix}");
        if collapsed.chars().count() <= max_chars {
            return collapsed;
        }
    }
    let keep = max_chars.saturating_sub(3);
    let tail = path
        .chars()
        .rev()
        .take(keep)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}
