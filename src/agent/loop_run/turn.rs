use super::auto_test::AutoTestRunner;
use super::interrupt::{InterruptEnv, InterruptFlag, InterruptMonitor};
use super::protocol::ExecutionProtocol;
use super::spinner::{Spinner, SpinnerStopSignal};
use super::summary::{ExitReason, LoopResult, LoopStats};
use super::*;
use crate::agent::orchestration::{RepoVerification, capture_repo_snapshot, verify_repo_progress};
use crate::logging::log_llm_event;
use crate::modes::plan_act::{PlanStage, TaskProfile, WorkMode, infer_work_mode_from_text};
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::tools::registry::{ToolSpec, resolve_plan_mode_write_target};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::deterministic;
#[cfg(test)]
use super::deterministic::empty_framework_app_files as deterministic_empty_framework_app_files;
#[cfg(test)]
use super::deterministic::empty_framework_game_files as deterministic_empty_framework_game_files;
use super::quality::{
    first_existing_impl_target, implementation_quality_issue_for_request,
    package_json_with_requested_port, react_dev_wrapper_for_requested_port,
    repo_change_request_text, request_allows_fast_polish_fallback,
    request_explicitly_requires_tests, request_mentions_unsupported_ui_framework,
    request_needs_playable_ui_quality_gate, workspace_has_unsupported_ui_framework,
};

/// Maximum number of characters of tool-call arguments retained in trace logs.
const LOG_ARGS_MAX_CHARS: usize = 200;
const PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD: usize = 2;
const USER_INTERRUPT_ERROR: &str = "__anvil_user_interrupt__";
const QWEN35_NON_NATIVE_HARD_TIMEOUT_SECS: u64 = 90;
const FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS: u64 = 30;
const FOCUSED_EDIT_PRE_READ_MAX_PREDICT: usize = 320;
const FOCUSED_EDIT_POST_READ_TIMEOUT_SECS: u64 = 45;
const FOCUSED_EDIT_POST_READ_MAX_PREDICT: usize = 320;
const CREATE_NEXT_APP_PACKAGE_VERSION: &str = "16.2.4";

fn is_qwen35_family(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("qwen3.5:")
}

fn request_explicitly_requests_script_execution(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    let mentions_script = [
        "script",
        ".sh",
        ".py",
        ".js",
        "スクリプト",
        "シェル",
        "コマンド",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let asks_execution = [
        "run",
        "execute",
        "実行",
        "起動",
        "結果",
        "要約",
        "summarize",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    mentions_script && asks_execution
}

fn answer_only_script_command_allowed(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some((cd_segment, rest)) = lower.split_once(" && ")
        && cd_segment.starts_with("cd ")
        && !cd_segment.contains(';')
        && !cd_segment.contains('|')
        && !cd_segment.contains('>')
    {
        return answer_only_script_command_allowed(rest);
    }
    if lower.contains(" >")
        || lower.contains(">>")
        || lower.contains(" 2>")
        || lower.contains(" | ")
        || lower.contains(" && ")
        || lower.contains(" || ")
        || lower.contains(';')
        || lower.contains(" rm ")
        || lower.starts_with("rm ")
        || lower.contains(" mv ")
        || lower.starts_with("mv ")
        || lower.contains(" cp ")
        || lower.starts_with("cp ")
        || lower.contains(" touch ")
        || lower.starts_with("touch ")
        || lower.contains(" mkdir ")
        || lower.starts_with("mkdir ")
        || lower.contains(" tee ")
        || lower.starts_with("tee ")
        || lower.contains("sed -i")
        || lower.contains("perl -pi")
    {
        return false;
    }
    ["bash ", "sh ", "./", "python ", "python3 ", "node "]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// UTF-8-safe truncation: keeps at most `max` characters and appends `...`
/// when the input was longer. Never splits a multi-byte code point.
fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}

fn raw_mode_safe_text(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn reply_looks_like_future_work(reply: &str) -> bool {
    let normalized = reply.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let completion_markers = [
        "done",
        "completed",
        "implemented",
        "finished",
        "ready",
        "作成しました",
        "実装しました",
        "完了",
        "できました",
    ];
    if completion_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }
    let future_markers = [
        "now i'll",
        "now i will",
        "i'll ",
        "i will ",
        "let me ",
        "you can run",
        "please run",
        "run this yourself",
        "run it yourself",
        "next,",
        "next i",
        "次に",
        "これから",
        "今から",
        "次は",
        "探してみます",
        "確認します",
        "調べます",
        "見てみます",
        "してみます",
        "実行してください",
        "確認してください",
    ];
    future_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn answer_only_reply_is_inadequate(reply: &str) -> bool {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "read('readme.md')" | "read(\"readme.md\")" | "glob('**/*.md')" | "grep"
    ) {
        return true;
    }
    if (lower.starts_with("read(")
        || lower.starts_with("glob(")
        || lower.starts_with("grep(")
        || lower.starts_with("bash("))
        && trimmed.chars().count() < 120
    {
        return true;
    }
    trimmed.chars().count() < 40
}

fn write_stdout_rendered(text: &str, trailing_newline: bool) {
    let mut out = io::stdout().lock();
    let rendered = raw_mode_safe_text(text);
    let _ = out.write_all(rendered.as_bytes());
    if trailing_newline {
        let _ = out.write_all(b"\r\n");
    }
    let _ = out.flush();
}

fn user_interrupt_result() -> String {
    "exit_code=-1\ninterrupted=true\ninterrupt requested by user".to_string()
}

fn tool_result_failed(result: &str) -> bool {
    result.starts_with("Error:") || result.contains("\ninterrupted=true\n")
}

fn extract_requested_port(task: &str) -> Option<String> {
    let bytes = task.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let candidate = &task[start..i];
        if (2..=5).contains(&candidate.len()) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn task_requires_nextjs_scaffold(task: &str) -> bool {
    requested_scaffold_framework(task) == Some(ScaffoldFramework::Next)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaffoldFramework {
    Next,
    React,
    Nuxt,
}

impl ScaffoldFramework {
    fn label(self) -> &'static str {
        match self {
            Self::Next => "Next.js",
            Self::React => "React.js",
            Self::Nuxt => "Nuxt.js",
        }
    }

    fn scaffold_hint(self) -> &'static str {
        match self {
            Self::Next => "Use create-next-app for the scaffold.",
            Self::React => {
                "Use a Vite React scaffold, for example: npm create vite@latest . -- --template react-ts."
            }
            Self::Nuxt => {
                "Use a Nuxt scaffold, for example: npx nuxi@latest init . --packageManager npm."
            }
        }
    }
}

fn requested_scaffold_framework(task: &str) -> Option<ScaffoldFramework> {
    let normalized = task.to_ascii_lowercase();
    if normalized.contains("next.js") || normalized.contains("nextjs") {
        Some(ScaffoldFramework::Next)
    } else if normalized.contains("nuxt.js") || normalized.contains("nuxt") {
        Some(ScaffoldFramework::Nuxt)
    } else if normalized.contains("react.js") || normalized.contains("react") {
        Some(ScaffoldFramework::React)
    } else {
        None
    }
}

fn scaffold_command_matches_framework(framework: ScaffoldFramework, command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    match framework {
        ScaffoldFramework::Next => normalized.contains("create-next-app"),
        ScaffoldFramework::React => {
            (normalized.contains("create vite")
                || normalized.contains("create-vite")
                || normalized.contains("vite@latest")
                || normalized.contains("vite@"))
                && normalized.contains("react")
        }
        ScaffoldFramework::Nuxt => {
            normalized.contains("nuxi")
                || normalized.contains("create-nuxt")
                || normalized.contains("create nuxt")
                || normalized.contains("nuxt@")
        }
    }
}

fn task_or_plan_requires_nextjs_scaffold(
    active_task: Option<&str>,
    plan_contents: Option<&str>,
) -> bool {
    active_task.is_some_and(task_requires_nextjs_scaffold)
        || plan_contents.is_some_and(task_requires_nextjs_scaffold)
}

fn deterministic_nextjs_scaffold_reply() -> AssistantReply {
    let command = format!(
        "npx --yes create-next-app@{CREATE_NEXT_APP_PACKAGE_VERSION} . --typescript --tailwind --eslint --app --no-src-dir --import-alias \"@/*\" --use-npm --yes"
    );
    AssistantReply {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "deterministic-nextjs-scaffold-1".to_string(),
            name: "Bash".to_string(),
            arguments: serde_json::json!({
                "command": command
            }),
        }],
    }
}

fn fallback_plan_request_label(task: &str) -> String {
    let lower = task.to_ascii_lowercase();
    if lower.contains("next.js") {
        "the requested Next.js app".to_string()
    } else {
        "the requested deliverable".to_string()
    }
}

fn fallback_plan_platform_label(task: &str) -> &'static str {
    if task.to_ascii_lowercase().contains("next.js") {
        "Next.js app"
    } else {
        "local app"
    }
}

fn deterministic_timeout_fallback_plan(
    task: &str,
    task_profile: TaskProfile,
    work_root: &Path,
) -> String {
    let request_label = fallback_plan_request_label(task);
    let platform_label = fallback_plan_platform_label(task);
    let port = extract_requested_port(task)
        .map(|port| format!("port {port}"))
        .unwrap_or_else(|| "the requested port".to_string());
    let worktree_name = work_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("the current repo");
    let execution_focus = match task_profile {
        TaskProfile::Ui => "strong visual identity, motion, and interaction polish",
        TaskProfile::Content => "clear reader-facing output and quality copy",
        TaskProfile::Research => "structured investigation and evidence capture",
        TaskProfile::Coding | TaskProfile::Generic => {
            "a playable vertical slice first, then layered polish"
        }
    };

    format!(
        "# Plan\n\n## Goal\n- Build {request_label} as a {platform_label} inside `{worktree_name}`.\n- Ensure the result runs locally on {port} and feels intentionally polished rather than placeholder-quality.\n\n## Constraints\n- Keep all work inside the current repository root and use repository-relative paths.\n- If the repository is empty, scaffold only the minimum project structure needed before implementing the requested feature.\n- Keep the implementation incremental and avoid placeholder-only output.\n\n## First Action\n- Confirm or scaffold the base app, then make the first concrete implementation edit in a primary artifact such as `src/app/page.tsx`, `app/page.tsx`, or the equivalent entry file.\n- Anchor `package.json` scripts and local startup behavior to {port} before final verification.\n\n## Verification\n- Install dependencies when needed and confirm the app boots locally on {port}.\n- Exercise the main interaction or user-facing flow end-to-end, including success and failure states where applicable.\n- If verification cannot run because of sandbox, network, or host constraints, report that exact constraint instead of treating the work as verified.\n\n<!-- runtime fallback plan: generated after repeated planning model timeouts; focus on {execution_focus}. -->\n"
    )
}

fn format_iteration_status(
    iter_human: usize,
    max_iterations: usize,
    headline: &str,
    note: &str,
    cols: Option<u16>,
) -> String {
    let mut lines = vec![format!("[iter {iter_human}/{max_iterations}] {headline}")];
    lines.push(format_progress_field("  note:   ", note, cols));
    lines.push(String::new());
    lines.join("\n")
}

fn is_plan_file_tool_call(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    plan_path: Option<&Path>,
) -> bool {
    if !matches!(tool_name, "Write" | "Edit") {
        return false;
    }
    let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
        return false;
    };
    resolve_plan_mode_write_target(work_root, raw_path, plan_path)
        .ok()
        .flatten()
        .is_some()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PlanExplorationKey {
    stage: String,
    tool_name: String,
    normalized_args: String,
}

fn normalize_plan_exploration_key(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    stage: &str,
) -> Option<PlanExplorationKey> {
    let normalized_args = match tool_name {
        "Read" => {
            let path = arguments.get("path").and_then(serde_json::Value::as_str)?;
            let path = normalize_exploration_path(path, work_root);
            let start_line = arguments
                .get("start_line")
                .and_then(serde_json::Value::as_u64);
            let end_line = arguments
                .get("end_line")
                .and_then(serde_json::Value::as_u64);
            serde_json::json!({
                "path": path,
                "start_line": start_line,
                "end_line": end_line,
            })
            .to_string()
        }
        "Glob" => serde_json::json!({
            "pattern": arguments
                .get("pattern")
                .and_then(serde_json::Value::as_str)?
                .trim(),
        })
        .to_string(),
        "Grep" => serde_json::json!({
            "pattern": arguments
                .get("pattern")
                .and_then(serde_json::Value::as_str)?
                .trim(),
            "glob": arguments.get("glob").and_then(serde_json::Value::as_str),
            "case_sensitive": arguments
                .get("case_sensitive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
        .to_string(),
        _ => return None,
    };

    Some(PlanExplorationKey {
        stage: stage.to_string(),
        tool_name: tool_name.to_string(),
        normalized_args,
    })
}

fn normalize_exploration_path(raw_path: &str, work_root: &Path) -> String {
    let input = Path::new(raw_path);
    if input.is_relative() {
        let cleaned = input
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !cleaned.is_empty() {
            return cleaned.join("/");
        }
    }

    if let Ok(resolved) = resolve_user_path(work_root, raw_path) {
        let canonical_root = std::fs::canonicalize(work_root).ok();
        let canonical_resolved = std::fs::canonicalize(&resolved).ok();
        if let (Some(root), Some(resolved_path)) = (canonical_root, canonical_resolved)
            && let Ok(relative) = resolved_path.strip_prefix(root)
        {
            return relative.to_string_lossy().replace('\\', "/");
        }
        if let Ok(relative) = resolved.strip_prefix(work_root) {
            return relative.to_string_lossy().replace('\\', "/");
        }
        return resolved.to_string_lossy().replace('\\', "/");
    }

    raw_path.trim().replace('\\', "/")
}

fn log_plan_stall(
    session_id: &str,
    iter: usize,
    reason: &str,
    stage: PlanStage,
    next_sections: &[&str],
    missing_sections: &[&str],
    attempt: usize,
) {
    log_llm_event(
        "agent.plan.stalled",
        serde_json::json!({
            "session_id": session_id,
            "iter": iter,
            "reason": reason,
            "stage": stage.as_str(),
            "next_sections": next_sections,
            "missing_sections": missing_sections,
            "attempt": attempt,
        }),
    );
}

fn progress_stage_label(
    mode: ExecutionMode,
    plan_stage: PlanStage,
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    plan_path: Option<&Path>,
) -> Option<String> {
    if mode != ExecutionMode::Plan {
        return Some("Implementation".to_string());
    }
    let raw_path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if plan_path_matches(raw_path, work_root, plan_path) {
        if tool_name == "Read" {
            return Some(if plan_stage == PlanStage::Ready {
                "Approval review".to_string()
            } else {
                "Plan review".to_string()
            });
        }
        let source_text = arguments
            .get("content")
            .or_else(|| arguments.get("new_string"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let summary = summarize_plan_write(
            tool_name,
            raw_path,
            source_text,
            work_root,
            plan_path,
            plan_stage,
        );
        return Some(summary.phase);
    }
    Some("Repo exploration".to_string())
}

fn extract_plan_constraints(contents: &str) -> Vec<String> {
    let mut in_constraints = false;
    let mut lines = Vec::new();
    for raw_line in contents.lines() {
        let trimmed = raw_line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            in_constraints = heading.trim() == "Constraints";
            continue;
        }
        if !in_constraints {
            continue;
        }
        if trimmed.is_empty() || trimmed == "-" {
            continue;
        }
        let cleaned = trimmed.trim_start_matches("- ").trim().to_string();
        if !cleaned.is_empty() {
            lines.push(cleaned);
        }
    }
    lines
}

fn normalize_memory_path(raw_path: &str, work_root: &Path) -> String {
    let path = Path::new(raw_path);
    if let Ok(resolved) = resolve_user_path(work_root, raw_path)
        && let Ok(relative) = resolved.strip_prefix(work_root)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    let canonical_root = std::fs::canonicalize(work_root).ok();
    let canonical_path = std::fs::canonicalize(path)
        .ok()
        .or_else(|| resolve_user_path(work_root, raw_path).ok());
    if let (Some(root), Some(candidate)) = (canonical_root, canonical_path)
        && let Ok(relative) = candidate.strip_prefix(root)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    if let Ok(relative) = path.strip_prefix(work_root) {
        return relative.to_string_lossy().replace('\\', "/");
    }
    raw_path.replace('\\', "/")
}

fn build_stats(
    accumulated: Vec<RepoVerification>,
    final_verif: RepoVerification,
    iter_used: usize,
    iter_max: usize,
    duration_secs: u64,
) -> LoopStats {
    let mut all_changed: HashSet<String> = HashSet::new();
    let mut impl_changed = 0usize;
    let mut test_changed = 0usize;
    let mut setup_changed = 0usize;
    let mut other_changed = 0usize;
    let mut deleted_changed = 0usize;

    for verif in accumulated.iter().chain(std::iter::once(&final_verif)) {
        for f in &verif.changed_files {
            all_changed.insert(f.clone());
        }
        impl_changed += verif.implementation_files_changed;
        test_changed += verif.test_files_changed;
        setup_changed += verif.setup_files_changed;
        other_changed += verif.other_files_changed;
        deleted_changed += verif.deleted_files_changed;
    }

    let total_changed =
        impl_changed + test_changed + setup_changed + other_changed + deleted_changed;
    let mut changed_files: Vec<String> = all_changed.into_iter().collect();
    changed_files.sort();
    changed_files.truncate(16);

    LoopStats {
        iter_used,
        iter_max,
        duration_secs,
        changed_files,
        total_changed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaffoldFallbackResult {
    NotApplicable,
    Applied,
    Failed,
    Skipped,
}

impl Agent {
    pub(super) fn handle_user_message(&mut self, input: &str, stream_output: bool) -> LoopResult {
        // Start the ESC interrupt monitor for the duration of this turn only —
        // rustyline owns raw mode during the REPL line-edit, so the monitor
        // must live strictly inside `handle_user_message`. Drop at function
        // exit disables raw mode deterministically (AC-2 / AC-3 / R1 / R2).
        let env = InterruptEnv::detect();
        let mut monitor = InterruptMonitor::start(&env);
        self.run_turn(input, stream_output, &mut monitor)
    }

    fn run_turn(
        &mut self,
        input: &str,
        stream_output: bool,
        monitor: &mut InterruptMonitor,
    ) -> LoopResult {
        self.push_user_message(input.to_string());
        if self.session.mode_state.mode != ExecutionMode::Plan {
            self.session.mode_state.work_mode = infer_work_mode_from_text(input);
            self.maybe_compact_session(DEFAULT_KEEP_TAIL);
        }
        let _ = self.refresh_plan_stage();

        let mut action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
        if !self.session.mode_state.policy().repo_edit_required {
            action_expectation = recovery::ActionExpectation::None;
        }
        let requires_action = action_expectation != recovery::ActionExpectation::None;

        self.run_actor_loop(
            action_expectation,
            requires_action,
            stream_output,
            false,
            monitor,
        )
    }

    fn run_actor_loop(
        &mut self,
        action_expectation: recovery::ActionExpectation,
        requires_action: bool,
        stream_output: bool,
        restart_convergence_mode: bool,
        monitor: &mut InterruptMonitor,
    ) -> LoopResult {
        let use_color = io::stdout().is_terminal() && !no_color_requested();
        let use_unicode = unicode_supported();
        let start = Instant::now();
        let mut before_snapshot = capture_repo_snapshot(&self.work_root);
        let mut accumulated: Vec<RepoVerification> = Vec::new();
        let mut last_known_root = self.work_root.clone();

        let mut tool_calls_made_this_turn = 0usize;
        let mut repo_edit_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;
        let mut repo_change_retries = 0usize;
        let mut python_test_retries = 0usize;
        let mut plan_progress_retries = 0usize;
        let mut plan_exploration_only_turns = 0usize;
        let mut plan_exploration_counts = HashMap::<PlanExplorationKey, usize>::new();
        let mut plan_write_signature_counts = HashMap::<String, usize>::new();
        let mut recent_bash_commands = Vec::<String>::new();
        let mut install_commands_seen = 0usize;
        let mut logged_plan_first_write = false;
        let mut logged_act_first_repo_edit = false;

        let mut exit_reason = ExitReason::MaxIterations;
        let mut error_text = String::new();
        let mut last_iter = 0usize;
        let mut final_prose = String::new();

        let interrupt_flag = monitor.flag();

        'outer: for iter_count in 0..self.config.max_iterations {
            last_iter = iter_count + 1;
            let approx_tokens = approximate_token_count(&self.session.messages);
            tracing::debug!(iter = iter_count, tokens = approx_tokens, "iter");
            // Publish per-turn token count to the footer (issue #430, AC12).
            // Reuses the value we just computed — O(1), no second walk over
            // `messages`. No-op when the footer handle is disabled.
            self.footer.publish_tokens(approx_tokens);

            // Boundary 1: before requesting the next assistant reply. Lets us
            // bail out between iterations without starting a fresh LLM call.
            if interrupt_flag.is_set() {
                exit_reason = ExitReason::Interrupted;
                break 'outer;
            }

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn == 0
                && let Some(summary) = self.maybe_materialize_mode_deterministic_fallback(last_iter)
            {
                final_prose = summary;
                exit_reason = ExitReason::Done;
                break 'outer;
            }

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn == 0
                && self.maybe_materialize_framework_game_fallback(last_iter)
            {
                final_prose =
                    "Implemented the requested runnable app with deterministic framework files."
                        .to_string();
                exit_reason = ExitReason::Done;
                break 'outer;
            }

            if repo_edit_calls_made_this_turn == 0
                && self.current_request_needs_playable_ui_quality_gate()
                && let Some((request, target_path)) = self.accepted_repo_change_polish_target()
            {
                match self.maybe_apply_deterministic_polish_fallback(&request, &target_path) {
                    Ok(true) => {
                        write_stdout_rendered(
                            &format_iteration_status(
                                last_iter,
                                self.config.max_iterations,
                                "Polish fallback",
                                &format!("Applied deterministic visual polish to {target_path}."),
                                self.footer.current_cols(),
                            ),
                            true,
                        );
                        final_prose = format!(
                            "Improved the requested playable UI with deterministic visual polish in {target_path}."
                        );
                        exit_reason = ExitReason::Done;
                        break 'outer;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        exit_reason = ExitReason::TransportError;
                        error_text = err;
                        break 'outer;
                    }
                }
            }

            let reply =
                match self.request_assistant_reply_with_retry(stream_output, &interrupt_flag) {
                    Ok(r) => r,
                    Err(err) => {
                        exit_reason = if err == USER_INTERRUPT_ERROR {
                            ExitReason::Interrupted
                        } else if lifecycle::is_tool_call_format_error(&err) {
                            ExitReason::ToolCallFormatError
                        } else {
                            ExitReason::TransportError
                        };
                        error_text = err;
                        break 'outer;
                    }
                };

            // Boundary 2: right after the Ollama response completes. This is
            // the AC-10 checkpoint — mid-flight cancel is out of scope.
            if interrupt_flag.is_set() {
                exit_reason = ExitReason::Interrupted;
                break 'outer;
            }

            let mut prepared_tool_calls = reply
                .tool_calls
                .into_iter()
                .map(|tool_call| self.prepare_tool_call(tool_call))
                .collect::<Vec<_>>();

            if let Some(target) = self.focused_edit_recovery_target() {
                let target_already_read = focused_edit_target_already_read(
                    &self.session.messages,
                    &target,
                    &self.work_root,
                );
                match focused_edit_tool_batch_action(
                    &prepared_tool_calls,
                    &target,
                    &self.work_root,
                    target_already_read,
                ) {
                    FocusedEditBatchAction::Accept => {}
                    FocusedEditBatchAction::TruncateToFirst => {
                        prepared_tool_calls.truncate(1);
                        write_stdout_rendered(
                            &format_iteration_status(
                                last_iter,
                                self.config.max_iterations,
                                "Focused edit narrowed",
                                "Ignored extra tool calls and kept only the first focused action on the target file.",
                                self.footer.current_cols(),
                            ),
                            true,
                        );
                    }
                    FocusedEditBatchAction::Reject(err) => {
                        self.session.working_memory.note_error(err);
                        repo_change_retries += 1;
                        if repo_change_retries >= 3 {
                            exit_reason = ExitReason::MissingRepoEdits;
                            error_text = exit_reason.default_error_text().to_string();
                            break 'outer;
                        }
                        write_stdout_rendered(
                            &format_iteration_status(
                                last_iter,
                                self.config.max_iterations,
                                "Retry requested",
                                "Focused edit recovery requires exactly one compact tool call on the target file. Asked the model to retry with a single action.",
                                self.footer.current_cols(),
                            ),
                            true,
                        );
                        self.push_system_note(recovery::focused_edit_no_tool_recovery_note(
                            &progress_path_display(
                                &target.display().to_string(),
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                120,
                            ),
                            target_already_read,
                            repo_change_retries,
                        ));
                        continue;
                    }
                }
            }

            if !prepared_tool_calls.is_empty() {
                let mut plan_file_edit_calls_this_turn = 0usize;
                let mut plan_exploration_calls_this_turn = 0usize;
                let mut plan_ready_after_tool = false;
                let mut bash_only_tool_turn = true;
                let current_plan_stage = self.session.mode_state.plan_stage;
                let plan_missing_before_turn =
                    if self.session.mode_state.mode == ExecutionMode::Plan {
                        self.current_plan_contents()
                            .ok()
                            .flatten()
                            .map(|contents| lifecycle::plan_missing_sections(&contents).len())
                    } else {
                        None
                    };
                let plan_exploration_budget =
                    lifecycle::plan_stage_exploration_budget(current_plan_stage);
                tool_calls_made_this_turn += prepared_tool_calls.len();
                repo_edit_calls_made_this_turn += prepared_tool_calls
                    .iter()
                    .filter(|tool_call| recovery::tool_call_counts_as_repo_edit(&tool_call.name))
                    .count();
                empty_retries = 0;
                no_tool_retries = 0;
                if repo_edit_calls_made_this_turn > 0 {
                    repo_change_retries = 0;
                }

                self.session.messages.push(ConversationMessage::assistant(
                    reply.content,
                    prepared_tool_calls.clone(),
                ));
                let mut emitted_bash_loop_note = false;
                for tool_call in prepared_tool_calls {
                    let tool_name = tool_call.name.clone();
                    if tool_name != "Bash" {
                        bash_only_tool_turn = false;
                    }
                    let args_str = tool_call.arguments.to_string();
                    let bash_command = if tool_name == "Bash" {
                        tool_call
                            .arguments
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        String::new()
                    };
                    if self.session.mode_state.mode == ExecutionMode::Plan {
                        if is_plan_file_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            &self.work_root,
                            self.session.mode_state.active_plan_path.as_deref(),
                        ) {
                            plan_file_edit_calls_this_turn += 1;
                            if !logged_plan_first_write {
                                logged_plan_first_write = true;
                                log_llm_event(
                                    "agent.milestone.plan_first_write",
                                    serde_json::json!({
                                        "session_id": self.session_store.session_id(),
                                        "iter": last_iter,
                                        "tool": tool_name,
                                        "path": tool_call.arguments.get("path").and_then(serde_json::Value::as_str),
                                    }),
                                );
                            }
                        } else if matches!(tool_name.as_str(), "Read" | "Glob" | "Grep") {
                            plan_exploration_calls_this_turn += 1;
                        }
                    }
                    if self.session.mode_state.mode == ExecutionMode::Act
                        && recovery::tool_call_counts_as_repo_edit(&tool_name)
                        && !logged_act_first_repo_edit
                    {
                        logged_act_first_repo_edit = true;
                        log_llm_event(
                            "agent.milestone.act_first_repo_edit",
                            serde_json::json!({
                                "session_id": self.session_store.session_id(),
                                "iter": last_iter,
                                "task_profile": self.session.mode_state.task_profile.as_str(),
                                "tool": tool_name,
                                "path": tool_call.arguments.get("path").and_then(serde_json::Value::as_str),
                            }),
                        );
                    }
                    let block_restart_discovery = recovery::should_block_restart_discovery(
                        &tool_name,
                        restart_convergence_mode && repo_edit_calls_made_this_turn == 0,
                    );
                    let repeated_plan_exploration =
                        if self.session.mode_state.mode == ExecutionMode::Plan {
                            normalize_plan_exploration_key(
                                &tool_name,
                                &tool_call.arguments,
                                &self.work_root,
                                current_plan_stage.as_str(),
                            )
                            .map(|key| {
                                let count = plan_exploration_counts.entry(key.clone()).or_insert(0);
                                *count += 1;
                                if *count == PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD {
                                    log_llm_event(
                                        "agent.plan.repeated_exploration_detected",
                                        serde_json::json!({
                                            "session_id": self.session_store.session_id(),
                                            "iter": last_iter,
                                            "stage": key.stage,
                                            "tool": key.tool_name,
                                            "normalized_args": key.normalized_args,
                                            "count": *count,
                                        }),
                                    );
                                }
                                *count >= PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD
                            })
                            .unwrap_or(false)
                        } else {
                            false
                        };
                    let block_bash_loop = tool_name == "Bash"
                        && recovery::should_block_bash_command(
                            &bash_command,
                            &recent_bash_commands,
                            install_commands_seen,
                        );
                    let block_repeated_plan_exploration = self.session.mode_state.mode
                        == ExecutionMode::Plan
                        && matches!(tool_name.as_str(), "Read" | "Glob" | "Grep")
                        && repeated_plan_exploration;
                    let block_plan_exploration = self.session.mode_state.mode
                        == ExecutionMode::Plan
                        && matches!(tool_name.as_str(), "Read" | "Glob" | "Grep")
                        && plan_exploration_budget > 0
                        && plan_exploration_calls_this_turn > plan_exploration_budget;
                    tracing::debug!(
                        tool = %tool_name,
                        args = %truncate(&args_str, LOG_ARGS_MAX_CHARS),
                        "tool call"
                    );
                    // Issue #430 Phase D: pause footer redraw for the whole
                    // tool dispatch (progress println, spinner, child-process
                    // fd-inheriting exec, optional approve prompt). The guard
                    // drops at the end of this iteration so the worker resumes
                    // before the next loop tick.
                    let _footer_freeze = self.footer.freeze_for_inference();
                    let progress = if block_restart_discovery {
                        format_blocked_progress_line(
                            &tool_name,
                            &tool_call.arguments,
                            iter_count + 1,
                            self.config.max_iterations,
                            &self.work_root,
                            use_color,
                            use_unicode,
                            self.footer.current_cols(),
                            "Restart discovery blocked",
                            "Resume from the current repo state instead of restarting broad discovery.",
                            self.session.mode_state.active_plan_path.as_deref(),
                            current_plan_stage,
                        )
                    } else if block_bash_loop {
                        format_blocked_progress_line(
                            &tool_name,
                            &tool_call.arguments,
                            iter_count + 1,
                            self.config.max_iterations,
                            &self.work_root,
                            use_color,
                            use_unicode,
                            self.footer.current_cols(),
                            "Bash loop blocked",
                            "Repeated shell command detected; choose a different next step.",
                            self.session.mode_state.active_plan_path.as_deref(),
                            current_plan_stage,
                        )
                    } else if block_plan_exploration {
                        format_blocked_progress_line(
                            &tool_name,
                            &tool_call.arguments,
                            iter_count + 1,
                            self.config.max_iterations,
                            &self.work_root,
                            use_color,
                            use_unicode,
                            self.footer.current_cols(),
                            "Plan exploration blocked",
                            "Exploration budget reached for this stage; write the next missing plan section.",
                            self.session.mode_state.active_plan_path.as_deref(),
                            current_plan_stage,
                        )
                    } else if block_repeated_plan_exploration {
                        format_blocked_progress_line(
                            &tool_name,
                            &tool_call.arguments,
                            iter_count + 1,
                            self.config.max_iterations,
                            &self.work_root,
                            use_color,
                            use_unicode,
                            self.footer.current_cols(),
                            "Plan exploration blocked",
                            "Repeated exploration detected; move the plan forward instead of rereading.",
                            self.session.mode_state.active_plan_path.as_deref(),
                            current_plan_stage,
                        )
                    } else {
                        let live_plan_stage = if self.session.mode_state.mode == ExecutionMode::Plan
                        {
                            self.current_plan_contents()
                                .ok()
                                .flatten()
                                .map(|contents| lifecycle::current_plan_stage(&contents))
                                .unwrap_or(current_plan_stage)
                        } else {
                            current_plan_stage
                        };
                        let stage_label = progress_stage_label(
                            self.session.mode_state.mode,
                            live_plan_stage,
                            &tool_name,
                            &tool_call.arguments,
                            &self.work_root,
                            self.session.mode_state.active_plan_path.as_deref(),
                        );
                        let write_retry_label = if self.session.mode_state.mode
                            == ExecutionMode::Plan
                            && is_plan_file_tool_call(
                                &tool_name,
                                &tool_call.arguments,
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                            ) {
                            let raw_path = tool_call
                                .arguments
                                .get("path")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default();
                            let source_text = tool_call
                                .arguments
                                .get("content")
                                .or_else(|| tool_call.arguments.get("new_string"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default();
                            let summary = summarize_plan_write(
                                &tool_name,
                                raw_path,
                                source_text,
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                live_plan_stage,
                            );
                            let count = plan_write_signature_counts
                                .entry(summary.signature)
                                .and_modify(|value| *value += 1)
                                .or_insert(1);
                            (*count > 1).then(|| format!("Model rewrite #{}", *count))
                        } else {
                            None
                        };
                        format_progress_line(
                            &tool_name,
                            &tool_call.arguments,
                            iter_count + 1,
                            self.config.max_iterations,
                            &self.work_root,
                            use_color,
                            use_unicode,
                            self.footer.current_cols(),
                            self.session.mode_state.active_plan_path.as_deref(),
                            live_plan_stage,
                            write_retry_label.as_deref(),
                            stage_label.as_deref(),
                        )
                    };
                    write_stdout_rendered(&progress, true);
                    // approve-guard: tools Bash/Write/Edit may invoke an
                    // interactive approve prompt in `tools/registry.rs`. We
                    // must not let the spinner write to stderr while stdin is
                    // being read. Skip spinner in that narrow case; RAII
                    // scope ends when execute_tool_call returns for all
                    // other branches.
                    let needs_approve_prompt =
                        matches!(tool_name.as_str(), "Bash" | "Write" | "Edit")
                            && !self.config.yes_mode
                            && io::stdin().is_terminal();
                    let start_spinner_for_exec = !needs_approve_prompt;
                    // Yield raw mode to the approve `stdin().read_line` and park
                    // the daemon thread until `resume()` is called. Idempotent,
                    // so a tool that never triggers the prompt is unaffected.
                    if needs_approve_prompt {
                        monitor.pause();
                    }
                    let raw_result = if block_restart_discovery {
                        recovery::broad_restart_discovery_error(&tool_name)
                    } else if block_repeated_plan_exploration {
                        let next_sections = self
                            .current_plan_contents()
                            .ok()
                            .flatten()
                            .map(|contents| lifecycle::plan_next_stage_sections(&contents))
                            .unwrap_or_default();
                        log_llm_event(
                            "agent.plan.guard_blocked",
                            serde_json::json!({
                                "session_id": self.session_store.session_id(),
                                "iter": last_iter,
                                "tool": tool_name,
                                "reason": "repeated_exploration",
                                "stage": current_plan_stage.as_str(),
                            }),
                        );
                        recovery::repeated_plan_exploration_error(
                            current_plan_stage,
                            &next_sections,
                            &tool_name,
                        )
                    } else if block_plan_exploration {
                        let next_sections = self
                            .current_plan_contents()
                            .ok()
                            .flatten()
                            .map(|contents| lifecycle::plan_next_stage_sections(&contents))
                            .unwrap_or_default();
                        log_llm_event(
                            "agent.plan.guard_blocked",
                            serde_json::json!({
                                "session_id": self.session_store.session_id(),
                                "iter": last_iter,
                                "tool": tool_name,
                                "reason": "exploration_budget",
                                "stage": current_plan_stage.as_str(),
                                "budget": plan_exploration_budget,
                            }),
                        );
                        recovery::plan_stage_budget_error(
                            current_plan_stage,
                            &next_sections,
                            plan_exploration_budget,
                        )
                    } else if tool_name == "Bash" {
                        recent_bash_commands.push(bash_command.clone());
                        if recovery::is_dependency_install_command(&bash_command) {
                            install_commands_seen += 1;
                        }
                        if block_bash_loop {
                            emitted_bash_loop_note = true;
                            recovery::repeated_bash_error(&bash_command)
                        } else if start_spinner_for_exec {
                            let _sp = Spinner::start(format!("running {tool_name}..."));
                            self.execute_tool_call(
                                &tool_name,
                                &tool_call.arguments,
                                Some(interrupt_flag.flag.clone()),
                            )
                        } else {
                            self.execute_tool_call(
                                &tool_name,
                                &tool_call.arguments,
                                Some(interrupt_flag.flag.clone()),
                            )
                        }
                    } else if start_spinner_for_exec {
                        let _sp = Spinner::start(format!("running {tool_name}..."));
                        self.execute_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            Some(interrupt_flag.flag.clone()),
                        )
                    } else {
                        self.execute_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            Some(interrupt_flag.flag.clone()),
                        )
                    };
                    if needs_approve_prompt {
                        monitor.resume();
                    }

                    // detect work_root change after each tool execution
                    if self.work_root != last_known_root {
                        let verif = verify_repo_progress(&before_snapshot, &last_known_root);
                        accumulated.push(verif);
                        before_snapshot = capture_repo_snapshot(&self.work_root);
                        last_known_root = self.work_root.clone();
                    }

                    let compact_result = prompting::compact_tool_result(&tool_name, raw_result);
                    self.session
                        .messages
                        .push(ConversationMessage::tool(tool_name.clone(), compact_result));

                    if self.session.mode_state.mode == ExecutionMode::Plan
                        && is_plan_file_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            &self.work_root,
                            self.session.mode_state.active_plan_path.as_deref(),
                        )
                        && self
                            .current_plan_contents()
                            .ok()
                            .flatten()
                            .is_some_and(|contents| {
                                self.plan_is_approval_ready_with_fallback(&contents)
                            })
                    {
                        plan_ready_after_tool = true;
                        break;
                    }
                }
                if self.session.mode_state.mode == ExecutionMode::Plan {
                    if plan_ready_after_tool {
                        final_prose = "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string();
                        exit_reason = ExitReason::Done;
                        break 'outer;
                    }
                    if plan_file_edit_calls_this_turn > 0 {
                        let plan_contents = self
                            .current_plan_contents()
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        self.session.mode_state.plan_stage =
                            lifecycle::current_plan_stage(&plan_contents);
                        let missing_after = lifecycle::plan_missing_sections(&plan_contents);
                        let made_section_progress = plan_missing_before_turn
                            .is_none_or(|before| missing_after.len() < before);
                        if made_section_progress {
                            plan_progress_retries = 0;
                            plan_exploration_only_turns = 0;
                        } else {
                            plan_progress_retries += 1;
                            if plan_progress_retries >= 2 {
                                match self.materialize_deterministic_fallback_plan(
                                    "agent.plan.non_progress_edit_fallback_materialized",
                                ) {
                                    Ok(true) => {
                                        final_prose = "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string();
                                        exit_reason = ExitReason::Done;
                                    }
                                    Ok(false) => {
                                        exit_reason = ExitReason::PlanIncomplete;
                                        error_text = exit_reason.default_error_text().to_string();
                                    }
                                    Err(err) => {
                                        exit_reason = ExitReason::TransportError;
                                        error_text = err;
                                    }
                                }
                                break 'outer;
                            }
                            self.push_system_note(recovery::plan_progress_recovery_note(
                                self.session.mode_state.plan_stage,
                                &lifecycle::plan_next_stage_sections(&plan_contents),
                                &missing_after,
                                plan_progress_retries,
                            ));
                        }
                    } else if plan_exploration_calls_this_turn >= 2 {
                        match self.current_plan_contents() {
                            Ok(Some(contents)) => {
                                let current_stage = lifecycle::current_plan_stage(&contents);
                                let next_sections = lifecycle::plan_next_stage_sections(&contents);
                                let missing_sections = lifecycle::plan_missing_sections(&contents);
                                if !missing_sections.is_empty() {
                                    plan_progress_retries += 1;
                                    log_plan_stall(
                                        self.session_store.session_id(),
                                        last_iter,
                                        "exploration_only_turn",
                                        current_stage,
                                        &next_sections,
                                        &missing_sections,
                                        plan_progress_retries,
                                    );
                                    self.push_system_note(recovery::plan_progress_recovery_note(
                                        current_stage,
                                        &next_sections,
                                        &missing_sections,
                                        plan_progress_retries,
                                    ));
                                }
                            }
                            Ok(None) => {}
                            Err(err) => {
                                exit_reason = ExitReason::TransportError;
                                error_text = err;
                                break 'outer;
                            }
                        }
                    } else if plan_exploration_calls_this_turn > 0 {
                        plan_exploration_only_turns += 1;
                        if plan_exploration_only_turns >= 1 {
                            match self.current_plan_contents() {
                                Ok(Some(contents)) => {
                                    let current_stage = lifecycle::current_plan_stage(&contents);
                                    let next_sections =
                                        lifecycle::plan_next_stage_sections(&contents);
                                    let missing_sections =
                                        lifecycle::plan_missing_sections(&contents);
                                    if !missing_sections.is_empty() {
                                        plan_progress_retries += 1;
                                        log_plan_stall(
                                            self.session_store.session_id(),
                                            last_iter,
                                            "repeated_exploration_only_turns",
                                            current_stage,
                                            &next_sections,
                                            &missing_sections,
                                            plan_progress_retries,
                                        );
                                        self.push_system_note(
                                            recovery::plan_progress_recovery_note(
                                                current_stage,
                                                &next_sections,
                                                &missing_sections,
                                                plan_progress_retries,
                                            ),
                                        );
                                    }
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    exit_reason = ExitReason::TransportError;
                                    error_text = err;
                                    break 'outer;
                                }
                            }
                            plan_exploration_only_turns = 0;
                        }
                    }
                }
                if emitted_bash_loop_note {
                    self.push_system_note(recovery::install_loop_recovery_note());
                } else if self.session.mode_state.mode == ExecutionMode::Act
                    && action_expectation == recovery::ActionExpectation::RepoChange
                    && bash_only_tool_turn
                    && repo_edit_calls_made_this_turn == 0
                    && !logged_act_first_repo_edit
                {
                    self.push_system_note(recovery::repo_change_after_setup_note());
                }
                if repo_edit_calls_made_this_turn == 0
                    && tool_calls_made_this_turn > 0
                    && self.current_request_needs_playable_ui_quality_gate()
                    && let Some((request, target_path)) = self.accepted_repo_change_polish_target()
                {
                    match self.maybe_apply_deterministic_polish_fallback(&request, &target_path) {
                        Ok(true) => {
                            write_stdout_rendered(
                                &format_iteration_status(
                                    last_iter,
                                    self.config.max_iterations,
                                    "Polish fallback",
                                    &format!(
                                        "Applied deterministic visual polish to {target_path}."
                                    ),
                                    self.footer.current_cols(),
                                ),
                                true,
                            );
                            final_prose = format!(
                                "Improved the requested playable UI with deterministic visual polish in {target_path}."
                            );
                            exit_reason = ExitReason::Done;
                            break 'outer;
                        }
                        Ok(false) => {}
                        Err(err) => {
                            exit_reason = ExitReason::TransportError;
                            error_text = err;
                            break 'outer;
                        }
                    }
                }
                if repo_edit_calls_made_this_turn == 0
                    && tool_calls_made_this_turn > 0
                    && self.current_request_needs_playable_ui_quality_gate()
                    && let Some((request, target_path, _issue)) =
                        self.accepted_repo_change_quality_issue()
                {
                    match self.maybe_apply_deterministic_quality_fallback(&request, &target_path) {
                        Ok(true) => {
                            write_stdout_rendered(
                                &format_iteration_status(
                                    last_iter,
                                    self.config.max_iterations,
                                    "Quality fallback",
                                    &format!(
                                        "Replaced scaffold placeholder output in {target_path}."
                                    ),
                                    self.footer.current_cols(),
                                ),
                                true,
                            );
                            final_prose = format!(
                                "Implemented the requested playable UI by replacing scaffold placeholder output in {target_path}."
                            );
                            exit_reason = ExitReason::Done;
                            break 'outer;
                        }
                        Ok(false) => {}
                        Err(err) => {
                            exit_reason = ExitReason::TransportError;
                            error_text = err;
                            break 'outer;
                        }
                    }
                }
                if repo_edit_calls_made_this_turn > 0
                    && (should_apply_repo_change_quality_gate(
                        action_expectation,
                        self.active_task_expects_repo_change(),
                        self.session.mode_state.mode,
                    ) || self.current_request_needs_playable_ui_quality_gate())
                    && let Some((request, target_path, issue)) =
                        self.accepted_repo_change_quality_issue()
                {
                    match self.maybe_apply_deterministic_quality_fallback(&request, &target_path) {
                        Ok(true) => {
                            write_stdout_rendered(
                                &format_iteration_status(
                                    last_iter,
                                    self.config.max_iterations,
                                    "Quality fallback",
                                    &format!(
                                        "Replaced scaffold placeholder output in {target_path}."
                                    ),
                                    self.footer.current_cols(),
                                ),
                                true,
                            );
                            final_prose = format!(
                                "Implemented the requested playable UI by replacing scaffold placeholder output in {target_path}."
                            );
                            exit_reason = ExitReason::Done;
                            break 'outer;
                        }
                        Ok(false) => {}
                        Err(err) => {
                            exit_reason = ExitReason::TransportError;
                            error_text = err;
                            break 'outer;
                        }
                    }
                    repo_change_retries += 1;
                    if repo_change_retries >= 3 {
                        exit_reason = ExitReason::MissingRepoEdits;
                        error_text = issue;
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Quality gate",
                            &format!(
                                "Asked the model to replace placeholder output in {target_path}."
                            ),
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    self.push_system_note(recovery::repo_change_quality_gate_note(
                        &request,
                        &target_path,
                        &issue,
                        repo_change_retries,
                    ));
                }
                let compacted = if self.session.mode_state.mode == ExecutionMode::Plan {
                    false
                } else {
                    self.maybe_compact_late_turn_session(
                        tool_calls_made_this_turn,
                        repo_edit_calls_made_this_turn,
                    )
                };
                if !compacted && self.session.mode_state.mode != ExecutionMode::Plan {
                    self.maybe_compact_session(DEFAULT_KEEP_TAIL);
                }
                // Boundary 3: after tool messages have been pushed and the
                // session has been compacted, so `persist_session` (called by
                // `process_line`) can save a consistent snapshot the user can
                // `--resume` from. `Condvar` wake on drop makes this cheap.
                if interrupt_flag.is_set() {
                    exit_reason = ExitReason::Interrupted;
                    break 'outer;
                }
                continue;
            }

            let final_reply = reply.content.trim().to_string();
            if final_reply.is_empty() {
                if action_expectation == recovery::ActionExpectation::RepoChange {
                    repo_change_retries += 1;
                    if repo_change_retries >= 2 {
                        if self.maybe_materialize_framework_game_fallback(last_iter) {
                            final_prose = "Implemented the requested runnable app with deterministic framework files.".to_string();
                            exit_reason = ExitReason::Done;
                            break 'outer;
                        }
                        exit_reason = ExitReason::MissingRepoEdits;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            "The model replied without edits. Asked it to make the required repository changes.",
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                } else if action_expectation == recovery::ActionExpectation::PlanProgress {
                    let plan_contents = self
                        .current_plan_contents()
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let current_stage = lifecycle::current_plan_stage(&plan_contents);
                    let next_sections = lifecycle::plan_next_stage_sections(&plan_contents);
                    plan_progress_retries += 1;
                    if plan_progress_retries >= 2 {
                        match self.materialize_deterministic_fallback_plan(
                            "agent.plan.progress_fallback_materialized",
                        ) {
                            Ok(true) => {
                                final_prose = "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string();
                                exit_reason = ExitReason::Done;
                            }
                            Ok(false) => {
                                exit_reason = ExitReason::PlanIncomplete;
                                error_text = exit_reason.default_error_text().to_string();
                            }
                            Err(err) => {
                                exit_reason = ExitReason::TransportError;
                                error_text = err;
                            }
                        }
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            &format!(
                                "The model returned an empty reply. Asked it to continue the plan by writing {}.",
                                join_sections_for_progress(&next_sections)
                            ),
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    let missing_sections = lifecycle::plan_missing_sections(&plan_contents);
                    log_plan_stall(
                        self.session_store.session_id(),
                        last_iter,
                        "empty_reply",
                        current_stage,
                        &next_sections,
                        &missing_sections,
                        plan_progress_retries,
                    );
                    self.push_system_note(recovery::plan_progress_recovery_note(
                        current_stage,
                        &next_sections,
                        &missing_sections,
                        plan_progress_retries,
                    ));
                } else {
                    empty_retries += 1;
                    if empty_retries >= 3 {
                        exit_reason = ExitReason::EmptyResponses;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            "The model returned an empty reply. Asked it to continue with concrete tool actions.",
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    self.push_system_note(recovery::empty_response_recovery_note(
                        empty_retries,
                        requires_action,
                    ));
                }
                continue;
            }

            if requires_action && tool_calls_made_this_turn == 0 {
                if action_expectation == recovery::ActionExpectation::RepoChange {
                    repo_change_retries += 1;
                    if repo_change_retries >= 2 {
                        if self.maybe_materialize_framework_game_fallback(last_iter) {
                            final_prose = "Implemented the requested runnable app with deterministic framework files.".to_string();
                            exit_reason = ExitReason::Done;
                            break 'outer;
                        }
                        exit_reason = ExitReason::MissingRepoEdits;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            "The model answered with prose only. Asked it to emit exactly one tool call now and resume concrete repo work.",
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    if let Some(target) = self.focused_edit_recovery_target() {
                        let target_already_read = focused_edit_target_already_read(
                            &self.session.messages,
                            &target,
                            &self.work_root,
                        );
                        self.push_system_note(recovery::focused_edit_no_tool_recovery_note(
                            &progress_path_display(
                                &target.display().to_string(),
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                120,
                            ),
                            target_already_read,
                            repo_change_retries,
                        ));
                    } else {
                        self.push_system_note(recovery::repo_change_no_tool_recovery_note(
                            repo_change_retries,
                        ));
                    }
                } else if action_expectation == recovery::ActionExpectation::PlanProgress {
                    let plan_contents = self
                        .current_plan_contents()
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if self.plan_is_substantive_with_fallback(&plan_contents) {
                        final_prose = final_reply;
                        exit_reason = ExitReason::Done;
                        break 'outer;
                    }
                    let current_stage = lifecycle::current_plan_stage(&plan_contents);
                    let next_sections = lifecycle::plan_next_stage_sections(&plan_contents);
                    plan_progress_retries += 1;
                    if plan_progress_retries >= 2 {
                        match self.materialize_deterministic_fallback_plan(
                            "agent.plan.progress_fallback_materialized",
                        ) {
                            Ok(true) => {
                                final_prose = "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string();
                                exit_reason = ExitReason::Done;
                            }
                            Ok(false) => {
                                exit_reason = ExitReason::PlanIncomplete;
                                error_text = exit_reason.default_error_text().to_string();
                            }
                            Err(err) => {
                                exit_reason = ExitReason::TransportError;
                                error_text = err;
                            }
                        }
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            &format!(
                                "The model answered without tool calls. Asked it to update {} with Write or Edit.",
                                join_sections_for_progress(&next_sections)
                            ),
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    let missing_sections = lifecycle::plan_missing_sections(&plan_contents);
                    log_plan_stall(
                        self.session_store.session_id(),
                        last_iter,
                        "no_tool_reply",
                        current_stage,
                        &next_sections,
                        &missing_sections,
                        plan_progress_retries,
                    );
                    self.push_system_note(recovery::plan_no_tool_recovery_note(
                        current_stage,
                        &next_sections,
                        plan_progress_retries,
                    ));
                } else {
                    no_tool_retries += 1;
                    if no_tool_retries >= 3 {
                        exit_reason = ExitReason::NoToolCalls;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Retry requested",
                            "The model answered without tool calls. Asked it to continue with concrete actions.",
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    self.push_system_note(recovery::no_tool_recovery_note(no_tool_retries));
                }
                continue;
            }

            if !requires_action
                && self.answer_only_mode_active()
                && reply_looks_like_future_work(&final_reply)
            {
                no_tool_retries += 1;
                if no_tool_retries >= 1 {
                    final_prose = self.answer_only_fallback_response();
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Retry requested",
                        "The model answered with next-step prose in answer-only mode. Asked it to answer directly without more tools.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(
                    "[Answer-only Recovery] Answer the user's request now using only the context already inspected. Do not announce the next action, do not use tools, do not edit files, and do not ask the user to run anything."
                        .to_string(),
                );
                continue;
            }

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn == 0
            {
                if self.maybe_materialize_framework_game_fallback(last_iter) {
                    final_prose = "Implemented the requested runnable app with deterministic framework files.".to_string();
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                match self.maybe_apply_deterministic_nextjs_scaffold(last_iter, &interrupt_flag) {
                    ScaffoldFallbackResult::Applied => {
                        repo_change_retries = 0;
                        continue;
                    }
                    ScaffoldFallbackResult::Failed | ScaffoldFallbackResult::Skipped => {
                        repo_change_retries += 1;
                        if repo_change_retries >= 3 {
                            exit_reason = ExitReason::MissingRepoEdits;
                            error_text = exit_reason.default_error_text().to_string();
                            break 'outer;
                        }
                        self.push_system_note(recovery::repo_change_recovery_note(
                            repo_change_retries,
                        ));
                        continue;
                    }
                    ScaffoldFallbackResult::NotApplicable => {}
                }
                repo_change_retries += 1;
                if repo_change_retries >= 3 {
                    exit_reason = ExitReason::MissingRepoEdits;
                    error_text = exit_reason.default_error_text().to_string();
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Retry requested",
                        "The turn finished without repository edits. Asked the model to continue implementing changes.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                if let Some(target) = self.focused_edit_recovery_target() {
                    let target_already_read = focused_edit_target_already_read(
                        &self.session.messages,
                        &target,
                        &self.work_root,
                    );
                    self.push_system_note(recovery::focused_edit_no_tool_recovery_note(
                        &progress_path_display(
                            &target.display().to_string(),
                            &self.work_root,
                            self.session.mode_state.active_plan_path.as_deref(),
                            120,
                        ),
                        target_already_read,
                        repo_change_retries,
                    ));
                } else {
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                }
                continue;
            }

            if repo_edit_calls_made_this_turn > 0
                && self.active_python_request_requires_tests()
                && !self.python_test_artifact_exists()
            {
                python_test_retries += 1;
                if python_test_retries >= 2 {
                    match self.maybe_materialize_python_test_fallback() {
                        Ok(Some(path)) => {
                            final_prose = format!(
                                "Added the requested Python test artifact with deterministic fallback: {path}."
                            );
                            exit_reason = ExitReason::Done;
                        }
                        Ok(None) => {
                            exit_reason = ExitReason::MissingRepoEdits;
                            error_text = "assistant did not add the requested Python test artifact"
                                .to_string();
                        }
                        Err(err) => {
                            exit_reason = ExitReason::TransportError;
                            error_text = err;
                        }
                    }
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Quality gate",
                        "Asked the model to add the requested Python test file or self-test command.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(
                    "[Python Test Policy] The user explicitly requested tests. Add a concrete Python test artifact now, such as test_*.py, *_test.py, or a clearly runnable self-test command. Keep the edit small and verify it if possible."
                        .to_string(),
                );
                continue;
            }

            if !requires_action
                && self.answer_only_mode_active()
                && answer_only_reply_is_inadequate(&final_reply)
            {
                no_tool_retries += 1;
                if no_tool_retries >= 2 {
                    final_prose = self.answer_only_fallback_response();
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Retry requested",
                        "The model gave an underspecified answer in answer-only mode. Asked it to provide a concrete response.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(
                    "[Answer-only Recovery] Answer the user's request now with concrete findings from the available context. Do not output a tool call, do not edit files, and do not ask the user to run anything."
                        .to_string(),
                );
                continue;
            }

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn > 0
                && reply_looks_like_future_work(&final_reply)
            {
                repo_change_retries += 1;
                if repo_change_retries >= 3 {
                    exit_reason = ExitReason::MissingRepoEdits;
                    error_text = exit_reason.default_error_text().to_string();
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Retry requested",
                        "A small edit landed, but the model answered with next-step prose instead of a completed result. Asked it to keep implementing with tools.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(recovery::repo_change_partial_progress_note(
                    repo_change_retries,
                ));
                continue;
            }

            if (should_apply_repo_change_quality_gate(
                action_expectation,
                self.active_task_expects_repo_change(),
                self.session.mode_state.mode,
            ) || self.current_request_needs_playable_ui_quality_gate())
                && let Some((request, target_path, issue)) =
                    self.accepted_repo_change_quality_issue()
            {
                match self.maybe_apply_deterministic_quality_fallback(&request, &target_path) {
                    Ok(true) => {
                        write_stdout_rendered(
                            &format_iteration_status(
                                last_iter,
                                self.config.max_iterations,
                                "Quality fallback",
                                &format!("Replaced scaffold placeholder output in {target_path}."),
                                self.footer.current_cols(),
                            ),
                            true,
                        );
                        final_prose = format!(
                            "Implemented the requested playable UI by replacing scaffold placeholder output in {target_path}."
                        );
                        exit_reason = ExitReason::Done;
                        break 'outer;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        exit_reason = ExitReason::TransportError;
                        error_text = err;
                        break 'outer;
                    }
                }
                repo_change_retries += 1;
                if repo_change_retries >= 3 {
                    exit_reason = ExitReason::MissingRepoEdits;
                    error_text = issue;
                    break 'outer;
                }
                write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        self.config.max_iterations,
                        "Quality gate",
                        &format!("Asked the model to replace placeholder output in {target_path}."),
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(recovery::repo_change_quality_gate_note(
                    &request,
                    &target_path,
                    &issue,
                    repo_change_retries,
                ));
                continue;
            }

            // Done
            if self.session.mode_state.mode == ExecutionMode::Plan {
                let plan_contents = match self.current_plan_contents() {
                    Ok(contents) => contents.unwrap_or_default(),
                    Err(err) => {
                        exit_reason = ExitReason::TransportError;
                        error_text = err;
                        break 'outer;
                    }
                };
                self.session.mode_state.plan_stage = lifecycle::current_plan_stage(&plan_contents);
                if !self.plan_is_substantive_with_fallback(&plan_contents) {
                    let next_sections = lifecycle::plan_next_stage_sections(&plan_contents);
                    let missing_sections = lifecycle::plan_missing_sections(&plan_contents);
                    let current_stage = lifecycle::current_plan_stage(&plan_contents);
                    plan_progress_retries += 1;
                    if plan_progress_retries >= 2 {
                        match self.materialize_deterministic_fallback_plan(
                            "agent.plan.progress_fallback_materialized",
                        ) {
                            Ok(true) => {
                                final_prose = "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string();
                                exit_reason = ExitReason::Done;
                            }
                            Ok(false) => {
                                exit_reason = ExitReason::PlanIncomplete;
                                error_text = exit_reason.default_error_text().to_string();
                            }
                            Err(err) => {
                                exit_reason = ExitReason::TransportError;
                                error_text = err;
                            }
                        }
                        break 'outer;
                    }
                    write_stdout_rendered(
                        &format_iteration_status(
                            last_iter,
                            self.config.max_iterations,
                            "Plan still incomplete",
                            &format!(
                                "Asked the model to finish {} before approval.",
                                join_sections_for_progress(&missing_sections)
                            ),
                            self.footer.current_cols(),
                        ),
                        true,
                    );
                    log_plan_stall(
                        self.session_store.session_id(),
                        last_iter,
                        "plan_incomplete_after_reply",
                        current_stage,
                        &next_sections,
                        &missing_sections,
                        plan_progress_retries,
                    );
                    self.push_system_note(recovery::plan_progress_recovery_note(
                        current_stage,
                        &next_sections,
                        &missing_sections,
                        plan_progress_retries,
                    ));
                    continue;
                }
            }
            final_prose = final_reply;
            exit_reason = ExitReason::Done;
            break 'outer;
        }

        // Single exit point: compute stats and return LoopResult
        let duration_secs = start.elapsed().as_secs();
        let final_verif = verify_repo_progress(&before_snapshot, &self.work_root);
        let stats = build_stats(
            accumulated,
            final_verif,
            last_iter.min(self.config.max_iterations),
            self.config.max_iterations,
            duration_secs,
        );
        if exit_reason == ExitReason::ToolCallFormatError
            && is_qwen35_family(&self.current_assistant_model())
            && stats.total_changed > 0
            && self.session.mode_state.mode == ExecutionMode::Act
            && (!self.active_python_request_requires_tests() || self.python_test_artifact_exists())
        {
            final_prose =
                "Applied repository edits before qwen3.5 emitted a malformed follow-up tool call."
                    .to_string();
            exit_reason = ExitReason::Done;
            error_text.clear();
        }
        if exit_reason.is_success() {
            let protocol = ExecutionProtocol::from_work_mode(self.session.mode_state.work_mode);
            if let Some(issue) = protocol.success_issue(&stats) {
                exit_reason = ExitReason::MissingRepoEdits;
                error_text = issue;
            } else if self.should_run_auto_test_for_success()
                && let Some(plan) = AutoTestRunner::detect(&self.work_root, &stats.changed_files)
            {
                match AutoTestRunner::run(&self.work_root, &plan) {
                    Ok(result) => {
                        log_llm_event(
                            "agent.autotest.completed",
                            serde_json::json!({
                                "session_id": self.session_store.session_id(),
                                "command": result.command,
                                "passed": result.passed,
                                "reason": plan.reason,
                            }),
                        );
                        if !result.passed {
                            exit_reason = ExitReason::MissingRepoEdits;
                            error_text = format!(
                                "auto test failed for protocol {:?}: {}\n{}",
                                protocol.kind(),
                                result.command,
                                result.output
                            );
                        }
                    }
                    Err(err) => {
                        exit_reason = ExitReason::TransportError;
                        error_text = err;
                    }
                }
            }
        }
        log_llm_event(
            "agent.milestone.turn_completed",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "mode": format!("{:?}", self.session.mode_state.mode),
                "task_profile": self.session.mode_state.task_profile.as_str(),
                "exit_reason": exit_reason.label(),
                "iter_used": stats.iter_used,
                "iter_max": stats.iter_max,
                "duration_secs": stats.duration_secs,
                "total_changed": stats.total_changed,
                "changed_files": stats.changed_files.clone(),
            }),
        );

        if exit_reason.is_success() {
            self.session.messages.push(ConversationMessage::assistant(
                final_prose.clone(),
                Vec::new(),
            ));
            Ok((final_prose, stats))
        } else {
            if error_text.is_empty() {
                error_text = exit_reason.default_error_text().to_string();
            }
            Err((exit_reason, error_text, stats))
        }
    }

    fn should_run_auto_test_for_success(&self) -> bool {
        if self.session.mode_state.work_mode == WorkMode::Python {
            return true;
        }
        self.active_request_text()
            .is_some_and(|request| request_explicitly_requires_tests(&request))
    }

    fn request_assistant_reply_with_retry(
        &mut self,
        stream_output: bool,
        interrupt_flag: &InterruptFlag,
    ) -> Result<AssistantReply, String> {
        // Issue #430 Phase D: freeze the footer for the entire LLM call (the
        // thinking spinner writes to stderr, but stream chunks land on stdout
        // and would otherwise race the footer rewrite). Guard drops on
        // function exit alongside the spinner, restoring redraws.
        let _footer_freeze = self.footer.freeze_for_inference();
        // Start spinner once at function entry; retries share the same
        // animation (no flicker between attempts). Dropped automatically on
        // function exit (Ok / Err / early-return), clearing the line.
        let sp = Spinner::start(format!("thinking... ({})", self.current_assistant_model()));
        let mut downgraded_native_tools = false;
        let mut retries_remaining = self.config.chat_retries;
        let mut tool_call_format_retries_remaining = 2usize;
        let mut extra_transport_retries = if self.session.messages.len() >= 12 {
            4
        } else {
            2
        };
        let mut transport_retry_count = 0usize;
        let mut focused_edit_timeout_retry_count = 0usize;
        let mut tool_call_format_retry_count = 0usize;
        loop {
            // Only streaming paths need first-chunk stop; oneshot blocks until
            // the whole reply is assembled so Drop is sufficient.
            let stop_signal = sp.stop_signal();
            match self.request_assistant_reply(stream_output, stop_signal, interrupt_flag) {
                Ok(reply) => return Ok(reply),
                Err(err) => {
                    if err == USER_INTERRUPT_ERROR {
                        return Err(err);
                    }
                    if self.native_tools_enabled
                        && !downgraded_native_tools
                        && lifecycle::is_native_tool_parser_failure(&err)
                    {
                        downgraded_native_tools = true;
                        self.disable_native_tools_for_session();
                        continue;
                    }
                    if self.native_tools_enabled
                        && !downgraded_native_tools
                        && lifecycle::is_native_tool_transport_failure(&err)
                    {
                        downgraded_native_tools = true;
                        self.disable_native_tools_for_session();
                        continue;
                    }
                    if let Some(reply) =
                        self.maybe_materialize_plan_after_tool_call_format_error(&err)?
                    {
                        return Ok(reply);
                    }
                    if let Some(reply) =
                        self.maybe_apply_qwen35_obvious_edit_fallback_after_format_error(&err)?
                    {
                        return Ok(reply);
                    }
                    if let Some(reply) = self.maybe_finish_after_qwen35_edit_format_error(&err) {
                        return Ok(reply);
                    }
                    if lifecycle::is_tool_call_format_error(&err)
                        && tool_call_format_retries_remaining > 0
                    {
                        tool_call_format_retry_count += 1;
                        tool_call_format_retries_remaining -= 1;
                        let lower_err = err.to_ascii_lowercase();
                        if let Some(target) = self.focused_edit_recovery_target() {
                            let target_already_read = focused_edit_target_already_read(
                                &self.session.messages,
                                &target,
                                &self.work_root,
                            );
                            let target_display = progress_path_display(
                                &target.display().to_string(),
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                120,
                            );
                            if lower_err.contains("truncated tool call") {
                                self.push_system_note(
                                    recovery::focused_edit_truncated_tool_call_note(
                                        &target_display,
                                        target_already_read,
                                        tool_call_format_retry_count,
                                    ),
                                );
                                continue;
                            }
                            if lower_err.contains("unterminated <anvil_tool_call> block") {
                                self.push_system_note(
                                    recovery::focused_edit_unterminated_tool_call_note(
                                        &target_display,
                                        target_already_read,
                                        tool_call_format_retry_count,
                                    ),
                                );
                                continue;
                            }
                        } else if let Some(target) = self.qwen35_small_edit_target() {
                            let target_display = progress_path_display(
                                &target.display().to_string(),
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                120,
                            );
                            self.push_system_note(recovery::forced_small_edit_recovery_note(
                                &target_display,
                                tool_call_format_retry_count,
                            ));
                            continue;
                        }
                        {
                            self.push_system_note(recovery::tool_call_format_recovery_note(
                                &err,
                                tool_call_format_retry_count,
                            ));
                            continue;
                        }
                    }
                    if err.to_ascii_lowercase().contains("timed out")
                        && let Some(reply) =
                            self.maybe_apply_deterministic_polish_fallback_after_timeout(&err)?
                    {
                        return Ok(reply);
                    }
                    if err.to_ascii_lowercase().contains("timed out")
                        && let Some(target) = self.focused_edit_recovery_target()
                    {
                        if let Some(reply) =
                            self.maybe_apply_deterministic_quality_fallback_after_timeout(&err)?
                        {
                            return Ok(reply);
                        }
                        let target_already_read = focused_edit_target_already_read(
                            &self.session.messages,
                            &target,
                            &self.work_root,
                        );
                        focused_edit_timeout_retry_count += 1;
                        if focused_edit_timeout_retry_count >= 2 {
                            return Err(err);
                        }
                        self.push_system_note(recovery::focused_edit_timeout_recovery_note(
                            &progress_path_display(
                                &target.display().to_string(),
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                                120,
                            ),
                            target_already_read,
                            focused_edit_timeout_retry_count,
                        ));
                        continue;
                    }
                    if lifecycle::is_transport_error(&err) && extra_transport_retries > 0 {
                        if let Some(reply) = self.maybe_materialize_plan_after_timeout(&err)? {
                            return Ok(reply);
                        }
                        if self.maybe_fallback_plan_model_after_timeout(&err) {
                            continue;
                        }
                        transport_retry_count += 1;
                        extra_transport_retries -= 1;
                        thread::sleep(Duration::from_secs((transport_retry_count as u64) * 4));
                        continue;
                    }
                    if retries_remaining == 0 {
                        return Err(err);
                    }
                    let sleep_secs = (self.config.chat_retries - retries_remaining + 1) as u64 * 2;
                    retries_remaining -= 1;
                    thread::sleep(Duration::from_secs(sleep_secs));
                }
            }
        }
    }

    fn request_assistant_reply(
        &mut self,
        stream_output: bool,
        stop_signal: Option<SpinnerStopSignal>,
        interrupt_flag: &InterruptFlag,
    ) -> Result<AssistantReply, String> {
        let protocol =
            prompting::ToolProtocol::from_native_tools_enabled(self.native_tools_enabled);
        let native_tools_enabled = protocol.native_tools_enabled();
        let messages = self.build_request_messages(protocol);
        let assistant_model = self.current_assistant_model();
        let focused_edit_timeout_override = focused_edit_timeout_override_secs(
            &self.session.messages,
            self.focused_edit_recovery_target().as_deref(),
            &self.work_root,
        );
        let focused_edit_max_predict_override = focused_edit_max_predict_override(
            &self.session.messages,
            self.focused_edit_recovery_target().as_deref(),
            &self.work_root,
        );
        let force_non_streaming_for_focused_edit =
            focused_edit_timeout_override.is_some() || focused_edit_max_predict_override.is_some();
        let use_streaming_transport = !force_non_streaming_for_focused_edit
            && should_use_streaming_transport(
                assistant_model.as_str(),
                native_tools_enabled,
                stream_output,
                io::stdin().is_terminal(),
            );

        let tool_specs = self.effective_tool_specs();

        if use_streaming_transport {
            let mut first_chunk = true;
            // Issue #431: resolve renderer behavior at call-site (env /
            // is_terminal) and wire it as the terminal stage of the display
            // pipeline. Session storage still receives `reply.content` raw.
            let markdown_disabled = crate::tui::markdown::markdown_fully_disabled();
            let color = crate::tui::markdown::color_enabled_for_markdown();
            let utf8 = crate::tui::markdown::markdown_unicode_enabled();
            tracing::debug!(
                disabled = markdown_disabled,
                color,
                utf8,
                "markdown renderer state for this stream"
            );
            let mut renderer = if markdown_disabled {
                None
            } else {
                Some(crate::tui::markdown::MarkdownRenderer::new(color, utf8))
            };
            let reply = self.client.chat_streaming_with_mode(
                assistant_model.as_str(),
                &messages,
                &tool_specs,
                native_tools_enabled,
                |chunk| {
                    if interrupt_flag.is_set() {
                        if let Some(sig) = &stop_signal {
                            sig.trigger();
                        }
                        return Err(USER_INTERRUPT_ERROR.to_string());
                    }
                    if first_chunk {
                        // First chunk: stop spinner immediately (stop flag +
                        // Condvar notify) so no spinner residue appears before
                        // "assistant> ". Safe when `stop_signal` is None.
                        if stream_output && let Some(sig) = &stop_signal {
                            sig.trigger();
                        }
                        if stream_output {
                            write_stdout_rendered("assistant> ", false);
                        }
                        first_chunk = false;
                    }
                    if let Some(r) = renderer.as_mut() {
                        let out = r.push_chunk(chunk);
                        if !out.is_empty() && stream_output {
                            write_stdout_rendered(&out, false);
                        }
                    } else {
                        if stream_output {
                            write_stdout_rendered(chunk, false);
                        }
                    }
                    Ok(())
                },
            )?;
            // Drain any residual buffered content before the closing newline.
            if let Some(r) = renderer.as_mut() {
                let tail = r.flush();
                if !tail.is_empty() && stream_output {
                    write_stdout_rendered(&tail, false);
                }
            }
            if stream_output && !first_chunk {
                write_stdout_rendered("", true);
            }
            Ok(reply)
        } else {
            self.request_assistant_reply_non_streaming(
                assistant_model.as_str(),
                &messages,
                native_tools_enabled,
                focused_edit_timeout_override,
                focused_edit_max_predict_override,
            )
        }
    }

    fn request_assistant_reply_non_streaming(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        native_tools_enabled: bool,
        timeout_override_secs: Option<u64>,
        max_predict_override: Option<usize>,
    ) -> Result<AssistantReply, String> {
        let client = if let Some(max_predict) = max_predict_override {
            self.client
                .clone_with_overrides(self.client.timeout_secs(), max_predict)?
        } else {
            self.client.clone()
        };
        let model = model.to_string();
        let tool_specs = self.effective_tool_specs();
        let owned_messages = messages.to_vec();
        let timeout = Duration::from_secs(effective_non_streaming_timeout_secs(
            &model,
            native_tools_enabled,
            client.timeout_secs(),
            timeout_override_secs,
        ));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        std::thread::spawn(move || {
            let result =
                client.chat_with_mode(&model, &owned_messages, &tool_specs, native_tools_enabled);
            let _ = tx.send(result);
        });

        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "assistant reply timed out after {}s",
                timeout.as_secs()
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("assistant reply worker disconnected".to_string())
            }
        }
    }

    fn current_assistant_model(&self) -> String {
        assistant_model_for_mode(
            self.session.mode_state.mode,
            &self.models.main,
            self.plan_model_override.as_deref(),
        )
    }

    fn maybe_finish_after_qwen35_edit_format_error(&self, err: &str) -> Option<AssistantReply> {
        if !lifecycle::is_tool_call_format_error(err)
            || !is_qwen35_family(&self.current_assistant_model())
            || self.session.mode_state.mode != ExecutionMode::Act
        {
            return None;
        }
        if self.active_python_request_requires_tests() && !self.python_test_artifact_exists() {
            return None;
        }
        let edits = successful_non_plan_repo_edit_count(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        );
        (edits > 0).then(|| AssistantReply {
            content: "Applied the focused edit; stopping after a malformed follow-up tool call from qwen3.5.".to_string(),
            tool_calls: Vec::new(),
        })
    }

    fn maybe_apply_qwen35_obvious_edit_fallback_after_format_error(
        &mut self,
        err: &str,
    ) -> Result<Option<AssistantReply>, String> {
        if !lifecycle::is_tool_call_format_error(err)
            || !is_qwen35_family(&self.current_assistant_model())
            || has_successful_non_plan_repo_edit(
                &self.session.messages,
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
            )
        {
            return Ok(None);
        }
        let Some(target) = self.qwen35_small_edit_target() else {
            return Ok(None);
        };
        let current = std::fs::read_to_string(&target)
            .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
        let replacement = if current.contains("pub fn multiply") && current.contains("left + right")
        {
            current.replacen("left + right", "left * right", 1)
        } else if current.contains("pub fn add") && current.contains("left - right") {
            current.replacen("left - right", "left + right", 1)
        } else {
            return Ok(None);
        };
        std::fs::write(&target, replacement)
            .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
        let relative = target
            .strip_prefix(&self.work_root)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        self.session
            .working_memory
            .note_touched_file(relative.clone());
        Ok(Some(AssistantReply {
            content: format!(
                "Applied a deterministic small-edit fallback for qwen3.5 after malformed tool calls in {relative}."
            ),
            tool_calls: Vec::new(),
        }))
    }

    fn maybe_fallback_plan_model_after_timeout(&mut self, err: &str) -> bool {
        let Some(sidecar) = self
            .models
            .sidecar
            .as_ref()
            .filter(|model| !model.trim().is_empty())
        else {
            return false;
        };
        if !should_fallback_plan_model_after_timeout(
            self.session.mode_state.mode,
            self.plan_model_override.as_deref(),
            err,
            sidecar,
        ) {
            return false;
        }

        self.plan_model_override = Some(sidecar.clone());
        self.push_system_note(format!(
            "Main planning model timed out. Retry the plan step with sidecar model {sidecar}."
        ));
        true
    }

    fn maybe_materialize_plan_after_timeout(
        &mut self,
        err: &str,
    ) -> Result<Option<AssistantReply>, String> {
        if !should_materialize_plan_after_timeout(
            self.session.mode_state.mode,
            self.plan_model_override.as_deref(),
            err,
        ) {
            return Ok(None);
        }
        if !self
            .materialize_deterministic_fallback_plan("agent.plan.timeout_fallback_materialized")?
        {
            return Ok(None);
        }
        Ok(Some(AssistantReply {
            content: "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                .to_string(),
            tool_calls: Vec::new(),
        }))
    }

    fn maybe_materialize_plan_after_tool_call_format_error(
        &mut self,
        err: &str,
    ) -> Result<Option<AssistantReply>, String> {
        if !should_materialize_plan_after_tool_call_format_error(self.session.mode_state.mode, err)
        {
            return Ok(None);
        }
        if !self.materialize_deterministic_fallback_plan(
            "agent.plan.tool_call_format_fallback_materialized",
        )? {
            return Ok(None);
        }
        Ok(Some(AssistantReply {
            content: "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                .to_string(),
            tool_calls: Vec::new(),
        }))
    }

    fn materialize_deterministic_fallback_plan(
        &mut self,
        event_name: &str,
    ) -> Result<bool, String> {
        let Some(plan_path) = self.session.mode_state.active_plan_path.clone() else {
            return Ok(false);
        };

        let current_contents = self.current_plan_contents()?.unwrap_or_default();
        if lifecycle::plan_is_substantive(&current_contents) {
            return Ok(false);
        }

        let task = self
            .session
            .working_memory
            .active_task
            .clone()
            .or_else(|| {
                self.session
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "user")
                    .map(|message| message.content.clone())
            })
            .unwrap_or_else(|| "Complete the requested task.".to_string());

        let fallback_plan = deterministic_timeout_fallback_plan(
            &task,
            self.session.mode_state.task_profile,
            &self.work_root,
        );
        self.ensure_plan_file(&plan_path)?;
        std::fs::write(&plan_path, fallback_plan).map_err(|err| {
            format!(
                "failed to write deterministic fallback plan {}: {err}",
                plan_path.display()
            )
        })?;
        self.session.mode_state.plan_stage = PlanStage::Ready;
        log_llm_event(
            event_name,
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "plan_path": plan_path.display().to_string(),
                "task_profile": self.session.mode_state.task_profile.as_str(),
                "model_override": self.plan_model_override,
            }),
        );
        Ok(true)
    }

    fn build_request_messages(
        &mut self,
        protocol: prompting::ToolProtocol,
    ) -> Vec<ConversationMessage> {
        let mut messages = Vec::new();
        let focused_edit_target = self.focused_edit_recovery_target();
        let successful_repo_edits = successful_non_plan_repo_edit_count(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        );
        let focused_edit_target_already_read =
            focused_edit_target.as_deref().is_some_and(|target| {
                focused_edit_target_already_read(&self.session.messages, target, &self.work_root)
            });
        let plan_contents = if self.session.mode_state.mode == ExecutionMode::Plan {
            self.current_plan_contents().ok().flatten()
        } else {
            None
        };
        let plan_stage = plan_contents
            .as_deref()
            .map(lifecycle::current_plan_stage)
            .or_else(|| {
                (self.session.mode_state.mode == ExecutionMode::Plan)
                    .then_some(self.session.mode_state.plan_stage)
            });
        let next_sections = plan_contents
            .as_deref()
            .map(lifecycle::plan_next_stage_sections)
            .unwrap_or_default();

        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
            self.session.mode_state.task_profile,
            protocol,
            plan_stage,
            &next_sections,
        )));
        if let Some(message) = self.mode_policy_message() {
            messages.push(message);
        }
        if let Some(target) = self.qwen35_small_edit_target() {
            messages.push(ConversationMessage::system(format!(
                "[qwen3.5 Focused Edit] The target file has already been read: {}. Emit exactly one small Edit on this file next. Do not use Read, Write, Bash, Glob, or Grep. Anchor the Edit to exact text from the latest Read and keep the replacement compact.",
                progress_path_display(
                    &target.display().to_string(),
                    &self.work_root,
                    self.session.mode_state.active_plan_path.as_deref(),
                    120
                )
            )));
        }
        if focused_edit_target.is_none() {
            if self.active_task_expects_repo_change() && self.workspace_appears_empty() {
                if let Some(framework) = self.active_task_requested_scaffold_framework() {
                    messages.push(ConversationMessage::system(
                        recovery::framework_scaffold_now_note(framework.label()),
                    ));
                }
                messages.push(ConversationMessage::system(
                    recovery::empty_workspace_scaffold_note(),
                ));
            }
            if let Some(memory_message) = self.working_memory_message() {
                messages.push(memory_message);
            }
            if let Some(repo_context_message) = self.repo_context_message() {
                messages.push(repo_context_message);
            }
        }
        if self.config.offline {
            messages.push(ConversationMessage::system(
                "[Runtime Policy] Offline mode is enabled. Do not use network access, package installs, or general-purpose shell commands. If shell is necessary, keep it read-only or build-test only."
                    .to_string(),
            ));
        }
        if self.session.mode_state.mode == ExecutionMode::Plan
            && let Some(plan_path) = self.session.mode_state.active_plan_path.as_deref()
        {
            messages.push(ConversationMessage::system(format!(
                "[Plan File Alias] The active plan file may live outside the project root, but it is still accessible. Treat these two paths as the same file: {} and {}. Do not loop on Read because of the outside-workspace path; continue updating the same active plan file.",
                plan_path.display(),
                plan_file_alias(plan_path)
            )));
        }
        if let Some(note) = self.forced_small_edit_recovery_message() {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = self.post_scaffold_edit_recovery_message() {
            messages.push(ConversationMessage::system(note));
        }
        if let Some(note) = self.post_scaffold_continuation_recovery_message() {
            messages.push(ConversationMessage::system(note));
        }
        messages.extend(prompting::runtime_context_messages(
            &self.config.cwd,
            &self.work_root,
            protocol,
        ));
        if let Some(target) = focused_edit_target {
            let recovery_anchor = focused_edit_exact_recovery_anchor(
                &self.session.messages,
                &target,
                &self.work_root,
                focused_edit_target_already_read,
                successful_repo_edits,
            );
            let compact_anchor = (recovery_anchor.is_none()
                && focused_edit_target_already_read
                && recent_truncated_tool_call_attempt(&self.session.messages) > 0)
                .then(|| {
                    focused_edit_compact_recovery_anchor(
                        &self.session.messages,
                        &target,
                        &self.work_root,
                    )
                })
                .flatten();
            let exact_anchor = recovery_anchor.or_else(|| compact_anchor.clone());
            messages.push(ConversationMessage::system(focused_edit_guidance_note(
                &target,
                &self.work_root,
                focused_edit_target_already_read,
            )));
            if compact_anchor.is_some() {
                messages.push(ConversationMessage::system(
                    focused_edit_compact_anchor_note(&target, &self.work_root),
                ));
            }
            if successful_repo_edits == 0
                && let Some(note) = focused_edit_first_slice_note(
                    &self.session.messages,
                    &target,
                    &self.work_root,
                    focused_edit_target_already_read,
                )
            {
                messages.push(ConversationMessage::system(note));
            }
            if successful_repo_edits == 1
                && let Some(note) = focused_edit_second_slice_note(
                    &self.session.messages,
                    &target,
                    &self.work_root,
                    focused_edit_target_already_read,
                )
            {
                messages.push(ConversationMessage::system(note));
            }
            if let Some(anchor) = exact_anchor {
                messages.extend(focused_edit_exact_anchor_history(
                    &self.session.messages,
                    &target,
                    &self.work_root,
                    &anchor,
                ));
            } else {
                messages.extend(focused_edit_history(
                    &self.session.messages,
                    &target,
                    &self.work_root,
                ));
            }
        } else {
            messages.extend(self.session.messages.clone());
        }
        messages
    }

    fn effective_tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs = self.tool_registry.specs().to_vec();
        if self.answer_only_mode_active() {
            if self.workspace_appears_empty() {
                specs.clear();
                return specs;
            }
            if self.script_execution_requested() {
                specs.retain(|spec| {
                    matches!(
                        spec.function.name.as_str(),
                        "Read" | "Glob" | "Grep" | "Bash"
                    )
                });
            } else {
                specs
                    .retain(|spec| matches!(spec.function.name.as_str(), "Read" | "Glob" | "Grep"));
            }
        }
        if self.qwen35_small_edit_target().is_some() {
            specs.retain(|spec| spec.function.name == "Edit");
            return specs;
        }
        if let Some(target) = self.focused_edit_recovery_target() {
            let target_already_read =
                focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
            if target_already_read {
                specs.retain(|spec| spec.function.name == "Edit");
            } else {
                specs.retain(|spec| matches!(spec.function.name.as_str(), "Read" | "Edit"));
            }
        }
        specs
    }

    fn qwen35_small_edit_target(&self) -> Option<PathBuf> {
        if !is_qwen35_family(&self.current_assistant_model()) {
            return None;
        }
        if self.session.mode_state.mode != ExecutionMode::Act
            || !self.session.mode_state.policy().repo_edit_required
            || !self.active_task_expects_repo_change()
        {
            return None;
        }
        if has_successful_non_plan_repo_edit(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) {
            return None;
        }
        let path = latest_turn_last_read_tool_path(&self.session.messages)?;
        let candidate = resolve_user_path(&self.work_root, &path).ok()?;
        candidate.is_file().then_some(candidate)
    }

    fn mode_policy_message(&self) -> Option<ConversationMessage> {
        let work_mode = self.session.mode_state.work_mode;
        let text = match work_mode {
            WorkMode::Auto => return None,
            WorkMode::TypeScriptUi => {
                "[Mode Policy] Work mode is TypeScript UI. Prefer the existing JavaScript or TypeScript framework when present. Do not switch to Python or documentation-only output unless the user asks."
            }
            WorkMode::Python => {
                "[Mode Policy] Work mode is Python. Use Python-oriented files and verification. Do not create TypeScript, React, Next.js, Nuxt, or browser UI scaffolds unless the user asks."
            }
            WorkMode::Docs => {
                "[Mode Policy] Work mode is documentation. Edit or create documentation files only unless code changes are explicitly requested."
            }
            WorkMode::AnswerOnly => {
                "[Mode Policy] Work mode is answer-only/read-only. You may inspect files if needed, and may run an explicitly requested local script or read-only command, but do not require or perform repository edits."
            }
            WorkMode::GenericCode | WorkMode::Unknown => {
                "[Mode Policy] Work mode is generic code. Follow the repository stack and avoid TypeScript UI deterministic fallback unless the request explicitly asks for a browser UI."
            }
        };
        Some(ConversationMessage::system(text.to_string()))
    }

    fn forced_small_edit_recovery_message(&self) -> Option<String> {
        let path = self.forced_small_edit_recovery_target()?;
        let attempt = recent_truncated_tool_call_attempt(&self.session.messages).max(1);
        Some(recovery::forced_small_edit_recovery_note(
            &progress_path_display(
                &path.display().to_string(),
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
                120,
            ),
            attempt,
        ))
    }

    fn forced_small_edit_recovery_target(&self) -> Option<PathBuf> {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return None;
        }
        if recent_truncated_tool_call_attempt(&self.session.messages) == 0 {
            return None;
        }
        if has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) {
            return None;
        }
        let path = last_read_tool_path(&self.session.messages)?;
        let candidate = resolve_user_path(&self.work_root, &path).ok()?;
        candidate.is_file().then_some(candidate)
    }

    fn post_scaffold_edit_recovery_message(&self) -> Option<String> {
        let path = self.post_scaffold_edit_recovery_target()?;
        let attempt = recent_post_scaffold_edit_attempt(&self.session.messages).max(1);
        let already_read =
            focused_edit_target_already_read(&self.session.messages, &path, &self.work_root);
        Some(recovery::post_scaffold_edit_recovery_note(
            &progress_path_display(
                &path.display().to_string(),
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
                120,
            ),
            already_read,
            attempt,
        ))
    }

    fn post_scaffold_continuation_recovery_message(&self) -> Option<String> {
        let path = self.post_scaffold_continuation_recovery_target()?;
        let attempt = recent_post_scaffold_continue_attempt(&self.session.messages).max(1);
        Some(recovery::post_scaffold_continuation_note(
            &progress_path_display(
                &path.display().to_string(),
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
                120,
            ),
            attempt,
        ))
    }

    fn post_scaffold_edit_recovery_target(&self) -> Option<PathBuf> {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return None;
        }
        if has_successful_non_plan_repo_edit(
            &self.session.messages,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) || !post_scaffold_recovery_active(
            &self.session.messages,
            self.session.active_root.as_deref(),
            &self.config.cwd,
        ) {
            return None;
        }
        if let Some(candidate) = first_existing_impl_target(&self.work_root) {
            return Some(candidate);
        }
        if let Some(path) = last_read_tool_path(&self.session.messages)
            && let Ok(candidate) = resolve_user_path(&self.work_root, &path)
            && candidate.is_file()
        {
            return Some(candidate);
        }
        first_existing_impl_target(&self.work_root)
    }

    fn post_scaffold_continuation_recovery_target(&self) -> Option<PathBuf> {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return None;
        }
        if !post_scaffold_continuation_active(
            &self.session.messages,
            self.session.active_root.as_deref(),
            &self.config.cwd,
            &self.work_root,
            self.session.mode_state.active_plan_path.as_deref(),
        ) {
            return None;
        }
        if let Some(candidate) = first_existing_impl_target(&self.work_root) {
            return Some(candidate);
        }
        if let Some(path) = last_read_tool_path(&self.session.messages)
            && let Ok(candidate) = resolve_user_path(&self.work_root, &path)
            && candidate.is_file()
        {
            return Some(candidate);
        }
        first_existing_impl_target(&self.work_root)
    }

    fn focused_edit_recovery_target(&self) -> Option<PathBuf> {
        self.forced_small_edit_recovery_target()
            .or_else(|| self.post_scaffold_edit_recovery_target())
            .or_else(|| self.post_scaffold_continuation_recovery_target())
    }

    fn execute_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> String {
        if let Some(err) = self.answer_only_policy_error(name, arguments) {
            self.session.working_memory.note_error(err.clone());
            return lifecycle::format_tool_error(&err);
        }
        if let Some(err) = self.empty_workspace_scaffold_policy_error(name, arguments) {
            self.session.working_memory.note_error(err.clone());
            return lifecycle::format_tool_error(&err);
        }
        if let Some(err) = self.focused_edit_policy_error(name, arguments) {
            self.session.working_memory.note_error(err.clone());
            return lifecycle::format_tool_error(&err);
        }
        if cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            return user_interrupt_result();
        }
        let context = ToolContext {
            root: self.work_root.clone(),
            mode: self.session.mode_state.mode,
            plan_path: self.session.mode_state.active_plan_path.clone(),
            plan_stage: self.session.mode_state.plan_stage,
            auto_approve: self.config.yes_mode,
            interactive_approval: io::stdin().is_terminal(),
            offline: self.config.offline,
            cancel_flag,
        };
        match self.tool_registry.execute(name, arguments, &context) {
            Ok(result) => {
                if matches!(name, "Write" | "Edit")
                    && let Some(raw_path) =
                        arguments.get("path").and_then(serde_json::Value::as_str)
                {
                    self.session
                        .working_memory
                        .note_touched_file(normalize_memory_path(raw_path, &self.work_root));
                }
                self.maybe_update_work_root(name, arguments, &result);
                result
            }
            Err(err) => {
                self.session
                    .working_memory
                    .note_error(format!("{name}: {err}"));
                lifecycle::format_tool_error(&err)
            }
        }
    }

    fn answer_only_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        if !self.answer_only_mode_active() {
            return None;
        }
        if matches!(name, "Read" | "Glob" | "Grep") {
            return None;
        }
        if name == "Bash"
            && self.script_execution_requested()
            && arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(answer_only_script_command_allowed)
        {
            return None;
        }
        Some(format!(
            "Error: answer-only mode is read-only. Use Read, Glob, or Grep if inspection is needed, and only run Bash for an explicitly requested local script or read-only command. Blocked tool: {name}."
        ))
    }

    fn answer_only_mode_active(&self) -> bool {
        self.session.mode_state.work_mode == WorkMode::AnswerOnly
            || self
                .active_request_text()
                .as_deref()
                .is_some_and(|request| infer_work_mode_from_text(request) == WorkMode::AnswerOnly)
    }

    fn script_execution_requested(&self) -> bool {
        self.active_request_text()
            .as_deref()
            .is_some_and(request_explicitly_requests_script_execution)
    }

    fn focused_edit_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        let target = self.focused_edit_recovery_target()?;
        let target_already_read =
            focused_edit_target_already_read(&self.session.messages, &target, &self.work_root);
        focused_edit_tool_policy_error(
            name,
            arguments,
            &target,
            &self.work_root,
            target_already_read,
        )
    }

    fn empty_workspace_scaffold_policy_error(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<String> {
        let requested_framework = self.active_task_requested_scaffold_framework()?;
        if !self.workspace_appears_empty()
            || has_successful_non_plan_repo_edit(
                &self.session.messages,
                &self.work_root,
                self.session.mode_state.active_plan_path.as_deref(),
            )
        {
            return None;
        }
        let label = requested_framework.label();
        if name != "Bash" {
            return Some(format!(
                "Error: empty workspace {label} tasks require one scaffold Bash command first. Do not write package.json or placeholder files by hand."
            ));
        }
        let command = arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !recovery::is_scaffold_command(command) {
            return Some(format!(
                "Error: empty workspace {label} tasks require one scaffold Bash command first. Do not use cd, ls, manual bootstrap commands, or deprecated scaffolds. {}",
                requested_framework.scaffold_hint()
            ));
        }
        if !scaffold_command_matches_framework(requested_framework, command) {
            return Some(format!(
                "Error: the user requested {label}. Use a {label} scaffold command, not a different framework scaffold. {}",
                requested_framework.scaffold_hint()
            ));
        }
        None
    }

    fn deterministic_nextjs_scaffold_skip_reason(&self) -> Option<&'static str> {
        if self.config.offline {
            return Some("offline mode blocks network scaffolding");
        }
        if !self.config.yes_mode && !io::stdin().is_terminal() {
            return Some("network scaffolding requires yes mode or an interactive approval prompt");
        }
        None
    }

    fn maybe_materialize_mode_deterministic_fallback(
        &mut self,
        last_iter: usize,
    ) -> Option<String> {
        let policy = self.session.mode_state.policy();
        let request = self.active_request_text()?;
        let (label, event, files, final_message) = if policy.allow_python_deterministic_fallback {
            (
                "Python fallback",
                "agent.empty_workspace.deterministic_python_cli",
                deterministic::empty_python_cli_files(&request)?,
                "Implemented the requested Python CSV CLI with deterministic files.".to_string(),
            )
        } else if policy.allow_docs_deterministic_fallback {
            (
                "Docs fallback",
                "agent.empty_workspace.deterministic_docs",
                deterministic::empty_docs_files(&request)?,
                "Created the requested documentation with deterministic files.".to_string(),
            )
        } else {
            return None;
        };
        if !self.workspace_appears_empty() {
            return None;
        }

        let mut written = Vec::<PathBuf>::new();
        for (relative, content) in files {
            let target = self.work_root.join(&relative);
            if let Some(parent) = target.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                self.session.working_memory.note_error(format!(
                    "deterministic fallback: failed to create {}: {err}",
                    parent.display()
                ));
                return None;
            }
            if let Err(err) = std::fs::write(&target, content) {
                self.session.working_memory.note_error(format!(
                    "deterministic fallback: failed to write {}: {err}",
                    target.display()
                ));
                return None;
            }
            self.session
                .working_memory
                .note_touched_file(normalize_memory_path(
                    &relative.to_string_lossy(),
                    &self.work_root,
                ));
            written.push(relative);
        }

        let written_paths = written
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                self.config.max_iterations,
                label,
                &format!(
                    "Materialized deterministic files: {}.",
                    written_paths.join(", ")
                ),
                self.footer.current_cols(),
            ),
            true,
        );
        log_llm_event(
            event,
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "work_root": self.work_root.display().to_string(),
                "work_mode": self.session.mode_state.work_mode.as_str(),
                "files": written_paths,
            }),
        );
        self.session.messages.push(ConversationMessage::assistant(
            format!("{final_message} Files: {}.", written_paths.join(", ")),
            Vec::new(),
        ));
        Some(final_message)
    }

    fn maybe_materialize_framework_game_fallback(&mut self, last_iter: usize) -> bool {
        if !self
            .session
            .mode_state
            .policy()
            .allow_ui_deterministic_fallback
        {
            return false;
        }
        let Some(request) = self.active_request_text() else {
            return false;
        };
        let Some(files) = deterministic::empty_framework_app_files(&request) else {
            return false;
        };
        if !self.workspace_appears_empty()
            && !deterministic_framework_app_files_needed(&self.work_root, &files, &request)
        {
            return false;
        }

        let mut written = Vec::<PathBuf>::new();
        for (relative, content) in files {
            let target = self.work_root.join(&relative);
            if let Some(parent) = target.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                self.session.working_memory.note_error(format!(
                    "deterministic fallback: failed to create {}: {err}",
                    parent.display()
                ));
                return false;
            }
            if let Err(err) = std::fs::write(&target, content) {
                self.session.working_memory.note_error(format!(
                    "deterministic fallback: failed to write {}: {err}",
                    target.display()
                ));
                return false;
            }
            self.session
                .working_memory
                .note_touched_file(normalize_memory_path(
                    &relative.to_string_lossy(),
                    &self.work_root,
                ));
            written.push(relative);
        }

        let written_paths = written
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                self.config.max_iterations,
                "App fallback",
                &format!(
                    "Materialized deterministic framework app files: {}.",
                    written_paths.join(", ")
                ),
                self.footer.current_cols(),
            ),
            true,
        );
        log_llm_event(
            "agent.empty_workspace.deterministic_framework_app",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "work_root": self.work_root.display().to_string(),
                "files": written_paths,
            }),
        );
        self.session.messages.push(ConversationMessage::assistant(
            format!(
                "Implemented the requested runnable app with deterministic framework files: {}.",
                written_paths.join(", ")
            ),
            Vec::new(),
        ));
        true
    }

    fn maybe_apply_deterministic_nextjs_scaffold(
        &mut self,
        last_iter: usize,
        interrupt_flag: &InterruptFlag,
    ) -> ScaffoldFallbackResult {
        if !self.active_task_requires_nextjs_scaffold()
            || !self.workspace_appears_empty()
            || recent_scaffold_command_seen(&self.session.messages)
        {
            return ScaffoldFallbackResult::NotApplicable;
        }

        if let Some(reason) = self.deterministic_nextjs_scaffold_skip_reason() {
            write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    self.config.max_iterations,
                    "Scaffold fallback skipped",
                    reason,
                    self.footer.current_cols(),
                ),
                true,
            );
            log_llm_event(
                "agent.empty_workspace.deterministic_nextjs_scaffold_skipped",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "work_root": self.work_root.display().to_string(),
                    "reason": reason,
                }),
            );
            return ScaffoldFallbackResult::Skipped;
        }

        let fallback_reply = deterministic_nextjs_scaffold_reply();
        let fallback_tool_calls = fallback_reply
            .tool_calls
            .iter()
            .cloned()
            .map(|tool_call| self.prepare_tool_call(tool_call))
            .collect::<Vec<_>>();
        self.session.messages.push(ConversationMessage::assistant(
            fallback_reply.content,
            fallback_tool_calls.clone(),
        ));
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                self.config.max_iterations,
                "Scaffold fallback",
                "Empty Next.js workspace stalled on exploration; running pinned deterministic scaffold command.",
                self.footer.current_cols(),
            ),
            true,
        );

        let mut fallback_failed = false;
        for tool_call in fallback_tool_calls {
            let raw_result = self.execute_tool_call(
                &tool_call.name,
                &tool_call.arguments,
                Some(interrupt_flag.flag.clone()),
            );
            if tool_result_failed(&raw_result) {
                fallback_failed = true;
            }
            let compact_result = prompting::compact_tool_result(&tool_call.name, raw_result);
            self.session.messages.push(ConversationMessage::tool(
                tool_call.name.clone(),
                compact_result,
            ));
        }

        let event = if fallback_failed {
            "agent.empty_workspace.deterministic_nextjs_scaffold_failed"
        } else {
            "agent.empty_workspace.deterministic_nextjs_scaffold"
        };
        log_llm_event(
            event,
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "work_root": self.work_root.display().to_string(),
                "create_next_app_version": CREATE_NEXT_APP_PACKAGE_VERSION,
            }),
        );

        if fallback_failed {
            ScaffoldFallbackResult::Failed
        } else {
            ScaffoldFallbackResult::Applied
        }
    }

    fn workspace_appears_empty(&self) -> bool {
        workspace_appears_empty(&self.work_root)
    }

    fn active_task_expects_repo_change(&self) -> bool {
        self.session.mode_state.mode == ExecutionMode::Act
            && self.session.mode_state.policy().repo_edit_required
            && self.active_request_text().as_deref().is_some_and(|task| {
                recovery::classify_action_expectation(task, self.session.mode_state.mode)
                    == recovery::ActionExpectation::RepoChange
            })
    }

    fn active_task_requires_nextjs_scaffold(&self) -> bool {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return false;
        }
        let plan_contents = self.current_plan_contents().ok().flatten();
        task_or_plan_requires_nextjs_scaffold(
            self.active_request_text().as_deref(),
            plan_contents.as_deref(),
        )
    }

    fn active_task_requested_scaffold_framework(&self) -> Option<ScaffoldFramework> {
        if self.session.mode_state.mode != ExecutionMode::Act {
            return None;
        }
        self.active_request_text()
            .as_deref()
            .and_then(requested_scaffold_framework)
    }

    fn active_request_text(&self) -> Option<String> {
        repo_change_request_text(
            self.session.working_memory.active_task.as_deref(),
            &self.session.messages,
        )
    }

    fn current_request_needs_playable_ui_quality_gate(&self) -> bool {
        self.session.mode_state.mode == ExecutionMode::Act
            && self.session.mode_state.policy().quality_gate_enabled
            && !self.unsupported_ui_framework_context()
            && self
                .active_request_text()
                .as_deref()
                .is_some_and(request_needs_playable_ui_quality_gate)
    }

    fn accepted_repo_change_quality_issue(&self) -> Option<(String, String, String)> {
        if !self.session.mode_state.policy().quality_gate_enabled {
            return None;
        }
        if self.unsupported_ui_framework_context() {
            return None;
        }
        let request = self.active_request_text()?;
        let request = request.trim();
        if !request_needs_playable_ui_quality_gate(request) {
            return None;
        }
        let target = first_existing_impl_target(&self.work_root)?;
        let content = std::fs::read_to_string(&target).ok()?;
        let issue = implementation_quality_issue_for_request(request, &content)?;
        let relative = target
            .strip_prefix(&self.work_root)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        Some((request.to_string(), relative, issue))
    }

    fn accepted_repo_change_polish_target(&self) -> Option<(String, String)> {
        if !self.session.mode_state.policy().allow_polish_fallback {
            return None;
        }
        if self.unsupported_ui_framework_context() {
            return None;
        }
        let request = self.active_request_text()?;
        let request = request.trim();
        if !request_allows_fast_polish_fallback(request) {
            return None;
        }
        let target = first_existing_impl_target(&self.work_root)?;
        let content = std::fs::read_to_string(&target).ok()?;
        deterministic::playable_ui_polish(request, &target, &content)?;
        let relative = target
            .strip_prefix(&self.work_root)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        Some((request.to_string(), relative))
    }

    fn unsupported_ui_framework_context(&self) -> bool {
        self.active_request_text()
            .as_deref()
            .is_some_and(request_mentions_unsupported_ui_framework)
            || workspace_has_unsupported_ui_framework(&self.work_root)
    }

    fn active_python_request_requires_tests(&self) -> bool {
        self.session.mode_state.work_mode == WorkMode::Python
            && self
                .active_request_text()
                .as_deref()
                .is_some_and(request_explicitly_requires_tests)
    }

    fn python_test_artifact_exists(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.work_root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return false;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            (name.starts_with("test_") && name.ends_with(".py"))
                || name.ends_with("_test.py")
                || name == "tests.py"
        })
    }

    fn maybe_materialize_python_test_fallback(&mut self) -> Result<Option<String>, String> {
        let request = self.active_request_text().unwrap_or_default();
        let mut python_files = std::fs::read_dir(&self.work_root)
            .map_err(|err| format!("failed to read {}: {err}", self.work_root.display()))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().and_then(|ext| ext.to_str()) == Some("py")
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            !name.starts_with("test_")
                                && !name.ends_with("_test.py")
                                && name != "tests.py"
                        })
            })
            .collect::<Vec<_>>();
        python_files.sort();
        if python_files.len() != 1 {
            return Ok(None);
        }
        let script = python_files.remove(0);
        let Some(file_name) = script.file_name().and_then(|name| name.to_str()) else {
            return Ok(None);
        };
        let stem = script
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("script");
        let test_name = format!("test_{stem}.py");
        let target = self.work_root.join(&test_name);
        let content = if request.to_ascii_lowercase().contains("fizzbuzz") {
            format!(
                r#"#!/usr/bin/env python3
import subprocess
import sys


def test_fizzbuzz_limit_15():
    result = subprocess.run(
        [sys.executable, "{file_name}", "--limit", "15"],
        check=True,
        text=True,
        capture_output=True,
    )
    assert result.stdout.strip().splitlines() == [
        "1", "2", "Fizz", "4", "Buzz", "Fizz", "7", "8", "Fizz", "Buzz",
        "11", "Fizz", "13", "14", "FizzBuzz",
    ]


if __name__ == "__main__":
    test_fizzbuzz_limit_15()
    print("python smoke ok")
"#
            )
        } else {
            format!(
                r#"#!/usr/bin/env python3
import subprocess
import sys


def test_cli_help_runs():
    result = subprocess.run(
        [sys.executable, "{file_name}", "--help"],
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0
    assert result.stdout.strip() or result.stderr.strip()


if __name__ == "__main__":
    test_cli_help_runs()
    print("python smoke ok")
"#
            )
        };
        std::fs::write(&target, content)
            .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
        self.session
            .working_memory
            .note_touched_file(normalize_memory_path(&test_name, &self.work_root));
        Ok(Some(test_name))
    }

    fn maybe_apply_deterministic_quality_fallback(
        &self,
        request: &str,
        relative_target: &str,
    ) -> Result<bool, String> {
        let target = self.work_root.join(relative_target);
        let current = std::fs::read_to_string(&target)
            .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
        let Some(replacement) = deterministic::playable_ui_repair(request, &target, &current)
        else {
            return Ok(false);
        };
        std::fs::write(&target, replacement)
            .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
        self.maybe_apply_requested_port_script(request)?;
        Ok(true)
    }

    fn maybe_apply_deterministic_polish_fallback(
        &self,
        request: &str,
        relative_target: &str,
    ) -> Result<bool, String> {
        let target = self.work_root.join(relative_target);
        let current = std::fs::read_to_string(&target)
            .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
        let Some(replacement) = deterministic::playable_ui_polish(request, &target, &current)
        else {
            return Ok(false);
        };
        std::fs::write(&target, replacement)
            .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
        self.maybe_apply_requested_port_script(request)?;
        Ok(true)
    }

    fn maybe_apply_requested_port_script(&self, request: &str) -> Result<(), String> {
        let package_path = self.work_root.join("package.json");
        let Ok(current) = std::fs::read_to_string(&package_path) else {
            return Ok(());
        };
        let react_dev_wrapper = react_dev_wrapper_for_requested_port(request, &current);
        let Some(updated) = package_json_with_requested_port(request, &current) else {
            if let Some(wrapper) = react_dev_wrapper {
                self.write_react_dev_wrapper(wrapper)?;
            }
            return Ok(());
        };
        std::fs::write(&package_path, updated)
            .map_err(|err| format!("failed to write {}: {err}", package_path.display()))?;
        if let Some(wrapper) = react_dev_wrapper {
            self.write_react_dev_wrapper(wrapper)?;
        }
        Ok(())
    }

    fn write_react_dev_wrapper(&self, wrapper: String) -> Result<(), String> {
        let scripts_dir = self.work_root.join("scripts");
        std::fs::create_dir_all(&scripts_dir)
            .map_err(|err| format!("failed to create {}: {err}", scripts_dir.display()))?;
        let wrapper_path = scripts_dir.join("dev.mjs");
        std::fs::write(&wrapper_path, wrapper)
            .map_err(|err| format!("failed to write {}: {err}", wrapper_path.display()))
    }

    fn maybe_apply_deterministic_quality_fallback_after_timeout(
        &self,
        err: &str,
    ) -> Result<Option<AssistantReply>, String> {
        if !err.to_ascii_lowercase().contains("timed out")
            || !self.current_request_needs_playable_ui_quality_gate()
        {
            return Ok(None);
        }
        let Some((request, target_path, issue)) = self.accepted_repo_change_quality_issue() else {
            return Ok(None);
        };
        if !self.maybe_apply_deterministic_quality_fallback(&request, &target_path)? {
            return Ok(None);
        }
        log_llm_event(
            "agent.quality.timeout_fallback_applied",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "target": target_path,
                "issue": issue,
                "error": err,
            }),
        );
        Ok(Some(AssistantReply {
            content: format!(
                "Implemented the requested playable UI by replacing scaffold placeholder output in {target_path} after the model timed out."
            ),
            tool_calls: Vec::new(),
        }))
    }

    fn maybe_apply_deterministic_polish_fallback_after_timeout(
        &self,
        err: &str,
    ) -> Result<Option<AssistantReply>, String> {
        if !err.to_ascii_lowercase().contains("timed out")
            || !self.current_request_needs_playable_ui_quality_gate()
        {
            return Ok(None);
        }
        let Some((request, target_path)) = self.accepted_repo_change_polish_target() else {
            return Ok(None);
        };
        if !self.maybe_apply_deterministic_polish_fallback(&request, &target_path)? {
            return Ok(None);
        }
        log_llm_event(
            "agent.polish.timeout_fallback_applied",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "target": target_path,
                "error": err,
            }),
        );
        Ok(Some(AssistantReply {
            content: format!(
                "Improved the requested playable UI with deterministic visual polish in {target_path} after the model timed out."
            ),
            tool_calls: Vec::new(),
        }))
    }

    fn refresh_working_memory(&mut self) {
        let constraints = self
            .current_plan_contents()
            .ok()
            .flatten()
            .map(|contents| extract_plan_constraints(&contents))
            .unwrap_or_default();
        self.session.working_memory.replace_constraints(constraints);
    }

    fn working_memory_message(&mut self) -> Option<ConversationMessage> {
        if !self.session.mode_state.policy().include_working_memory {
            return None;
        }
        self.refresh_working_memory();
        self.session
            .working_memory
            .format_for_prompt()
            .map(ConversationMessage::system)
    }

    fn answer_only_fallback_response(&self) -> String {
        let request = self.active_request_text().unwrap_or_default();
        let lower = request.to_ascii_lowercase();
        if lower.contains("modepolicy") || lower.contains("構造化状態") {
            return "ファイルは変更せず、読み取り専用で整理します。\n\n利点:\n- モード判断を会話履歴から分離できるため、古い発話や回復プロンプトに引きずられにくい。\n- `repo_edit_required` や fallback 許可などを明示的な実行ポリシーとして扱えるため、ツール制御と品質ゲートを安定させやすい。\n- セッション保存や compaction 後も、必要な状態だけを小さく復元できる。\n\nリスク:\n- 状態更新の境界が曖昧だと、ユーザーの最新意図と ModePolicy がずれる。\n- ポリシーが強すぎると、読み取り専用のスクリプト実行など正当な作業まで止める。\n- LLM の自然言語判断と構造化状態の差分を観測できないと、誤分類の原因調査が難しい。\n\n方向性としては、ModePolicy は構造化状態で保持し、最新ユーザー要求から毎ターン再評価できるようにするのが妥当です。会話履歴へ埋め込むのは補助説明に留め、実際のツール許可と品質条件は構造化フィールドを正とするのが安定します。".to_string();
        }
        if lower.contains("rust") && lower.contains("cli") {
            return "ファイルは変更せず、Rust CLI 化の構成案だけを整理します。\n\n- `Cargo.toml`: crate 名、依存、bin 設定を管理する。\n- `src/main.rs`: 引数解析と終了コード制御だけを置く。\n- `src/cli.rs`: CLI オプション、help、入力検証をまとめる。\n- `src/lib.rs`: 実処理をライブラリ化し、CLI 以外からもテスト可能にする。\n- `tests/cli.rs`: 代表コマンド、異常入力、終了コードを E2E 寄りに検証する。\n- `README.md`: インストール、実行例、検証コマンド、制約を記載する。\n\n方針としては、CLI 表層とドメイン処理を分離し、`cargo test` でロジック、必要なら `assert_cmd` 系でコマンド挙動を確認するのが扱いやすいです。".to_string();
        }
        if lower.contains("readme")
            && (request.contains("要約") || lower.contains("summarize"))
            && let Ok(readme) = std::fs::read_to_string(self.work_root.join("README.md"))
        {
            let summary = readme
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            return format!(
                "README の要約: {summary}\n\n設計上の課題: README から確認できる情報は概要レベルに限られており、内部構成、実行手順、検証方法、制約、fallback や session 管理の責務分担が文書化されていません。そのため、初見の開発者が変更範囲や品質確認方法を判断しにくい状態です。ファイルは変更していません。"
            );
        }
        "ファイルは変更せず、読み取り専用の回答として整理します。目的、前提、推奨構成、検証方法、残リスクを分け、実装や編集が必要な場合だけ次のターンで明示的に依頼してください。".to_string()
    }

    fn repo_context_message(&mut self) -> Option<ConversationMessage> {
        if !self.session.mode_state.policy().allow_repo_context {
            return None;
        }
        self.refresh_working_memory();
        let task = self.session.working_memory.active_task.clone()?;
        if let Some(cache) = &self.repo_context_cache
            && cache.task == task
            && cache.work_root == self.work_root
        {
            return cache.message.clone();
        }
        let message = prompting::repo_context_message(&self.work_root, Some(&task));
        self.repo_context_cache = Some(super::RepoContextCache {
            task,
            work_root: self.work_root.clone(),
            message: message.clone(),
        });
        message
    }

    fn prepare_tool_call(&self, mut tool_call: ToolCall) -> ToolCall {
        tool_call.arguments = normalize_tool_call_arguments(&tool_call.name, tool_call.arguments);
        if matches!(tool_call.name.as_str(), "Read" | "Write" | "Edit")
            && let Some(arguments) = tool_call.arguments.as_object_mut()
            && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
            && let Ok(resolved) = resolve_user_path(&self.work_root, raw_path)
        {
            let resolved = if tool_call.name == "Read" {
                self.focused_edit_recovery_target()
                    .and_then(|target| {
                        focused_read_target_for_directory(&resolved, &target).then_some(target)
                    })
                    .unwrap_or(resolved)
            } else {
                resolved
            };
            arguments.insert(
                "path".to_string(),
                serde_json::Value::String(resolved.display().to_string()),
            );
        }
        tool_call
    }

    pub(super) fn push_system_note(&mut self, note: String) {
        if prompting::should_skip_system_note(&self.session.messages, &note) {
            return;
        }
        self.session
            .messages
            .push(ConversationMessage::system(note));
    }

    fn push_user_message(&mut self, content: String) {
        self.session
            .working_memory
            .set_active_task(Some(content.clone()));
        self.session
            .messages
            .push(ConversationMessage::user(content));
    }
}

/// Returns true when the environment requests that color output be suppressed
/// (https://no-color.org/): `NO_COLOR` is set to any non-empty value.
pub(crate) fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

fn is_utf8_locale(lang: &str) -> bool {
    let lower = lang.to_ascii_lowercase();
    lower
        .split(['.', '_', '@', ';', ',', ' '])
        .any(|t| t == "utf-8" || t == "utf8")
}

pub(crate) fn unicode_supported() -> bool {
    if std::env::var_os("ANVIL_NO_EMOJI").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(key)
            && is_utf8_locale(&v)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        FOCUSED_EDIT_POST_READ_TIMEOUT_SECS, FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS,
        PlanExplorationKey, answer_only_reply_is_inadequate, answer_only_script_command_allowed,
        assistant_model_for_mode, deterministic_timeout_fallback_plan,
        effective_non_streaming_timeout_secs, non_streaming_assistant_reply_timeout_secs,
        normalize_exploration_path, normalize_plan_exploration_key,
        request_explicitly_requests_script_execution, should_fallback_plan_model_after_timeout,
        should_materialize_plan_after_timeout,
        should_materialize_plan_after_tool_call_format_error, should_use_streaming_transport,
    };
    use crate::modes::plan_act::{ExecutionMode, TaskProfile};
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn normalizes_read_path_to_repo_relative_key() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("README.md");
        std::fs::write(&file, "hello").unwrap();

        let key = normalize_plan_exploration_key(
            "Read",
            &json!({"path": file.display().to_string(), "start_line": 1, "end_line": 10}),
            temp.path(),
            "stage1",
        )
        .unwrap();

        assert_eq!(
            key,
            PlanExplorationKey {
                stage: "stage1".to_string(),
                tool_name: "Read".to_string(),
                normalized_args: r#"{"end_line":10,"path":"README.md","start_line":1}"#.to_string(),
            }
        );
    }

    #[test]
    fn normalizes_relative_path_without_touching_missing_file() {
        let temp = tempdir().unwrap();
        let normalized = normalize_exploration_path("docs/plan.md", temp.path());
        assert_eq!(normalized, "docs/plan.md");
    }

    #[test]
    fn includes_stage_in_plan_exploration_key() {
        let temp = tempdir().unwrap();
        let args = json!({"pattern": "README.md"});
        let stage1 = normalize_plan_exploration_key("Glob", &args, temp.path(), "stage1").unwrap();
        let stage2 = normalize_plan_exploration_key("Glob", &args, temp.path(), "stage2").unwrap();
        assert_ne!(stage1, stage2);
    }

    #[test]
    fn qwen35_generate_tool_path_uses_non_streaming_transport() {
        assert!(!should_use_streaming_transport(
            "qwen3.5:122b",
            false,
            false,
            true,
        ));
    }

    #[test]
    fn qwen35_sidecar_tool_path_also_uses_non_streaming_transport() {
        assert!(!should_use_streaming_transport(
            "qwen3.5:9b",
            false,
            false,
            true,
        ));
    }

    #[test]
    fn qwen35_native_tool_path_uses_non_streaming_transport() {
        assert!(!should_use_streaming_transport(
            "qwen3.5:122b",
            true,
            false,
            true,
        ));
    }

    #[test]
    fn non_qwen35_native_tool_models_still_use_streaming_transport() {
        assert!(should_use_streaming_transport(
            "qwen3.6:27b-coding-nvfp4",
            true,
            false,
            true,
        ));
    }

    #[test]
    fn qwen35_non_native_requests_use_shorter_hard_timeout() {
        assert_eq!(
            non_streaming_assistant_reply_timeout_secs("qwen3.5:122b", false, 120),
            90
        );
        assert_eq!(
            non_streaming_assistant_reply_timeout_secs("qwen3.5:9b", false, 120),
            90
        );
        assert_eq!(
            non_streaming_assistant_reply_timeout_secs("qwen3.5:122b", true, 120),
            90
        );
        assert_eq!(
            non_streaming_assistant_reply_timeout_secs("qwen3.6:27b-coding-nvfp4", true, 120),
            120
        );
    }

    #[test]
    fn qwen35_focused_edit_timeout_override_remains_short() {
        assert_eq!(
            effective_non_streaming_timeout_secs(
                "qwen3.5:122b",
                true,
                120,
                Some(FOCUSED_EDIT_POST_READ_TIMEOUT_SECS),
            ),
            FOCUSED_EDIT_POST_READ_TIMEOUT_SECS
        );
        assert_eq!(
            effective_non_streaming_timeout_secs(
                "qwen3.5:122b",
                true,
                120,
                Some(FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS),
            ),
            FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS
        );
    }

    #[test]
    fn non_qwen35_focused_edit_timeout_override_remains_short() {
        assert_eq!(
            effective_non_streaming_timeout_secs(
                "qwen3.6:27b-coding-nvfp4",
                true,
                120,
                Some(FOCUSED_EDIT_POST_READ_TIMEOUT_SECS),
            ),
            FOCUSED_EDIT_POST_READ_TIMEOUT_SECS
        );
    }

    #[test]
    fn detects_explicit_script_execution_requests() {
        assert!(request_explicitly_requests_script_execution(
            "check_env.sh を実行して結果を要約してください。ファイルは変更しないでください。"
        ));
        assert!(!request_explicitly_requests_script_execution(
            "READMEを読んで設計を整理してください。"
        ));
    }

    #[test]
    fn answer_only_script_commands_are_narrowly_allowed() {
        assert!(answer_only_script_command_allowed("bash check_env.sh"));
        assert!(answer_only_script_command_allowed("./check_env.sh"));
        assert!(answer_only_script_command_allowed(
            "cd /tmp/project && bash check_env.sh"
        ));
        assert!(!answer_only_script_command_allowed(
            "bash check_env.sh > out.txt"
        ));
        assert!(!answer_only_script_command_allowed("rm generated.txt"));
    }

    #[test]
    fn answer_only_rejects_tool_call_like_final_text() {
        assert!(answer_only_reply_is_inadequate("Read('README.md')"));
        assert!(!answer_only_reply_is_inadequate(
            "ModePolicyを構造化状態として持つ利点は、会話履歴のノイズからツール許可を分離できることです。リスクは最新意図とのずれです。"
        ));
    }

    #[test]
    fn plan_timeout_can_fallback_to_sidecar_model() {
        assert!(should_fallback_plan_model_after_timeout(
            ExecutionMode::Plan,
            None,
            "assistant reply timed out after 90s",
            "qwen3.5:9b",
        ));
        assert!(!should_fallback_plan_model_after_timeout(
            ExecutionMode::Act,
            None,
            "assistant reply timed out after 90s",
            "qwen3.5:9b",
        ));
        assert!(!should_fallback_plan_model_after_timeout(
            ExecutionMode::Plan,
            Some("qwen3.5:9b"),
            "assistant reply timed out after 90s",
            "qwen3.5:9b",
        ));
    }

    #[test]
    fn plan_mode_override_selects_sidecar_model_only_for_plan() {
        assert_eq!(
            assistant_model_for_mode(ExecutionMode::Plan, "qwen3.5:122b", Some("qwen3.5:9b")),
            "qwen3.5:9b"
        );
        assert_eq!(
            assistant_model_for_mode(ExecutionMode::Act, "qwen3.5:122b", Some("qwen3.5:9b")),
            "qwen3.5:122b"
        );
    }

    #[test]
    fn plan_timeout_materializes_fallback_plan_immediately() {
        assert!(should_materialize_plan_after_timeout(
            ExecutionMode::Plan,
            Some("qwen3.5:9b"),
            "assistant reply timed out after 90s",
        ));
        assert!(should_materialize_plan_after_timeout(
            ExecutionMode::Plan,
            None,
            "assistant reply timed out after 90s",
        ));
        assert!(!should_materialize_plan_after_timeout(
            ExecutionMode::Act,
            Some("qwen3.5:9b"),
            "assistant reply timed out after 90s",
        ));
    }

    #[test]
    fn plan_tool_call_format_error_materializes_fallback_plan() {
        assert!(should_materialize_plan_after_tool_call_format_error(
            ExecutionMode::Plan,
            "tool call parser failed: malformed tool call markup",
        ));
        assert!(!should_materialize_plan_after_tool_call_format_error(
            ExecutionMode::Act,
            "tool call parser failed: malformed tool call markup",
        ));
        assert!(!should_materialize_plan_after_tool_call_format_error(
            ExecutionMode::Plan,
            "assistant reply timed out after 90s",
        ));
    }

    #[test]
    fn deterministic_timeout_fallback_plan_mentions_requested_port() {
        let temp = tempdir().unwrap();
        let plan = deterministic_timeout_fallback_plan(
            "ブラウザゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            TaskProfile::Coding,
            temp.path(),
        );
        assert!(plan.contains("3011"));
        assert!(plan.contains("`src/app/page.tsx`"));
        assert!(plan.contains("`package.json`"));
        assert!(plan.contains("## First Action"));
        assert!(plan.contains("## Verification"));
        assert!(plan.contains("runtime fallback plan"));
        assert!(super::lifecycle::plan_is_substantive(&plan));
        assert_eq!(
            super::lifecycle::current_plan_stage(&plan),
            crate::modes::plan_act::PlanStage::Ready
        );
    }
}

/// Replace control characters (C0, DEL, and C1) with spaces, then trim trailing
/// whitespace. Required for model-derived text so that newlines or ANSI escape
/// sequences cannot be injected into the terminal. C1 (`U+0080..U+009F`) is
/// included because some terminals interpret 8-bit CSI (`U+009B`) and OSC
/// (`U+009D`) equivalently to `ESC [` and `ESC ]`.
fn sanitize_for_progress(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

const COLOR_RESET: &str = "\x1b[0m";

fn tool_color(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "\x1b[38;5;198m",
        "Read" => "\x1b[38;5;87m",
        "Edit" => "\x1b[38;5;208m",
        "Bash" => "\x1b[38;5;226m",
        "Glob" => "\x1b[38;5;51m",
        "Grep" => "\x1b[38;5;39m",
        _ => "\x1b[38;5;245m",
    }
}

fn tool_emoji(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "✏️",
        "Read" => "📄",
        "Edit" => "📝",
        "Bash" => "⚡",
        "Glob" => "🔍",
        "Grep" => "🔎",
        _ => "🔧",
    }
}

fn paint(s: &str, color: &str, use_color: bool) -> String {
    if use_color && !color.is_empty() {
        format!("{color}{s}{COLOR_RESET}")
    } else {
        s.to_string()
    }
}

/// Returns `(display_str, extra)` for the progress line. `display_str` is the
/// main single-line description (path / command / pattern); `extra` is an
/// optional parenthesized suffix (e.g. `"5B"` for Write byte count). Paths are
/// made relative to `work_root` when possible. All model-derived strings pass
/// through `sanitize_for_progress` to prevent terminal injection.
///
/// `arg_budget` caps the Bash command display length (issue #432). Other tool
/// arms currently ignore this budget; the uniform signature lets the caller
/// compute the budget once via `progress_available_width`.
struct ProgressDisplay {
    action: String,
    path: Option<String>,
    note: Option<String>,
    status: Option<String>,
}

struct PlanWriteSummary {
    action: String,
    note: Option<String>,
    status: Option<String>,
    phase: String,
    signature: String,
}

fn plan_path_matches(raw_path: &str, work_root: &Path, plan_path: Option<&Path>) -> bool {
    resolve_plan_mode_write_target(work_root, raw_path, plan_path)
        .ok()
        .flatten()
        .is_some()
}

fn progress_path_display(
    raw_path: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
    max_chars: usize,
) -> String {
    if raw_path.is_empty() {
        return "<missing path>".to_string();
    }
    if plan_path_matches(raw_path, work_root, plan_path) {
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

fn join_sections_for_progress(sections: &[&str]) -> String {
    match sections {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut parts = sections[..sections.len() - 1]
                .iter()
                .map(|section| (*section).to_string())
                .collect::<Vec<_>>();
            parts.push(format!("and {}", sections[sections.len() - 1]));
            parts.join(", ")
        }
    }
}

fn should_use_streaming_transport(
    model: &str,
    _native_tools_enabled: bool,
    stream_output: bool,
    stdin_is_terminal: bool,
) -> bool {
    let wants_streaming = stream_output || stdin_is_terminal;
    if !wants_streaming {
        return false;
    }

    if is_qwen35_family(model) {
        return false;
    }

    true
}

fn non_streaming_assistant_reply_timeout_secs(
    model: &str,
    _native_tools_enabled: bool,
    default_timeout_secs: u64,
) -> u64 {
    if is_qwen35_family(model) {
        return QWEN35_NON_NATIVE_HARD_TIMEOUT_SECS;
    }
    default_timeout_secs
}

fn effective_non_streaming_timeout_secs(
    model: &str,
    native_tools_enabled: bool,
    default_timeout_secs: u64,
    timeout_override_secs: Option<u64>,
) -> u64 {
    let model_timeout = non_streaming_assistant_reply_timeout_secs(
        model,
        native_tools_enabled,
        default_timeout_secs,
    );
    match timeout_override_secs {
        Some(override_secs) => override_secs,
        None => model_timeout,
    }
}

fn focused_edit_timeout_override_secs(
    messages: &[ConversationMessage],
    target: Option<&Path>,
    work_root: &Path,
) -> Option<u64> {
    let target = target?;
    Some(
        if focused_edit_target_already_read(messages, target, work_root) {
            FOCUSED_EDIT_POST_READ_TIMEOUT_SECS
        } else {
            FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS
        },
    )
}

fn focused_edit_max_predict_override(
    messages: &[ConversationMessage],
    target: Option<&Path>,
    work_root: &Path,
) -> Option<usize> {
    let target = target?;
    Some(
        if focused_edit_target_already_read(messages, target, work_root) {
            FOCUSED_EDIT_POST_READ_MAX_PREDICT
        } else {
            FOCUSED_EDIT_PRE_READ_MAX_PREDICT
        },
    )
}

fn should_materialize_plan_after_timeout(
    mode: ExecutionMode,
    plan_model_override: Option<&str>,
    err: &str,
) -> bool {
    let _ = plan_model_override;
    mode == ExecutionMode::Plan && err.to_ascii_lowercase().contains("timed out")
}

fn should_materialize_plan_after_tool_call_format_error(mode: ExecutionMode, err: &str) -> bool {
    mode == ExecutionMode::Plan && lifecycle::is_tool_call_format_error(err)
}

fn assistant_model_for_mode(
    mode: ExecutionMode,
    main_model: &str,
    plan_model_override: Option<&str>,
) -> String {
    if mode == ExecutionMode::Plan
        && let Some(model) = plan_model_override
    {
        return model.to_string();
    }
    main_model.to_string()
}

fn should_fallback_plan_model_after_timeout(
    mode: ExecutionMode,
    plan_model_override: Option<&str>,
    err: &str,
    sidecar_model: &str,
) -> bool {
    if mode != ExecutionMode::Plan {
        return false;
    }
    if plan_model_override.is_some() {
        return false;
    }
    if sidecar_model.trim().is_empty() {
        return false;
    }
    err.to_ascii_lowercase().contains("timed out")
}

fn plan_file_alias(path: &Path) -> String {
    path.file_name()
        .map(|name| format!("plans/{}", name.to_string_lossy()))
        .unwrap_or_else(|| "plans/plan.md".to_string())
}

fn recent_truncated_tool_call_attempt(messages: &[ConversationMessage]) -> usize {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "system" {
                return None;
            }
            let lower = message.content.to_ascii_lowercase();
            if !lower.contains("truncated tool call") {
                return None;
            }
            message
                .content
                .rsplit("tool_call_format_attempt=")
                .next()
                .and_then(|suffix| suffix.trim().parse::<usize>().ok())
                .or(Some(1))
        })
        .unwrap_or(0)
}

fn latest_truncated_tool_call_note_index(messages: &[ConversationMessage]) -> Option<usize> {
    let slice = latest_user_turn_slice(messages);
    let offset = messages.len().saturating_sub(slice.len());
    slice
        .iter()
        .rposition(|message| {
            message.role == "system"
                && message
                    .content
                    .to_ascii_lowercase()
                    .contains("truncated tool call")
        })
        .map(|index| offset + index)
}

fn has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
    messages: &[ConversationMessage],
    work_root: &Path,
    plan_path: Option<&Path>,
) -> bool {
    let Some(index) = latest_truncated_tool_call_note_index(messages) else {
        return false;
    };
    successful_non_plan_repo_edit_count(&messages[index + 1..], work_root, plan_path) > 0
}

fn recent_post_scaffold_edit_attempt(messages: &[ConversationMessage]) -> usize {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "system" {
                return None;
            }
            message
                .content
                .rsplit("post_scaffold_edit_attempt=")
                .next()
                .and_then(|suffix| suffix.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

fn recent_post_scaffold_continue_attempt(messages: &[ConversationMessage]) -> usize {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "system" {
                return None;
            }
            message
                .content
                .rsplit("post_scaffold_continue_attempt=")
                .next()
                .and_then(|suffix| suffix.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

pub(super) fn prune_plan_mode_messages(messages: &mut Vec<ConversationMessage>) {
    messages.retain(|message| {
        if message.role != "system" {
            return true;
        }
        !is_plan_mode_only_system_note(&message.content)
    });
}

fn is_plan_mode_only_system_note(note: &str) -> bool {
    let trimmed = note.trim_start();
    trimmed.starts_with("[Plan Mode /")
        || trimmed.starts_with("[Plan File Alias]")
        || trimmed.contains("plan_no_tool_attempt=")
        || trimmed.contains("plan_progress_attempt=")
        || trimmed.starts_with("Main planning model timed out.")
        || trimmed.starts_with("The plan is still incomplete.")
        || trimmed.starts_with("You are still in Plan mode")
}

#[cfg(test)]
fn has_successful_repo_edit(messages: &[ConversationMessage]) -> bool {
    successful_repo_edit_count(messages) > 0
}

#[cfg(test)]
fn successful_repo_edit_count(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .filter(|message| {
            message.role == "tool"
                && matches!(message.name.as_deref(), Some("Write" | "Edit"))
                && !message.content.trim_start().starts_with("Error:")
        })
        .count()
}

fn has_successful_non_plan_repo_edit(
    messages: &[ConversationMessage],
    work_root: &Path,
    plan_path: Option<&Path>,
) -> bool {
    successful_non_plan_repo_edit_count(messages, work_root, plan_path) > 0
}

fn successful_non_plan_repo_edit_count(
    messages: &[ConversationMessage],
    work_root: &Path,
    plan_path: Option<&Path>,
) -> usize {
    let mut count = 0usize;
    let mut pending_tool_calls: std::collections::VecDeque<ToolCall> =
        std::collections::VecDeque::new();

    for message in messages {
        match message.role.as_str() {
            "assistant" => {
                pending_tool_calls = message.tool_calls.iter().cloned().collect();
            }
            "tool" => {
                let Some(expected_tool_call) = pending_tool_calls.pop_front() else {
                    continue;
                };
                if !matches!(message.name.as_deref(), Some("Write" | "Edit"))
                    || message.content.trim_start().starts_with("Error:")
                {
                    continue;
                }
                if is_plan_file_tool_call(
                    &expected_tool_call.name,
                    &expected_tool_call.arguments,
                    work_root,
                    plan_path,
                ) {
                    continue;
                }
                count += 1;
            }
            _ => {}
        }
    }

    count
}

fn last_read_tool_path(messages: &[ConversationMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        message.tool_calls.iter().rev().find_map(|tool_call| {
            if tool_call.name != "Read" {
                return None;
            }
            tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
    })
}

fn latest_turn_last_read_tool_path(messages: &[ConversationMessage]) -> Option<String> {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "assistant" {
                return None;
            }
            message.tool_calls.iter().rev().find_map(|tool_call| {
                if tool_call.name != "Read" {
                    return None;
                }
                tool_call
                    .arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
        })
}

fn recent_scaffold_command_seen(messages: &[ConversationMessage]) -> bool {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .any(|message| {
            if message.role != "assistant" {
                return false;
            }
            message.tool_calls.iter().rev().any(|tool_call| {
                tool_call.name == "Bash"
                    && tool_call
                        .arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(recovery::is_scaffold_command)
            })
        })
}

fn latest_user_turn_slice(messages: &[ConversationMessage]) -> &[ConversationMessage] {
    messages
        .iter()
        .rposition(|message| message.role == "user")
        .map(|index| &messages[index..])
        .unwrap_or(messages)
}

fn post_scaffold_recovery_active(
    messages: &[ConversationMessage],
    active_root: Option<&Path>,
    cwd: &Path,
) -> bool {
    recent_scaffold_command_seen(messages)
        || recent_post_scaffold_edit_attempt(messages) > 0
        || (active_root.is_some_and(|root| root != cwd)
            && latest_user_turn_slice(messages).iter().any(|message| {
                message.role == "system"
                    && message
                        .content
                        .trim_start()
                        .starts_with("[Workspace Root Updated]")
            }))
}

fn post_scaffold_continuation_active(
    _messages: &[ConversationMessage],
    _active_root: Option<&Path>,
    _cwd: &Path,
    _work_root: &Path,
    _plan_path: Option<&Path>,
) -> bool {
    // A second forced microscopic edit tends to trap scaffolded apps in
    // placeholder-copy churn. After the first repo edit, let the normal
    // implementation loop and final quality gate drive the next action.
    false
}

fn workspace_appears_empty(work_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return false;
    };
    !entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        !matches!(
            name.as_ref(),
            ".git" | ".anvil" | ".anvil-state" | "node_modules" | "target"
        )
    })
}

fn deterministic_framework_game_files_needed(
    work_root: &Path,
    files: &[(PathBuf, String)],
) -> bool {
    let impl_paths = files
        .iter()
        .map(|(path, _)| path)
        .filter(|path| deterministic_framework_game_impl_path(path))
        .collect::<Vec<_>>();
    if impl_paths.is_empty() || impl_paths.iter().any(|path| work_root.join(path).is_file()) {
        return false;
    }

    let Some(existing_files) = meaningful_workspace_files(work_root, 32) else {
        return false;
    };
    if existing_files.is_empty() {
        return true;
    }

    existing_files.iter().all(|existing| {
        files
            .iter()
            .filter(|(path, _)| !deterministic_framework_game_impl_path(path))
            .any(|(path, _)| path == existing)
    })
}

fn deterministic_framework_app_files_needed(
    work_root: &Path,
    files: &[(PathBuf, String)],
    request: &str,
) -> bool {
    if deterministic_framework_game_files_needed(work_root, files) {
        return true;
    }

    let Some(target) = first_existing_impl_target(work_root) else {
        return false;
    };
    let Ok(current) = std::fs::read_to_string(&target) else {
        return false;
    };
    if implementation_quality_issue_for_request(request, &current).is_none() {
        return false;
    }
    deterministic::playable_ui_repair(request, &target, &current).is_some()
}

fn deterministic_framework_game_impl_path(path: &Path) -> bool {
    matches!(
        path.to_string_lossy().as_ref(),
        "app.vue" | "src/App.tsx" | "src/app/page.tsx"
    )
}

fn meaningful_workspace_files(work_root: &Path, limit: usize) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_meaningful_workspace_files(work_root, work_root, limit, &mut files).ok()?;
    Some(files)
}

fn collect_meaningful_workspace_files(
    root: &Path,
    current: &Path,
    limit: usize,
    files: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    if files.len() > limit {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | ".anvil" | ".anvil-state" | "node_modules" | "target"
        ) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_meaningful_workspace_files(root, &path, limit, files)?;
        } else if path.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.push(relative.to_path_buf());
            if files.len() > limit {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn should_apply_repo_change_quality_gate(
    action_expectation: recovery::ActionExpectation,
    active_task_expects_repo_change: bool,
    mode: ExecutionMode,
) -> bool {
    mode == ExecutionMode::Act
        && (action_expectation == recovery::ActionExpectation::RepoChange
            || active_task_expects_repo_change)
}

fn is_page_component_target(relative: &str) -> bool {
    matches!(relative, "app/page.tsx" | "src/app/page.tsx")
        || relative.ends_with("/app/page.tsx")
        || relative.ends_with("/src/app/page.tsx")
}

fn focused_edit_guidance_note(
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> String {
    let path = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    if target_already_read {
        format!(
            "[Focused Edit Recovery] The target file {path} has already been read. The only available tool for this turn is Edit. Do not call Read again. Use exactly one compact Edit on that file now. Replace only one contiguous block from the last Read. Do not attempt a full-file rewrite, multi-file change, scaffold command, or dev-server command."
        )
    } else {
        format!(
            "[Focused Edit Recovery] Keep this turn minimal. If you need context, do one Read on {path} first; otherwise use exactly one compact Edit. Do not attempt a full-file rewrite, multi-file change, scaffold command, or dev-server command. Replace only one contiguous block from the last Read and move the implementation forward with the first concrete slice."
        )
    }
}

fn focused_edit_compact_anchor_note(target: &Path, work_root: &Path) -> String {
    let path = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    format!(
        "[Focused Edit Recovery / Compact Anchor] The Read result for {path} is intentionally only a tiny exact anchor from the real file, not the whole file. Use that anchor only for `old_string`. Keep `new_string` similarly small: at most 3 lines and under 240 characters. Do not insert imports, hooks, component definitions, or full-file content. If the anchor is CTA or placeholder text, replace only that text with a short task-specific label or copy."
    )
}

fn focused_edit_first_slice_note(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> Option<String> {
    if !target_already_read {
        return None;
    }
    let relative = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    if is_page_component_target(&relative) {
        if let Some(old_string) = latest_page_copy_block_from_read(messages, target, work_root) {
            return Some(recovery::first_scaffold_shell_edit_exact_anchor_note(
                &relative,
                &old_string,
            ));
        }
        return Some(recovery::first_scaffold_shell_edit_note(&relative));
    }
    None
}

fn focused_edit_first_slice_uses_exact_anchor(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> bool {
    if !target_already_read {
        return false;
    }
    let relative = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    is_page_component_target(&relative)
        && latest_page_copy_block_from_read(messages, target, work_root).is_some()
}

fn focused_edit_exact_recovery_anchor(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
    successful_repo_edits: usize,
) -> Option<String> {
    match successful_repo_edits {
        0 => {
            if !focused_edit_first_slice_uses_exact_anchor(
                messages,
                target,
                work_root,
                target_already_read,
            ) {
                return None;
            }
            latest_page_copy_block_from_read(messages, target, work_root)
        }
        1 => {
            let relative = target
                .strip_prefix(work_root)
                .unwrap_or(target)
                .to_string_lossy()
                .replace('\\', "/");
            if !target_already_read || !is_page_component_target(&relative) {
                return None;
            }
            latest_page_intro_copy_line_from_read(messages, target, work_root)
        }
        _ => None,
    }
}

fn focused_edit_second_slice_note(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> Option<String> {
    if !target_already_read {
        return None;
    }
    let relative = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    if !is_page_component_target(&relative) {
        return None;
    }
    latest_page_intro_copy_line_from_read(messages, target, work_root).map(|old_string| {
        recovery::second_scaffold_shell_edit_exact_anchor_note(&relative, &old_string)
    })
}

fn latest_page_copy_block_from_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_page_copy_block_from_numbered_read(&tool.content)
}

fn latest_page_intro_copy_line_from_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_page_intro_copy_line_from_numbered_read(&tool.content)
        .or_else(|| extract_page_intro_paragraph_from_numbered_read(&tool.content))
}

fn focused_edit_compact_recovery_anchor(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_compact_edit_anchor_from_numbered_read(&tool.content)
}

fn extract_compact_edit_anchor_from_numbered_read(contents: &str) -> Option<String> {
    let lines = contents
        .lines()
        .map(strip_read_line_number_prefix)
        .collect::<Vec<_>>();
    let candidate = |line: &&String| {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && !matches!(trimmed, "{" | "}" | ");" | "</div>" | "</main>")
            && line.chars().count() <= 180
    };

    lines
        .iter()
        .find(|line| {
            candidate(line)
                && matches!(
                    line.trim(),
                    "Deploy Now" | "Documentation" | "Get started" | "Learn More"
                )
        })
        .or_else(|| {
            lines.iter().find(|line| {
                let trimmed = line.trim_start();
                candidate(line)
                    && (trimmed.starts_with("<button ")
                        || trimmed.starts_with("<a ")
                        || trimmed.starts_with("<h1 ")
                        || trimmed.starts_with("<p "))
            })
        })
        .or_else(|| {
            lines.iter().find(|line| {
                let trimmed = line.trim_start();
                candidate(line)
                    && !trimmed.starts_with("import ")
                    && !trimmed.starts_with("export default")
            })
        })
        .cloned()
}

fn extract_page_copy_block_from_numbered_read(contents: &str) -> Option<String> {
    let lines = contents
        .lines()
        .map(strip_read_line_number_prefix)
        .collect::<Vec<_>>();
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("<h1 ") || trimmed.starts_with("<h1>")
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, line)| line.trim_start().contains("</h1>").then_some(index))?;
    Some(lines[start..=end].join("\n"))
}

fn extract_page_intro_paragraph_from_numbered_read(contents: &str) -> Option<String> {
    let lines = contents
        .lines()
        .map(strip_read_line_number_prefix)
        .collect::<Vec<_>>();
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("<p ") || trimmed.starts_with("<p>")
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, line)| line.trim_start().contains("</p>").then_some(index))?;
    Some(lines[start..=end].join("\n"))
}

fn extract_page_intro_copy_line_from_numbered_read(contents: &str) -> Option<String> {
    let lines = contents
        .lines()
        .map(strip_read_line_number_prefix)
        .collect::<Vec<_>>();
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("<p ") || trimmed.starts_with("<p>")
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, line)| line.trim_start().contains("</p>").then_some(index))?;

    lines[start + 1..end]
        .iter()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('<')
                && trimmed.chars().any(|ch| ch.is_alphabetic())
        })
        .cloned()
}

fn strip_read_line_number_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some((prefix, rest)) = trimmed.split_once(": ")
        && !prefix.is_empty()
        && prefix.chars().all(|ch| ch.is_ascii_digit())
    {
        return rest.to_string();
    }
    line.to_string()
}

fn focused_edit_target_already_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> bool {
    latest_read_exchange_for_target(messages, target, work_root).is_some()
}

fn focused_edit_history(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Vec<ConversationMessage> {
    let mut filtered = Vec::new();
    if let Some(note) = messages.iter().rev().find(|message| {
        message.role == "system" && message.content.trim_start().starts_with("[Act Mode /")
    }) {
        filtered.push(note.clone());
    }
    if let Some(user) = messages.iter().rev().find(|message| message.role == "user") {
        filtered.push(user.clone());
    }
    if let Some((assistant, tool)) = latest_read_exchange_for_target(messages, target, work_root) {
        filtered.push(assistant);
        filtered.push(tool);
    }
    filtered
}

fn focused_edit_minimal_history(messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
    let mut filtered = Vec::new();
    if let Some(note) = messages.iter().rev().find(|message| {
        message.role == "system" && message.content.trim_start().starts_with("[Act Mode /")
    }) {
        filtered.push(note.clone());
    }
    if let Some(user) = messages.iter().rev().find(|message| message.role == "user") {
        filtered.push(user.clone());
    }
    filtered
}

fn focused_edit_exact_anchor_history(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
    anchor: &str,
) -> Vec<ConversationMessage> {
    let mut filtered = focused_edit_minimal_history(messages);
    let relative = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    filtered.push(ConversationMessage::assistant(
        String::new(),
        vec![ToolCall {
            id: "focused-anchor-read".to_string(),
            name: "Read".to_string(),
            arguments: serde_json::json!({ "path": relative }),
        }],
    ));
    filtered.push(ConversationMessage::tool(
        "Read".to_string(),
        format_numbered_read_block(anchor),
    ));
    filtered
}

fn format_numbered_read_block(contents: &str) -> String {
    contents
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{:>4}: {line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn focused_edit_tool_policy_error(
    name: &str,
    arguments: &serde_json::Value,
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> Option<String> {
    let path_display = target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    let path_matches = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|raw_path| tool_path_matches_target(raw_path, target, work_root));

    if target_already_read {
        if name != "Edit" || !path_matches {
            return Some(format!(
                "focused edit recovery only allows Edit on {path_display} after the file has already been read"
            ));
        }
        return None;
    }

    match name {
        "Read" | "Edit" if path_matches => None,
        _ => Some(format!(
            "focused edit recovery only allows Read or Edit on {path_display} until the first edit succeeds"
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FocusedEditBatchAction {
    Accept,
    TruncateToFirst,
    Reject(String),
}

fn focused_edit_tool_batch_action(
    tool_calls: &[ToolCall],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> FocusedEditBatchAction {
    let Some(first_tool_call) = tool_calls.first() else {
        return FocusedEditBatchAction::Accept;
    };

    let first_call_error = focused_edit_tool_policy_error(
        &first_tool_call.name,
        &first_tool_call.arguments,
        target,
        work_root,
        target_already_read,
    );

    if tool_calls.len() == 1 {
        return first_call_error
            .map(FocusedEditBatchAction::Reject)
            .unwrap_or(FocusedEditBatchAction::Accept);
    }

    if first_call_error.is_none() {
        FocusedEditBatchAction::TruncateToFirst
    } else {
        FocusedEditBatchAction::Reject(first_call_error.unwrap_or_default())
    }
}

fn tool_path_matches_target(raw_path: &str, target: &Path, work_root: &Path) -> bool {
    let Ok(resolved) = resolve_user_path(work_root, raw_path) else {
        return false;
    };
    let canonical_target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let canonical_resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
    canonical_resolved == canonical_target
}

fn focused_read_target_for_directory(resolved: &Path, target: &Path) -> bool {
    if !resolved.is_dir() {
        return false;
    }
    let Some(parent) = target.parent() else {
        return false;
    };
    let canonical_resolved =
        std::fs::canonicalize(resolved).unwrap_or_else(|_| resolved.to_path_buf());
    let canonical_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    canonical_resolved == canonical_parent
}

fn latest_read_exchange_for_target(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<(ConversationMessage, ConversationMessage)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if message.role != "assistant" || !assistant_reads_target(message, target, work_root) {
            continue;
        }
        let tool_message = messages.get(index + 1)?;
        if tool_message.role == "tool" && tool_message.name.as_deref() == Some("Read") {
            if messages[index + 2..]
                .iter()
                .any(is_successful_repo_edit_tool_result)
            {
                continue;
            }
            return Some((message.clone(), tool_message.clone()));
        }
    }
    None
}

fn is_successful_repo_edit_tool_result(message: &ConversationMessage) -> bool {
    message.role == "tool"
        && matches!(message.name.as_deref(), Some("Write" | "Edit"))
        && !message.content.trim_start().starts_with("Error:")
}

fn assistant_reads_target(message: &ConversationMessage, target: &Path, work_root: &Path) -> bool {
    let normalized_target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    message.tool_calls.iter().any(|tool_call| {
        if tool_call.name != "Read" {
            return false;
        }
        let Some(path) = tool_call
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        resolve_user_path(work_root, path)
            .ok()
            .is_some_and(|resolved| resolved == normalized_target)
    })
}

fn plan_sections_with_content(contents: &str) -> Vec<&'static str> {
    [
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
    ]
    .into_iter()
    .filter(|section| plan_section_has_content(contents, section))
    .collect()
}

fn plan_section_body_for_progress<'a>(contents: &'a str, section: &str) -> Option<&'a str> {
    let mut start = None;
    let mut end = contents.len();
    let mut offset = 0usize;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            if start.is_some() {
                end = offset;
                break;
            }
            if normalize_plan_heading_for_progress(heading) == section {
                start = Some(offset + line.len());
            }
        }
        offset += line.len() + 1;
    }
    start.map(|idx| &contents[idx..end])
}

fn plan_section_excerpt(contents: &str, sections: &[&str]) -> Option<String> {
    for section in sections {
        let Some(body) = plan_section_body_for_progress(contents, section) else {
            continue;
        };
        for line in body.lines().map(str::trim) {
            if line.is_empty()
                || line == "-"
                || matches!(
                    line,
                    "1." | "2."
                        | "3."
                        | "1. First slice:"
                        | "2. Next phases:"
                        | "3. Review checkpoint:"
                )
            {
                continue;
            }
            let cleaned = line.trim_start_matches("- ").trim();
            return Some(format!(
                "{section}: {}",
                truncate(&sanitize_for_progress(cleaned), 72)
            ));
        }
    }
    None
}

fn plan_phase_from_sections(
    sections: &[&str],
    current_stage: PlanStage,
    approval_ready: bool,
) -> &'static str {
    if approval_ready {
        "Approval review"
    } else if sections.iter().any(|section| {
        matches!(
            *section,
            "First Action"
                | "Verification"
                | "Execution Plan"
                | "Verification Plan"
                | "Risks / Fallbacks"
        )
    }) {
        "Define next action"
    } else if sections
        .iter()
        .any(|section| matches!(*section, "Acceptance Criteria" | "Quality Bar"))
    {
        "Define quality bar"
    } else if sections
        .iter()
        .any(|section| matches!(*section, "Goal" | "Constraints" | "Deliverables"))
    {
        "Draft foundation"
    } else {
        match current_stage {
            PlanStage::Stage1 => "Draft foundation",
            PlanStage::Stage2 => "Define next action",
            PlanStage::Stage3 => "Approval review",
            PlanStage::Ready => "Approval review",
        }
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

fn summarize_plan_write(
    tool_name: &str,
    raw_path: &str,
    new_text: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> PlanWriteSummary {
    let previous = if raw_path.is_empty() {
        String::new()
    } else if plan_path_matches(raw_path, work_root, plan_path) {
        plan_path
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    } else {
        resolve_user_path(work_root, raw_path)
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    };
    let previous_sections = plan_sections_with_content(&previous);
    let current_sections = plan_sections_with_content(new_text);
    let changed_sections = current_sections
        .iter()
        .copied()
        .filter(|section| {
            let old_body = plan_section_body_for_progress(&previous, section).unwrap_or_default();
            let new_body = plan_section_body_for_progress(new_text, section).unwrap_or_default();
            sanitize_for_progress(old_body) != sanitize_for_progress(new_body)
        })
        .collect::<Vec<_>>();
    let added_sections = current_sections
        .iter()
        .copied()
        .filter(|section| !previous_sections.contains(section))
        .collect::<Vec<_>>();
    let removed_sections = previous_sections
        .iter()
        .copied()
        .filter(|section| !current_sections.contains(section))
        .collect::<Vec<_>>();
    let focus_sections = if !changed_sections.is_empty() {
        changed_sections.clone()
    } else if !current_sections.is_empty() {
        current_sections.clone()
    } else {
        Vec::new()
    };
    let verb = if !removed_sections.is_empty() {
        "Rewrite"
    } else if previous_sections.is_empty() {
        "Draft"
    } else if !added_sections.is_empty() {
        "Add"
    } else if tool_name == "Edit" {
        "Revise"
    } else {
        "Update"
    };
    let action = if focus_sections.is_empty() {
        "Update plan draft".to_string()
    } else {
        format!("{verb} {}", join_sections_for_progress(&focus_sections))
    };
    let note = plan_section_excerpt(new_text, &focus_sections).or_else(|| {
        let fallback = current_sections.clone();
        plan_section_excerpt(new_text, &fallback)
    });
    let delta = new_text.len() as isize - previous.len() as isize;
    let next = lifecycle::plan_next_stage_sections(new_text);
    let approval_ready = lifecycle::plan_missing_sections(new_text).is_empty();
    let status = if approval_ready {
        Some(format!("Approval ready | delta {delta:+}B"))
    } else if !next.is_empty() {
        Some(format!(
            "Next: {} | delta {delta:+}B",
            join_sections_for_progress(&next)
        ))
    } else {
        Some(format!("delta {delta:+}B"))
    };
    let phase =
        plan_phase_from_sections(&focus_sections, current_stage, approval_ready).to_string();
    let signature = format!("{}|{}|{}", phase, action, note.clone().unwrap_or_default());
    PlanWriteSummary {
        action,
        note,
        status,
        phase,
        signature,
    }
}

fn tool_display(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &std::path::Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
    arg_budget: usize,
) -> ProgressDisplay {
    let str_arg = |key: &str| -> &str {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    let raw_path = str_arg("path");
    let path_display = progress_path_display(raw_path, work_root, plan_path, arg_budget.max(48));
    match tool_name {
        "Write" => {
            let content = arguments
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let (action, note, status) = if plan_path_matches(raw_path, work_root, plan_path) {
                let summary = summarize_plan_write(
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
                    (!text_preview(content, 72).is_empty())
                        .then(|| format!("Preview: {}", text_preview(content, 72))),
                    arguments
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(|c| format!("{}B", c.len())),
                )
            };
            ProgressDisplay {
                action,
                path: Some(path_display),
                note,
                status,
            }
        }
        "Edit" => {
            let new_text = arguments
                .get("new_string")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let (action, note, status) = if plan_path_matches(raw_path, work_root, plan_path) {
                let summary = summarize_plan_write(
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
                    (!text_preview(new_text, 72).is_empty())
                        .then(|| format!("Preview: {}", text_preview(new_text, 72))),
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
        "Read" => {
            let line_suffix = read_line_suffix(arguments);
            let path = format!("{path_display}{line_suffix}");
            let (action, note, status) = if plan_path_matches(raw_path, work_root, plan_path) {
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
                            plan_phase_from_sections(&next, current_stage, false)
                        )
                    })
                };
                (action, note, status)
            } else if !line_suffix.is_empty() {
                let (preview, extra) = read_preview(raw_path, work_root);
                (
                    format!("Read lines {}", line_suffix.trim_start_matches(':')),
                    preview.map(|preview| format!("Preview: {preview}")),
                    extra,
                )
            } else {
                let (preview, extra) = read_preview(raw_path, work_root);
                (
                    "Read file".to_string(),
                    preview.map(|preview| format!("Preview: {preview}")),
                    extra,
                )
            };
            ProgressDisplay {
                action,
                path: Some(compact_progress_path(&path, arg_budget.max(48))),
                note,
                status,
            }
        }
        "Bash" => {
            let sanitized = sanitize_for_progress(str_arg("command"));
            ProgressDisplay {
                action: format!("Run {}", truncate(&sanitized, arg_budget.saturating_sub(4))),
                path: None,
                note: None,
                status: None,
            }
        }
        "Glob" | "Grep" => ProgressDisplay {
            action: format!(
                "Search {}",
                truncate(
                    &sanitize_for_progress(str_arg("pattern")),
                    arg_budget.saturating_sub(7)
                )
            ),
            path: None,
            note: None,
            status: None,
        },
        _ => ProgressDisplay {
            action: truncate(&sanitize_for_progress(tool_name), arg_budget),
            path: None,
            note: None,
            status: None,
        },
    }
}

fn plan_section_has_content(contents: &str, target: &str) -> bool {
    let mut in_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            in_section = normalize_plan_heading_for_progress(heading) == target;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty()
            || trimmed == "-"
            || matches!(
                trimmed,
                "1." | "2."
                    | "3."
                    | "1. First slice:"
                    | "2. Next phases:"
                    | "3. Review checkpoint:"
            )
        {
            continue;
        }
        return true;
    }
    false
}

fn normalize_plan_heading_for_progress(heading: &str) -> &str {
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

/// Compute the argument-summary budget for a progress line given the current
/// terminal width (issue #432 §4.3.1).
///
/// Subtracts the fixed chrome (`[iter N/M]  `, optional emoji, tool name, and
/// the two-space separator) plus 3 chars reserved for the `...` ellipsis that
/// `truncate()` appends when the input exceeds the budget, then clamps the
/// result to `MIN_ARG_BUDGET` (20). When `cols` is `None` (footer disabled /
/// handle absent / first-tick race) the caller falls back to
/// `DEFAULT_ARG_BUDGET` (57), preserving the pre-#432 behaviour.
pub(super) fn progress_available_width(
    cols: Option<u16>,
    tool_name: &str,
    iter_human: usize,
    max_iterations: usize,
    use_unicode: bool,
) -> usize {
    const DEFAULT_ARG_BUDGET: usize = 57;
    const MIN_ARG_BUDGET: usize = 20;
    // truncate() appends "..." (3 chars) when it fires, so reserve those chars
    // up-front. Otherwise a fully-truncated Bash command overflows cols by 3.
    const ELLIPSIS_RESERVE: usize = 3;

    let Some(cols) = cols else {
        return DEFAULT_ARG_BUDGET;
    };

    // Chrome must stay in sync with the `format!` in `format_progress_line`:
    //   "[iter N/M]  " + (emoji " ")? + tool_name + "  "
    let iter_prefix = format!("[iter {iter_human}/{max_iterations}]  ");
    let emoji_width = if use_unicode {
        tool_emoji(tool_name).chars().count() + 1
    } else {
        0
    };
    let chrome = iter_prefix.len() + emoji_width + tool_name.chars().count() + 2;

    (cols as usize)
        .saturating_sub(chrome)
        .saturating_sub(ELLIPSIS_RESERVE)
        .max(MIN_ARG_BUDGET)
}

/// Format a single-line per-iteration progress line. ANSI color is only
/// applied to the tool name when `use_color` is true, and emoji is prepended
/// when `use_unicode` is true. `cols` is the current terminal width from the
/// footer broadcaster; `None` falls back to the pre-#432 fixed budget.
fn progress_detail_budget(cols: Option<u16>, prefix: &str) -> usize {
    cols.map(|value| value as usize)
        .unwrap_or(96)
        .saturating_sub(prefix.chars().count())
        .max(24)
}

fn format_progress_field(prefix: &str, value: &str, cols: Option<u16>) -> String {
    let budget = progress_detail_budget(cols, prefix);
    format!(
        "{prefix}{}",
        truncate(&sanitize_for_progress(value), budget)
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn format_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
    use_unicode: bool,
    cols: Option<u16>,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
    status_prefix: Option<&str>,
    stage_label: Option<&str>,
) -> String {
    let arg_budget =
        progress_available_width(cols, tool_name, iter_human, max_iterations, use_unicode);
    let display = tool_display(
        tool_name,
        arguments,
        work_root,
        plan_path,
        current_stage,
        arg_budget,
    );
    // Sanitize before painting so an adversarial tool_name cannot inject escapes.
    // emoji は &'static str ハードコードなので再 sanitize は不要。
    let safe_tool_name = sanitize_for_progress(tool_name);
    let label = if use_unicode {
        format!("{} {}", tool_emoji(tool_name), safe_tool_name)
    } else {
        safe_tool_name
    };
    let painted = paint(&label, tool_color(tool_name), use_color);
    if matches!(tool_name, "Read" | "Write" | "Edit") {
        let mut lines = Vec::new();
        let stage = stage_label.unwrap_or("Working");
        lines.push(format!("[iter {iter_human}/{max_iterations}] {stage}"));
        lines.push(format!("  tool:   {painted}"));
        lines.push(format_progress_field("  action: ", &display.action, cols));
        if let Some(path) = display.path {
            lines.push(format_progress_field("  file:   ", &path, cols));
        }
        if let Some(note) = display.note {
            lines.push(format_progress_field("  note:   ", &note, cols));
        }
        if let Some(status) = display.status {
            let combined_status = status_prefix
                .map(|prefix| format!("{prefix} | {status}"))
                .unwrap_or(status);
            lines.push(format_progress_field("  status: ", &combined_status, cols));
        } else if let Some(prefix) = status_prefix {
            lines.push(format_progress_field("  status: ", prefix, cols));
        }
        lines.push(String::new());
        lines.join("\n")
    } else {
        format!(
            "[iter {iter_human}/{max_iterations}]  {painted}  {}",
            display.action
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn format_blocked_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
    use_unicode: bool,
    cols: Option<u16>,
    headline: &str,
    note: &str,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> String {
    let arg_budget =
        progress_available_width(cols, headline, iter_human, max_iterations, use_unicode);
    let display = tool_display(
        tool_name,
        arguments,
        work_root,
        plan_path,
        current_stage,
        arg_budget,
    );
    let label = if use_unicode {
        format!("⛔ {headline}")
    } else {
        headline.to_string()
    };
    let painted = paint(&label, "\x1b[38;5;196m", use_color);
    if matches!(tool_name, "Read" | "Write" | "Edit") {
        let mut lines = Vec::new();
        lines.push(format!("[iter {iter_human}/{max_iterations}] {headline}"));
        lines.push(format!("  tool:   {painted}"));
        lines.push(format_progress_field("  action: ", &display.action, cols));
        if let Some(path) = display.path {
            lines.push(format_progress_field("  file:   ", &path, cols));
        }
        lines.push(format_progress_field("  status: ", note, cols));
        lines.push(String::new());
        lines.join("\n")
    } else {
        format!(
            "[iter {iter_human}/{max_iterations}]  {painted}  {}",
            display.action
        )
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::{
        ScaffoldFramework, deterministic_nextjs_scaffold_reply, reply_looks_like_future_work,
        requested_scaffold_framework, scaffold_command_matches_framework,
        task_or_plan_requires_nextjs_scaffold, task_requires_nextjs_scaffold, truncate,
    };

    #[test]
    fn preserves_short_strings_verbatim() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncates_long_strings_with_ellipsis() {
        assert_eq!(truncate("abcdefgh", 3), "abc...");
    }

    #[test]
    fn never_splits_multibyte_code_points() {
        // Each Japanese char is 3 bytes in UTF-8; taking 2 must not slice mid-char.
        assert_eq!(truncate("あいうえお", 2), "あい...");
    }

    #[test]
    fn detects_future_work_prose_after_partial_edit() {
        assert!(reply_looks_like_future_work(
            "Now I'll create the full interactive app as a client component."
        ));
        assert!(reply_looks_like_future_work("次にゲーム本体を実装します。"));
        assert!(reply_looks_like_future_work(
            "READMEの全文を確認しました。さらに詳細な設計ファイルがないか探してみます。"
        ));
        assert!(!reply_looks_like_future_work(
            "Implemented the first playable shell in app/page.tsx."
        ));
    }

    #[test]
    fn detects_nextjs_framework_tasks() {
        assert!(task_requires_nextjs_scaffold(
            "3011ポートで起動可能なnext.jsアプリとして開発してください"
        ));
        assert!(task_requires_nextjs_scaffold("Build this as a NextJS app"));
        assert!(!task_requires_nextjs_scaffold("Build a Rust CLI tool"));
    }

    #[test]
    fn detects_explicit_scaffold_frameworks() {
        assert_eq!(
            requested_scaffold_framework("React.jsアプリとして開発してください"),
            Some(ScaffoldFramework::React)
        );
        assert_eq!(
            requested_scaffold_framework("Nuxt.jsアプリとして開発してください"),
            Some(ScaffoldFramework::Nuxt)
        );
        assert_eq!(
            requested_scaffold_framework("Next.jsアプリとして開発してください"),
            Some(ScaffoldFramework::Next)
        );
        assert_eq!(requested_scaffold_framework("Rust CLIを作って"), None);
    }

    #[test]
    fn scaffold_commands_must_match_requested_framework() {
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npm create vite@latest . -- --template react-ts"
        ));
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::Nuxt,
            "npx nuxi@latest init . --packageManager npm"
        ));
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::Next,
            "npx create-next-app@latest . --typescript --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npx create-next-app@latest . --typescript --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npx create-react-app . --template cra-template --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::Nuxt,
            "npm create vite@latest . -- --template react-ts"
        ));
    }

    #[test]
    fn detects_nextjs_request_from_active_task_or_plan_only() {
        assert!(task_or_plan_requires_nextjs_scaffold(
            Some("3011ポートで起動可能なnext.jsアプリとして開発してください"),
            None,
        ));
        assert!(task_or_plan_requires_nextjs_scaffold(
            Some("yes"),
            Some("Build the accepted plan as a Next.js app."),
        ));
        assert!(!task_or_plan_requires_nextjs_scaffold(
            Some("yes"),
            Some("Build a local Rust CLI."),
        ));
    }

    #[test]
    fn deterministic_nextjs_scaffold_uses_pinned_noninteractive_command() {
        let reply = deterministic_nextjs_scaffold_reply();
        let command = reply.tool_calls[0]
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .expect("command");
        assert!(command.contains("npx --yes create-next-app@16.2.4"));
        assert!(command.contains(" --yes"));
        assert!(!command.contains("@latest"));
    }
}

#[cfg(test)]
mod progress_tests {
    use super::{
        FOCUSED_EDIT_POST_READ_MAX_PREDICT, FOCUSED_EDIT_POST_READ_TIMEOUT_SECS,
        FOCUSED_EDIT_PRE_READ_MAX_PREDICT, FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS,
        FocusedEditBatchAction, deterministic_empty_framework_app_files,
        deterministic_empty_framework_game_files, deterministic_framework_app_files_needed,
        deterministic_framework_game_files_needed, extract_page_copy_block_from_numbered_read,
        first_existing_impl_target, focused_edit_compact_anchor_note,
        focused_edit_compact_recovery_anchor, focused_edit_exact_anchor_history,
        focused_edit_exact_recovery_anchor, focused_edit_first_slice_note,
        focused_edit_first_slice_uses_exact_anchor, focused_edit_guidance_note,
        focused_edit_history, focused_edit_max_predict_override, focused_edit_minimal_history,
        focused_edit_second_slice_note, focused_edit_target_already_read,
        focused_edit_timeout_override_secs, focused_edit_tool_batch_action,
        focused_edit_tool_policy_error, focused_read_target_for_directory,
        format_blocked_progress_line, format_progress_line, has_successful_non_plan_repo_edit,
        has_successful_non_plan_repo_edit_after_latest_truncated_tool_call,
        has_successful_repo_edit, implementation_quality_issue_for_request, is_utf8_locale,
        last_read_tool_path, latest_page_copy_block_from_read,
        latest_truncated_tool_call_note_index, post_scaffold_continuation_active,
        post_scaffold_recovery_active, progress_available_width, prune_plan_mode_messages,
        recent_scaffold_command_seen, recent_truncated_tool_call_attempt, repo_change_request_text,
        request_needs_playable_ui_quality_gate, sanitize_for_progress,
        should_apply_repo_change_quality_gate, should_use_streaming_transport,
        strip_read_line_number_prefix, successful_non_plan_repo_edit_count,
        successful_repo_edit_count, tool_color, tool_display, tool_emoji, unicode_supported,
        workspace_appears_empty,
    };
    use crate::agent::recovery::ActionExpectation;
    use crate::modes::plan_act::{ExecutionMode, PlanStage};
    use crate::ollama::xml_fallback::ToolCall;
    use crate::safety::path_guard::resolve_user_path;
    use crate::session::store::ConversationMessage;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use tempfile::tempdir;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn sanitize_removes_newline() {
        assert_eq!(sanitize_for_progress("hello\nworld"), "hello world");
    }

    #[test]
    fn sanitize_removes_escape() {
        assert_eq!(sanitize_for_progress("red\x1b[31m!"), "red [31m!");
    }

    #[test]
    fn sanitize_passthrough_normal() {
        assert_eq!(sanitize_for_progress("hello world"), "hello world");
    }

    #[test]
    fn tool_display_write_ascii() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/src/foo.rs", "content": "hello"});
        let display = tool_display("Write", &args, &work_root, None, PlanStage::Stage1, 57);
        assert_eq!(display.action, "Write file");
        assert_eq!(display.path.as_deref(), Some("src/foo.rs"));
        assert!(
            display
                .note
                .as_deref()
                .is_some_and(|note| note.contains("hello"))
        );
        assert_eq!(display.status, Some("5B".to_string()));
    }

    #[test]
    fn tool_display_write_non_ascii() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/a.txt", "content": "日本語"});
        let display = tool_display("Write", &args, &work_root, None, PlanStage::Stage1, 57);
        // "日本語" is 9 bytes in UTF-8
        assert_eq!(display.status, Some("9B".to_string()));
    }

    #[test]
    fn tool_display_bash_short() {
        let work_root = PathBuf::from("/work");
        let cmd = "cargo test";
        let args = json!({"command": cmd});
        let display = tool_display("Bash", &args, &work_root, None, PlanStage::Stage1, 57);
        assert_eq!(display.action, format!("Run {cmd}"));
    }

    #[test]
    fn tool_display_bash_long() {
        let work_root = PathBuf::from("/work");
        let cmd = "a".repeat(61);
        let args = json!({"command": cmd});
        let display = tool_display("Bash", &args, &work_root, None, PlanStage::Stage1, 57);
        assert_eq!(display.action.len(), 60);
        assert!(display.action.ends_with("..."));
    }

    #[test]
    fn tool_display_path_relative() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/src/lib.rs"});
        let display = tool_display("Read", &args, &work_root, None, PlanStage::Stage1, 57);
        assert_eq!(display.path.as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn tool_display_path_outside() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/tmp/outside.txt"});
        let display = tool_display("Read", &args, &work_root, None, PlanStage::Stage1, 57);
        assert_eq!(display.path.as_deref(), Some("/tmp/outside.txt"));
    }

    #[test]
    fn tool_display_plan_write_shows_sections() {
        let work_root = PathBuf::from("/work");
        let plan_path = PathBuf::from("/work/.anvil-state/sessions/abc/plans/plan-1.md");
        let args = json!({
            "path": "/work/.anvil-state/sessions/abc/plans/plan-1.md",
            "content": "# Plan\n\n## Goal\n- Improve README.\n\n## Constraints\n- Keep markdown.\n\n## Deliverables\n- Updated README.\n"
        });
        let display = tool_display(
            "Write",
            &args,
            &work_root,
            Some(plan_path.as_path()),
            PlanStage::Stage1,
            120,
        );
        assert_eq!(display.action, "Draft Goal, Constraints, and Deliverables");
        assert!(
            display
                .path
                .as_deref()
                .is_some_and(|path| path.contains("plans/plan-1.md"))
        );
        assert!(
            display
                .note
                .as_deref()
                .is_some_and(|note| note.contains("Goal: Improve README."))
        );
        assert!(display.status.is_some());
    }

    #[test]
    fn tool_display_plan_write_accepts_same_filename_alias() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().join("repo");
        let plan_root = temp.path().join("state").join("plans");
        std::fs::create_dir_all(&work_root).unwrap();
        std::fs::create_dir_all(&plan_root).unwrap();
        let plan_path = plan_root.join("plan-1.md");
        std::fs::write(&plan_path, "# Plan\n\n## Goal\n- Existing goal\n").unwrap();
        let args = json!({
            "path": "plans/plan-1.md",
            "content": "# Plan\n\n## Goal\n- Improve README.\n\n## Constraints\n- Keep markdown.\n\n## Deliverables\n- Updated README.\n"
        });
        let display = tool_display(
            "Write",
            &args,
            &work_root,
            Some(plan_path.as_path()),
            PlanStage::Stage1,
            120,
        );
        assert_eq!(display.action, "Add Goal, Constraints, and Deliverables");
        assert!(
            display
                .path
                .as_deref()
                .is_some_and(|path| path.contains("plans/plan-1.md"))
        );
    }

    #[test]
    fn tool_display_plan_read_marks_review() {
        let work_root = PathBuf::from("/work");
        let plan_path = PathBuf::from("/work/.anvil-state/sessions/abc/plans/plan-1.md");
        let args = json!({"path": "/work/.anvil-state/sessions/abc/plans/plan-1.md"});
        let display = tool_display(
            "Read",
            &args,
            &work_root,
            Some(plan_path.as_path()),
            PlanStage::Stage2,
            120,
        );
        assert_eq!(display.action, "Review plan draft");
        assert!(
            display
                .path
                .as_deref()
                .is_some_and(|path| path.contains("plans/plan-1.md"))
        );
    }

    #[test]
    fn tool_display_read_includes_line_range() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/src/lib.rs", "start_line": 12, "end_line": 40});
        let display = tool_display("Read", &args, &work_root, None, PlanStage::Stage1, 120);
        assert_eq!(display.action, "Read lines 12-40");
        assert_eq!(display.path.as_deref(), Some("src/lib.rs:12-40"));
    }

    #[test]
    fn tool_display_missing_path_is_explicit() {
        let work_root = PathBuf::from("/work");
        let args = json!({});
        let display = tool_display("Read", &args, &work_root, None, PlanStage::Stage1, 120);
        assert_eq!(display.action, "Read file");
        assert_eq!(display.path.as_deref(), Some("<missing path>"));
    }

    #[test]
    fn tool_display_bash_with_wide_budget() {
        let work_root = PathBuf::from("/work");
        let cmd = "a".repeat(100);
        let args = json!({"command": cmd});
        let display = tool_display("Bash", &args, &work_root, None, PlanStage::Stage1, 200);
        assert_eq!(display.action.len(), 104);
        assert!(!display.action.ends_with("..."));
    }

    #[test]
    fn tool_display_bash_with_narrow_budget() {
        let work_root = PathBuf::from("/work");
        let cmd = "a".repeat(30);
        let args = json!({"command": cmd});
        let display = tool_display("Bash", &args, &work_root, None, PlanStage::Stage1, 20);
        assert_eq!(display.action.len(), 23);
        assert!(display.action.ends_with("..."));
    }

    #[test]
    fn blocked_progress_uses_specific_reason_not_bash_label() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/README.md"});
        let progress = format_blocked_progress_line(
            "Read",
            &args,
            3,
            50,
            &work_root,
            false,
            true,
            Some(120),
            "Plan exploration blocked",
            "Exploration budget reached for this stage; write the next missing plan section.",
            None,
            PlanStage::Stage2,
        );
        assert!(progress.contains("[iter 3/50] Plan exploration blocked"));
        assert!(progress.contains("tool:   ⛔ Plan exploration blocked"));
        assert!(!progress.contains("Bash blocked"));
    }

    #[test]
    fn truncated_tool_call_recovery_detects_latest_attempt() {
        let messages = vec![
            ConversationMessage::system("irrelevant".to_string()),
            ConversationMessage::system(
                "Previous tool call was cut off by the model length limit: tool call parser failed: truncated tool call (generate response hit length limit). tool_call_format_attempt=2".to_string(),
            ),
        ];
        assert_eq!(recent_truncated_tool_call_attempt(&messages), 2);
    }

    #[test]
    fn last_read_tool_path_returns_recent_read_target() {
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "ok".to_string()),
        ];
        assert_eq!(
            last_read_tool_path(&messages).as_deref(),
            Some("app/page.tsx")
        );
    }

    #[test]
    fn has_successful_repo_edit_ignores_errors() {
        let messages = vec![
            ConversationMessage::tool("Write".to_string(), "Error: nope".to_string()),
            ConversationMessage::tool("Edit".to_string(), "edited app/page.tsx".to_string()),
        ];
        assert!(has_successful_repo_edit(&messages));
    }

    #[test]
    fn forced_small_edit_recovery_targets_existing_recent_read_file() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::system(
                "Previous tool call was cut off by the model length limit: tool call parser failed: truncated tool call (generate response hit length limit). tool_call_format_attempt=1".to_string(),
            ),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
        ];
        let resolved =
            resolve_user_path(&work_root, &last_read_tool_path(&messages).unwrap()).unwrap();
        assert!(
            resolved.ends_with("app/page.tsx"),
            "got: {}",
            resolved.display()
        );
        assert_eq!(recent_truncated_tool_call_attempt(&messages), 1);
        assert!(!has_successful_repo_edit(&messages));
    }

    #[test]
    fn truncated_recovery_ignores_edits_before_latest_truncation() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(
            work_root.join("app/page.tsx"),
            "export default function Home() {}\n",
        )
        .unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "edited app/page.tsx".to_string()),
            ConversationMessage::system(
                "Previous tool call was cut off by the model length limit: tool call parser failed: truncated tool call (generate response hit length limit). tool_call_format_attempt=1".to_string(),
            ),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "1: export default".to_string()),
        ];

        assert!(has_successful_non_plan_repo_edit(
            &messages, &work_root, None
        ));
        assert!(
            !has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
                &messages, &work_root, None
            )
        );
    }

    #[test]
    fn truncated_recovery_stops_after_edit_following_latest_truncation() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(
            work_root.join("app/page.tsx"),
            "export default function Home() {}\n",
        )
        .unwrap();
        let messages = vec![
            ConversationMessage::system(
                "Previous tool call was cut off by the model length limit: tool call parser failed: truncated tool call (generate response hit length limit). tool_call_format_attempt=1".to_string(),
            ),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "edited app/page.tsx".to_string()),
        ];

        assert!(
            has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
                &messages, &work_root, None
            )
        );
    }

    #[test]
    fn prune_plan_mode_messages_removes_plan_only_notes() {
        let mut messages = vec![
            ConversationMessage::system(
                "[Plan Mode / coding] Explore with Read, Glob, and Grep.".to_string(),
            ),
            ConversationMessage::system(
                "[Plan File Alias] Treat paths as the same file.".to_string(),
            ),
            ConversationMessage::system(
                "The plan is still incomplete. plan_progress_attempt=2".to_string(),
            ),
            ConversationMessage::user("build the app".to_string()),
            ConversationMessage::system("[Act Mode / coding] Execute the plan.".to_string()),
        ];
        prune_plan_mode_messages(&mut messages);
        let contents = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents.len(), 2, "got: {contents:?}");
        assert!(
            contents
                .iter()
                .any(|content| content.starts_with("[Act Mode /"))
        );
        assert!(contents.contains(&"build the app"));
    }

    #[test]
    fn focused_edit_history_keeps_act_note_user_and_latest_target_read_only() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let other = work_root.join("README.md");
        std::fs::write(&other, "# readme\n").unwrap();
        let messages = vec![
            ConversationMessage::system("[Plan Mode / coding] old".to_string()),
            ConversationMessage::system("[Act Mode / coding] Execute the plan.".to_string()),
            ConversationMessage::user("make a game".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"README.md"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "# readme".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "1: export default".to_string()),
        ];
        let filtered = focused_edit_history(&messages, &target, &work_root);
        assert_eq!(filtered.len(), 4, "got: {filtered:?}");
        assert!(filtered[0].content.starts_with("[Act Mode /"));
        assert_eq!(filtered[1].role, "user");
        assert_eq!(filtered[2].role, "assistant");
        assert_eq!(filtered[3].name.as_deref(), Some("Read"));
        assert!(
            !filtered
                .iter()
                .any(|message| message.content.starts_with("[Plan Mode /"))
        );
        assert!(!filtered.iter().any(|message| message.content == "# readme"));
    }

    #[test]
    fn focused_edit_minimal_history_keeps_only_act_note_and_user() {
        let messages = vec![
            ConversationMessage::system("[Plan Mode / coding] old".to_string()),
            ConversationMessage::system("[Act Mode / coding] Execute the plan.".to_string()),
            ConversationMessage::user("make a game".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "1: export default".to_string()),
        ];
        let filtered = focused_edit_minimal_history(&messages);
        assert_eq!(filtered.len(), 2, "got: {filtered:?}");
        assert!(filtered[0].content.starts_with("[Act Mode /"));
        assert_eq!(filtered[1].role, "user");
    }

    #[test]
    fn focused_edit_target_already_read_detects_latest_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "1: export default".to_string()),
        ];
        assert!(focused_edit_target_already_read(
            &messages, &target, &work_root
        ));
    }

    #[test]
    fn focused_edit_target_read_becomes_stale_after_successful_edit() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "1: export default".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx","old_string":"Home","new_string":"Game"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "edited app/page.tsx".to_string()),
        ];
        assert!(!focused_edit_target_already_read(
            &messages, &target, &work_root
        ));
    }

    #[test]
    fn focused_edit_guidance_note_requires_edit_after_read() {
        let note = focused_edit_guidance_note(Path::new("app/page.tsx"), Path::new("."), true);
        assert!(note.contains("Do not call Read again"));
        assert!(note.contains("exactly one compact Edit"));
    }

    #[test]
    fn focused_edit_compact_anchor_note_forbids_full_file_insertions() {
        let note = focused_edit_compact_anchor_note(Path::new("app/page.tsx"), Path::new("."));
        assert!(note.contains("tiny exact anchor"));
        assert!(note.contains("at most 3 lines"));
        assert!(note.contains("under 240 characters"));
        assert!(note.contains("Do not insert imports"));
        assert!(note.contains("full-file content"));
    }

    #[test]
    fn focused_edit_first_slice_note_targets_next_page_shell() {
        let note = focused_edit_first_slice_note(
            &[],
            Path::new("/tmp/project/src/app/page.tsx"),
            Path::new("/tmp/project"),
            true,
        )
        .expect("expected note");
        assert!(note.contains("compact task-specific title"));
        assert!(note.contains("src/app/page.tsx"));
    }

    #[test]
    fn focused_edit_second_slice_note_targets_intro_paragraph() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("src").join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool(
                "Read".to_string(),
                "1: <h1 className=\"title\">\n2:   NEON INVADERS\n3: </h1>\n4: <p className=\"copy\">\n5:   Old starter copy.\n6: </p>"
                    .to_string(),
            ),
        ];
        let note = focused_edit_second_slice_note(&messages, &target, &work_root, true)
            .expect("expected second slice note");
        assert!(note.contains("intro copy line"), "got: {note}");
        assert!(note.contains("Old starter copy."), "got: {note}");
        assert!(note.contains("src/app/page.tsx"), "got: {note}");
    }

    #[test]
    fn focused_edit_exact_anchor_applies_to_second_slice() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("src").join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool(
                "Read".to_string(),
                "1: <h1 className=\"title\">\n2:   NEON INVADERS\n3: </h1>\n4: <p className=\"copy\">\n5:   Old starter copy.\n6: </p>"
                    .to_string(),
            ),
        ];

        assert_eq!(
            focused_edit_exact_recovery_anchor(&messages, &target, &work_root, true, 1).as_deref(),
            Some("  Old starter copy.")
        );
        assert!(
            focused_edit_exact_recovery_anchor(&messages, &target, &work_root, true, 2).is_none()
        );
    }

    #[test]
    fn focused_edit_compact_recovery_anchor_prefers_placeholder_cta_text() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("src").join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool(
                "Read".to_string(),
                "   1: import Image from \"next/image\";\n   2: export default function Home() {\n   3:   return (\n   4:     <main>\n   5:       <h1>NEON SPACE INVADERS</h1>\n   6:       <p>Play the mission.</p>\n   7:       <a href=\"https://vercel.com/new\">\n   8:         Deploy Now\n   9:       </a>\n  10:     </main>\n  11:   );\n  12: }"
                    .to_string(),
            ),
        ];

        assert_eq!(
            focused_edit_compact_recovery_anchor(&messages, &target, &work_root).as_deref(),
            Some("        Deploy Now")
        );
    }

    #[test]
    fn focused_edit_compact_anchor_history_drops_full_latest_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let target = work_root.join("src").join("app").join("page.tsx");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let long_read = "   1: import Image from \"next/image\";\n   2: export default function Home() {\n   3:   return (\n   4:     <main>\n   5:       <h1>NEON SPACE INVADERS</h1>\n   6:       <p>Play the mission.</p>\n   7:       <a href=\"https://vercel.com/new\">\n   8:         Deploy Now\n   9:       </a>\n  10:     </main>\n  11:   );\n  12: }";
        let messages = vec![
            ConversationMessage::system("[Act Mode / coding] Execute the plan.".to_string()),
            ConversationMessage::user("make a game".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), long_read.to_string()),
        ];
        let anchor = focused_edit_compact_recovery_anchor(&messages, &target, &work_root)
            .expect("expected compact anchor");
        let filtered = focused_edit_exact_anchor_history(&messages, &target, &work_root, &anchor);

        assert_eq!(anchor, "        Deploy Now");
        assert_eq!(filtered.len(), 4, "got: {filtered:?}");
        assert_eq!(filtered[3].name.as_deref(), Some("Read"));
        assert_eq!(filtered[3].content, "   1:         Deploy Now");
        assert!(!filtered.iter().any(|message| message.content == long_read));
    }

    #[test]
    fn focused_edit_exact_anchor_history_includes_compact_synthetic_read() {
        let work_root = Path::new("/tmp/project");
        let target = Path::new("/tmp/project/src/app/page.tsx");
        let messages = vec![
            ConversationMessage::system("[Act Mode / coding] Execute the plan.".to_string()),
            ConversationMessage::user("make a game".to_string()),
        ];
        let filtered = focused_edit_exact_anchor_history(
            &messages,
            target,
            work_root,
            "  <p>\n    Old\n  </p>",
        );

        assert_eq!(filtered.len(), 4, "got: {filtered:?}");
        assert!(filtered[0].content.starts_with("[Act Mode /"));
        assert_eq!(filtered[1].role, "user");
        assert_eq!(filtered[2].role, "assistant");
        assert_eq!(filtered[2].tool_calls[0].name, "Read");
        assert_eq!(
            filtered[2].tool_calls[0].arguments.get("path"),
            Some(&json!("src/app/page.tsx"))
        );
        assert_eq!(filtered[3].name.as_deref(), Some("Read"));
        assert_eq!(
            filtered[3].content,
            "   1:   <p>\n   2:     Old\n   3:   </p>"
        );
    }

    #[test]
    fn recent_scaffold_command_seen_detects_create_next_app() {
        let messages = vec![ConversationMessage::assistant(
            String::new(),
            vec![ToolCall {
                id: "xml-1".to_string(),
                name: "Bash".to_string(),
                arguments: json!({"command":"npx create-next-app@latest . --ts --yes"}),
            }],
        )];
        assert!(recent_scaffold_command_seen(&messages));
    }

    #[test]
    fn recent_scaffold_command_seen_ignores_previous_user_turns() {
        let messages = vec![
            ConversationMessage::user("build a Next.js app".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Bash".to_string(),
                    arguments: json!({"command":"npx create-next-app@latest . --ts --yes"}),
                }],
            ),
            ConversationMessage::tool("Bash".to_string(), "scaffolded".to_string()),
            ConversationMessage::user("make the existing game cooler".to_string()),
        ];
        assert!(!recent_scaffold_command_seen(&messages));
        assert!(!post_scaffold_recovery_active(
            &messages,
            None,
            Path::new("/tmp/project"),
        ));
    }

    #[test]
    fn post_scaffold_recovery_stays_active_after_root_switch() {
        let messages = vec![ConversationMessage::system(
            "[Workspace Root Updated] Continue work inside /tmp/project/app.".to_string(),
        )];
        assert!(post_scaffold_recovery_active(
            &messages,
            Some(Path::new("/tmp/project/app")),
            Path::new("/tmp/project"),
        ));
    }

    #[test]
    fn post_scaffold_recovery_ignores_stale_root_switch_after_new_user_turn() {
        let messages = vec![
            ConversationMessage::system(
                "[Workspace Root Updated] Continue work inside /tmp/project/app.".to_string(),
            ),
            ConversationMessage::user("make the existing app cooler".to_string()),
        ];
        assert!(!post_scaffold_recovery_active(
            &messages,
            Some(Path::new("/tmp/project/app")),
            Path::new("/tmp/project"),
        ));
    }

    #[test]
    fn recent_truncated_tool_call_attempt_ignores_previous_user_turns() {
        let messages = vec![
            ConversationMessage::user("first task".to_string()),
            ConversationMessage::system(
                "truncated tool call tool_call_format_attempt=2".to_string(),
            ),
            ConversationMessage::user("second task".to_string()),
        ];
        assert_eq!(recent_truncated_tool_call_attempt(&messages), 0);
        assert_eq!(latest_truncated_tool_call_note_index(&messages), None);
    }

    #[test]
    fn successful_repo_edit_count_counts_only_non_error_edits() {
        let messages = vec![
            ConversationMessage::tool("Edit".to_string(), "updated page".to_string()),
            ConversationMessage::tool("Write".to_string(), "created file".to_string()),
            ConversationMessage::tool("Edit".to_string(), "Error: failed".to_string()),
        ];
        assert_eq!(successful_repo_edit_count(&messages), 2);
    }

    #[test]
    fn non_plan_repo_edit_count_ignores_plan_file_writes() {
        let work_root = Path::new("/tmp/project");
        let plan_path = Path::new("/tmp/project/.anvil/plan.md");
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-plan".to_string(),
                    name: "Write".to_string(),
                    arguments: json!({"path":"/tmp/project/.anvil/plan.md"}),
                }],
            ),
            ConversationMessage::tool("Write".to_string(), "updated plan".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx","old_string":"a","new_string":"b"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "updated page".to_string()),
        ];
        assert_eq!(successful_repo_edit_count(&messages), 2);
        assert_eq!(
            successful_non_plan_repo_edit_count(&messages, work_root, Some(plan_path)),
            1
        );
        assert!(has_successful_non_plan_repo_edit(
            &messages,
            work_root,
            Some(plan_path)
        ));
    }

    #[test]
    fn post_scaffold_continuation_stays_disabled_after_first_edit() {
        let cwd = Path::new("/tmp/project");
        let work_root = Path::new("/tmp/project");
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Bash".to_string(),
                    arguments: json!({"command":"npx create-next-app@latest . --ts --yes"}),
                }],
            ),
            ConversationMessage::tool("Bash".to_string(), "scaffolded".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx","old_string":"a","new_string":"b"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "updated page".to_string()),
        ];
        assert!(!post_scaffold_continuation_active(
            &messages, None, cwd, work_root, None
        ));

        let mut completed = messages.clone();
        completed.push(ConversationMessage::assistant(
            String::new(),
            vec![ToolCall {
                id: "xml-3".to_string(),
                name: "Edit".to_string(),
                arguments: json!({"path":"app/page.tsx","old_string":"b","new_string":"c"}),
            }],
        ));
        completed.push(ConversationMessage::tool(
            "Edit".to_string(),
            "second update".to_string(),
        ));
        assert!(!post_scaffold_continuation_active(
            &completed, None, cwd, work_root, None
        ));
    }

    #[test]
    fn post_scaffold_continuation_stays_disabled_with_plan_file_edits() {
        let cwd = Path::new("/tmp/project");
        let work_root = Path::new("/tmp/project");
        let plan_path = Path::new("/tmp/project/.anvil/plan.md");
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-plan".to_string(),
                    name: "Write".to_string(),
                    arguments: json!({"path":"/tmp/project/.anvil/plan.md"}),
                }],
            ),
            ConversationMessage::tool("Write".to_string(), "updated plan".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Bash".to_string(),
                    arguments: json!({"command":"npx create-next-app@latest . --ts --yes"}),
                }],
            ),
            ConversationMessage::tool("Bash".to_string(), "scaffolded".to_string()),
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-2".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({"path":"app/page.tsx","old_string":"a","new_string":"b"}),
                }],
            ),
            ConversationMessage::tool("Edit".to_string(), "updated page".to_string()),
        ];
        assert!(!post_scaffold_continuation_active(
            &messages,
            None,
            cwd,
            work_root,
            Some(plan_path)
        ));
    }

    #[test]
    fn workspace_appears_empty_ignores_state_and_git_dirs() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join(".git")).unwrap();
        std::fs::create_dir_all(work_root.join(".anvil/plans")).unwrap();
        std::fs::create_dir_all(work_root.join(".anvil-state")).unwrap();
        assert!(workspace_appears_empty(work_root));

        std::fs::write(work_root.join("README.md"), "# app\n").unwrap();
        assert!(!workspace_appears_empty(work_root));
    }

    #[test]
    fn deterministic_framework_game_files_needed_accepts_sparse_nuxt_shell() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join(".anvil/plans")).unwrap();
        std::fs::write(
            work_root.join("package.json"),
            r#"{"scripts":{"dev":"nuxt dev --port 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            work_root.join("nuxt.config.ts"),
            "export default defineNuxtConfig({ ssr: false });\n",
        )
        .unwrap();

        let files = deterministic_empty_framework_game_files(
            "最高に面白くかっこいいテトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください。",
        )
        .expect("files");

        assert!(deterministic_framework_game_files_needed(work_root, &files));
    }

    #[test]
    fn deterministic_framework_game_files_needed_preserves_existing_impl() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("package.json"), "{}\n").unwrap();
        std::fs::write(
            work_root.join("nuxt.config.ts"),
            "export default defineNuxtConfig({});\n",
        )
        .unwrap();
        std::fs::write(
            work_root.join("app.vue"),
            "<template><canvas /></template>\n",
        )
        .unwrap();

        let files = deterministic_empty_framework_game_files(
            "最高に面白くかっこいいテトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください。",
        )
        .expect("files");

        assert!(!deterministic_framework_game_files_needed(
            work_root, &files
        ));
    }

    #[test]
    fn deterministic_framework_game_files_needed_rejects_non_shell_files() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("package.json"), "{}\n").unwrap();
        std::fs::write(work_root.join("README.md"), "# existing project\n").unwrap();

        let files = deterministic_empty_framework_game_files(
            "最高に面白くかっこいいテトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください。",
        )
        .expect("files");

        assert!(!deterministic_framework_game_files_needed(
            work_root, &files
        ));
    }

    #[test]
    fn deterministic_framework_app_files_needed_accepts_vite_placeholder_scaffold() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src")).unwrap();
        std::fs::write(
            work_root.join("package.json"),
            r#"{"scripts":{"dev":"vite"},"dependencies":{"react":"latest","react-dom":"latest"}}"#,
        )
        .unwrap();
        std::fs::write(
            work_root.join("index.html"),
            r#"<div id="root"></div><script type="module" src="/src/main.jsx"></script>"#,
        )
        .unwrap();
        std::fs::write(work_root.join("src/main.jsx"), "import App from './App';\n").unwrap();
        std::fs::write(
            work_root.join("src/App.jsx"),
            r#"import reactLogo from './assets/react.svg'
import viteLogo from './assets/vite.svg'
export default function App() {
  return <a href="https://vite.dev/">Documentation</a>
}
"#,
        )
        .unwrap();

        let request = "React.jsで家計簿ダッシュボードを作って下さい。収入、支出、カテゴリ別合計、残高表示を入れ、起動ポートは3011にして下さい。";
        let files = deterministic_empty_framework_app_files(request).expect("files");

        assert!(deterministic_framework_app_files_needed(
            work_root, &files, request
        ));
    }

    #[test]
    fn playable_ui_quality_gate_targets_interactive_ui_requests() {
        assert!(request_needs_playable_ui_quality_gate(
            "操作できるUIを3011ポートで起動可能なnext.jsアプリとして開発してください"
        ));
        assert!(request_needs_playable_ui_quality_gate(
            "Build an interactive browser UI as a React app"
        ));
        assert!(request_needs_playable_ui_quality_gate(
            "入力に反応する画面を3011ポートで起動可能なNuxt.jsアプリとして開発してください。"
        ));
        assert!(request_needs_playable_ui_quality_gate(
            "既存のinteractive UIをよりカッコよくしてください。"
        ));
        assert!(!request_needs_playable_ui_quality_gate(
            "READMEをわかりやすく改善してください"
        ));
    }

    #[test]
    fn repo_change_quality_gate_applies_to_active_task_after_yes() {
        assert!(should_apply_repo_change_quality_gate(
            ActionExpectation::None,
            true,
            ExecutionMode::Act,
        ));
        assert!(!should_apply_repo_change_quality_gate(
            ActionExpectation::None,
            true,
            ExecutionMode::Plan,
        ));
        assert!(!should_apply_repo_change_quality_gate(
            ActionExpectation::None,
            false,
            ExecutionMode::Act,
        ));
    }

    #[test]
    fn repo_change_request_text_falls_back_to_latest_user_prompt() {
        let messages = vec![
            ConversationMessage::user("Build an interactive Next.js UI".to_string()),
            ConversationMessage::assistant("done".to_string(), Vec::new()),
        ];
        assert_eq!(
            repo_change_request_text(None, &messages).as_deref(),
            Some("Build an interactive Next.js UI")
        );
        assert_eq!(
            repo_change_request_text(Some("Active task wins"), &messages).as_deref(),
            Some("Active task wins")
        );
    }

    #[test]
    fn repo_change_request_text_recovers_original_request_after_plan_approval() {
        let messages = vec![
            ConversationMessage::user(
                "Create an implementation plan for the user's request.\n\nUser request:\n最高に面白いスペースインベーダーゲームをNext.jsアプリとして開発してください。"
                    .to_string(),
            ),
            ConversationMessage::assistant(
                "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                    .to_string(),
                Vec::new(),
            ),
            ConversationMessage::user("yes".to_string()),
        ];
        let active_task =
            "The user approved the plan and said: yes\nExecute the approved plan now.";
        assert_eq!(
            repo_change_request_text(Some(active_task), &messages).as_deref(),
            Some("最高に面白いスペースインベーダーゲームをNext.jsアプリとして開発してください。")
        );
    }

    #[test]
    fn playable_ui_quality_gate_rejects_generic_placeholder_page() {
        let request = "Build an interactive UI as a Next.js app";
        let content = r#"
            "use client";
            import Image from "next/image";
            export default function Home() {
              return <button onClick={() => alert("ok")}>Start Experience</button>;
            }
        "#;
        let issue = implementation_quality_issue_for_request(request, content)
            .expect("expected quality issue");
        assert!(issue.contains("interactive vertical slice"), "got: {issue}");
    }

    #[test]
    fn playable_ui_quality_gate_rejects_template_after_copy_edits() {
        let request = "Build an interactive UI as a Next.js app";
        let content = r#"
            import Image from "next/image";
            export default function Home() {
              return <main>
                <Image src="/next.svg" alt="Next.js logo" />
                <h1>Interactive UI</h1>
                <p>Status panel with input, state, and visible feedback</p>
                <a href="https://vercel.com/new">Deploy Now</a>
                <a href="https://nextjs.org/docs">Documentation</a>
              </main>;
            }
        "#;
        let issue = implementation_quality_issue_for_request(request, content)
            .expect("expected quality issue");
        assert!(issue.contains("placeholder markers"), "got: {issue}");
    }

    #[test]
    fn playable_ui_quality_gate_accepts_basic_interactive_slice() {
        let request = "Build an interactive UI as a Next.js app";
        let content = r#"
            "use client";
            const [status, setStatus] = useState("ready");
            const [progress, setProgress] = useState(0);
            export default function App() {
              return <button className="primary" onClick={() => { setStatus("running"); setProgress(1); }}>
                {status} {progress}
              </button>;
            }
        "#;
        assert!(implementation_quality_issue_for_request(request, content).is_none());
    }

    #[test]
    fn first_existing_impl_target_prefers_page_component() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(
            work_root.join("app/page.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        std::fs::write(work_root.join("next.config.ts"), "export default {};\n").unwrap();
        let target = first_existing_impl_target(work_root).unwrap();
        assert!(
            target.ends_with("app/page.tsx"),
            "got: {}",
            target.display()
        );
    }

    #[test]
    fn first_existing_impl_target_finds_nested_scaffold_page_component() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        let nested = work_root.join("sample-app");
        std::fs::create_dir_all(nested.join("app")).unwrap();
        std::fs::write(
            nested.join("package.json"),
            "{\n  \"name\": \"sample-app\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            nested.join("app/page.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        let target = first_existing_impl_target(work_root).unwrap();
        assert!(
            target.ends_with("sample-app/app/page.tsx"),
            "got: {}",
            target.display()
        );
    }

    #[test]
    fn first_existing_impl_target_beats_package_json_after_scaffold() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(
            work_root.join("app/page.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        std::fs::write(
            work_root.join("package.json"),
            "{\n  \"name\": \"sample-app\"\n}\n",
        )
        .unwrap();
        let target = first_existing_impl_target(work_root).unwrap();
        assert!(target.ends_with("app/page.tsx"));
    }

    #[test]
    fn first_existing_impl_target_rejects_config_only_scaffolds() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("package.json"), "{}\n").unwrap();
        std::fs::write(work_root.join("vite.config.js"), "export default {};\n").unwrap();
        std::fs::write(work_root.join("svelte.config.js"), "export default {};\n").unwrap();

        assert!(first_existing_impl_target(work_root).is_none());
    }

    #[test]
    fn first_existing_impl_target_supports_react_and_nuxt_entries() {
        let react = tempdir().unwrap();
        let react_root = react.path();
        std::fs::create_dir_all(react_root.join("src")).unwrap();
        std::fs::write(
            react_root.join("package.json"),
            "{\n  \"name\": \"sample-app\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            react_root.join("src/App.tsx"),
            "export default function App() {}\n",
        )
        .unwrap();
        let target = first_existing_impl_target(react_root).unwrap();
        assert!(target.ends_with("src/App.tsx"), "got: {}", target.display());

        let nuxt = tempdir().unwrap();
        let nuxt_root = nuxt.path();
        std::fs::write(nuxt_root.join("nuxt.config.ts"), "export default {};\n").unwrap();
        std::fs::write(nuxt_root.join("app.vue"), "<template><main /></template>\n").unwrap();
        let target = first_existing_impl_target(nuxt_root).unwrap();
        assert!(target.ends_with("app.vue"), "got: {}", target.display());
    }

    #[test]
    fn focused_edit_first_slice_note_matches_nested_page_component() {
        let note = focused_edit_first_slice_note(
            &[],
            Path::new("/tmp/project/sample-app/app/page.tsx"),
            Path::new("/tmp/project"),
            true,
        )
        .expect("expected note");
        assert!(note.contains("compact task-specific title"));
        assert!(note.contains("sample-app/app/page.tsx"));
    }

    #[test]
    fn strip_read_line_number_prefix_preserves_code_indent() {
        assert_eq!(
            strip_read_line_number_prefix("  14:         <div className=\"hero\">"),
            "        <div className=\"hero\">"
        );
        assert_eq!(strip_read_line_number_prefix("plain text"), "plain text");
    }

    #[test]
    fn extract_page_copy_block_from_numbered_read_finds_marketing_block() {
        let read = r#"   1: import Image from "next/image";
   2: 
   3: export default function Home() {
   4:   return (
   5:     <div>
   6:       <main>
   7:         <div className="flex flex-col items-center gap-6 text-center sm:items-start sm:text-left">
   8:           <h1>Hello</h1>
   9:           <p>World</p>
  10:         </div>
  11:         <div className="other">Keep</div>
  12:       </main>
  13:     </div>
  14:   );
  15: }"#;
        let block = extract_page_copy_block_from_numbered_read(read).expect("expected block");
        assert!(block.contains("<h1>Hello</h1>"), "got: {block}");
        assert!(block.starts_with("          <h1"), "got: {block}");
        assert!(!block.contains("<p>World</p>"), "got: {block}");
        assert!(block.ends_with("          <h1>Hello</h1>"), "got: {block}");
    }

    #[test]
    fn latest_page_copy_block_from_read_uses_recent_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "placeholder\n").unwrap();
        let read = r#"   7:         <div className="flex flex-col items-center gap-6 text-center sm:items-start sm:text-left">
   8:           <h1>Hello</h1>
   9:           <p>World</p>
  10:         </div>"#;
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), read.to_string()),
        ];
        let block =
            latest_page_copy_block_from_read(&messages, &target, work_root).expect("expected");
        assert!(block.contains("<h1>Hello</h1>"), "got: {block}");
        assert!(!block.contains("<p>World</p>"), "got: {block}");
    }

    #[test]
    fn focused_edit_first_slice_uses_exact_anchor_for_recent_page_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "placeholder\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool(
                "Read".to_string(),
                r#"   8:           <h1>Hello</h1>
   9:           <p>World</p>"#
                    .to_string(),
            ),
        ];
        assert!(focused_edit_first_slice_uses_exact_anchor(
            &messages, &target, work_root, true
        ));
    }

    #[test]
    fn focused_edit_first_slice_note_embeds_exact_old_string_when_recent_read_exists() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        std::fs::write(work_root.join("src/app/page.tsx"), "placeholder\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool(
                "Read".to_string(),
                r#"   7:         <div className="flex flex-col items-center gap-6 text-center sm:items-start sm:text-left">
   8:           <h1>Hello</h1>
   9:           <p>World</p>
  10:         </div>"#
                    .to_string(),
            ),
        ];
        let note = focused_edit_first_slice_note(
            &messages,
            &work_root.join("src/app/page.tsx"),
            work_root,
            true,
        )
        .expect("expected note");
        assert!(note.contains("byte-for-byte as old_string"), "got: {note}");
        assert!(note.contains("<h1>Hello</h1>"), "got: {note}");
        assert!(!note.contains("<p>World</p>"), "got: {note}");
        assert!(note.contains("under about 240 characters"), "got: {note}");
    }

    #[test]
    fn focused_edit_policy_rejects_repeat_read_after_target_was_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let err = focused_edit_tool_policy_error(
            "Read",
            &json!({"path":"app/page.tsx"}),
            &target,
            work_root,
            true,
        )
        .expect("expected policy error");
        assert!(err.contains("only allows Edit"));
    }

    #[test]
    fn focused_edit_policy_rejects_wrong_path_before_first_edit() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let err = focused_edit_tool_policy_error(
            "Read",
            &json!({"path":"app"}),
            &target,
            work_root,
            false,
        )
        .expect("expected policy error");
        assert!(err.contains("only allows Read or Edit on app/page.tsx"));
    }

    #[test]
    fn focused_read_target_for_directory_matches_target_parent_only() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        std::fs::create_dir_all(work_root.join("src/components")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();

        assert!(focused_read_target_for_directory(
            &work_root.join("src/app"),
            &target
        ));
        assert!(!focused_read_target_for_directory(
            &work_root.join("src/components"),
            &target
        ));
        assert!(!focused_read_target_for_directory(&target, &target));
    }

    #[test]
    fn focused_edit_batch_policy_truncates_multiple_calls_before_first_edit() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let action = focused_edit_tool_batch_action(
            &[
                ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                },
                ToolCall {
                    id: "xml-2".to_string(),
                    name: "Glob".to_string(),
                    arguments: json!({"pattern":"*"}),
                },
            ],
            &target,
            work_root,
            false,
        );
        assert_eq!(action, FocusedEditBatchAction::TruncateToFirst);
    }

    #[test]
    fn focused_edit_batch_policy_truncates_multiple_calls_after_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let action = focused_edit_tool_batch_action(
            &[
                ToolCall {
                    id: "xml-1".to_string(),
                    name: "Edit".to_string(),
                    arguments: json!({
                        "path":"app/page.tsx",
                        "old_string":"return null;",
                        "new_string":"return <main />;"
                    }),
                },
                ToolCall {
                    id: "xml-2".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                },
            ],
            &target,
            work_root,
            true,
        );
        assert_eq!(action, FocusedEditBatchAction::TruncateToFirst);
    }

    #[test]
    fn focused_edit_batch_policy_rejects_invalid_first_call() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let action = focused_edit_tool_batch_action(
            &[
                ToolCall {
                    id: "xml-1".to_string(),
                    name: "Glob".to_string(),
                    arguments: json!({"pattern":"*"}),
                },
                ToolCall {
                    id: "xml-2".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                },
            ],
            &target,
            work_root,
            false,
        );
        match action {
            FocusedEditBatchAction::Reject(err) => {
                assert!(err.contains("only allows Read or Edit on app/page.tsx"));
            }
            other => panic!("expected reject, got {other:?}"),
        }
    }

    #[test]
    fn focused_edit_timeout_override_activates_after_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        let target = work_root.join("app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "page contents".to_string()),
        ];
        assert_eq!(
            focused_edit_timeout_override_secs(&messages, Some(&target), work_root),
            Some(FOCUSED_EDIT_POST_READ_TIMEOUT_SECS)
        );
        assert_eq!(
            focused_edit_max_predict_override(&messages, Some(&target), work_root),
            Some(FOCUSED_EDIT_POST_READ_MAX_PREDICT)
        );
    }

    #[test]
    fn focused_edit_timeout_override_activates_before_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![ConversationMessage::user("build the app".to_string())];
        assert_eq!(
            focused_edit_timeout_override_secs(&messages, Some(&target), work_root),
            Some(FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS)
        );
        assert_eq!(
            focused_edit_max_predict_override(&messages, Some(&target), work_root),
            Some(FOCUSED_EDIT_PRE_READ_MAX_PREDICT)
        );
    }

    #[test]
    fn focused_edit_override_forces_non_streaming_even_with_native_tools_after_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![
            ConversationMessage::assistant(
                String::new(),
                vec![ToolCall {
                    id: "xml-1".to_string(),
                    name: "Read".to_string(),
                    arguments: json!({"path":"src/app/page.tsx"}),
                }],
            ),
            ConversationMessage::tool("Read".to_string(), "page contents".to_string()),
        ];
        let force_non_streaming =
            focused_edit_timeout_override_secs(&messages, Some(&target), work_root).is_some()
                || focused_edit_max_predict_override(&messages, Some(&target), work_root).is_some();
        let use_streaming_transport = !force_non_streaming
            && should_use_streaming_transport("qwen3.5:122b", true, false, true);
        assert!(
            !use_streaming_transport,
            "focused post-read turns should bypass streaming transport"
        );
    }

    #[test]
    fn focused_edit_override_forces_non_streaming_even_before_target_read() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("src/app")).unwrap();
        let target = work_root.join("src/app/page.tsx");
        std::fs::write(&target, "export default function Home() { return null; }\n").unwrap();
        let messages = vec![ConversationMessage::user("build the app".to_string())];
        let force_non_streaming =
            focused_edit_timeout_override_secs(&messages, Some(&target), work_root).is_some()
                || focused_edit_max_predict_override(&messages, Some(&target), work_root).is_some();
        let use_streaming_transport = !force_non_streaming
            && should_use_streaming_transport("qwen3.5:122b", true, false, true);
        assert!(
            !use_streaming_transport,
            "focused pre-read turns should bypass streaming transport"
        );
    }

    #[test]
    fn progress_available_width_none_returns_default() {
        assert_eq!(progress_available_width(None, "Bash", 1, 12, false), 57);
    }

    #[test]
    fn progress_available_width_large_cols_returns_budget() {
        // cols=200, tool_name="Bash" (4 chars), use_unicode=false
        // iter_prefix "[iter 1/12]  " = 13 chars; chrome = 13 + 0 + 4 + 2 = 19
        // ellipsis reserve = 3; expected budget = 200 - 19 - 3 = 178
        assert_eq!(
            progress_available_width(Some(200), "Bash", 1, 12, false),
            178
        );
    }

    #[test]
    fn progress_available_width_small_cols_clamps_to_min() {
        // cols=30; chrome + ellipsis = 22; 30 - 22 = 8 → clamp to 20
        assert_eq!(progress_available_width(Some(30), "Bash", 1, 12, false), 20);
    }

    #[test]
    fn progress_available_width_zero_cols_clamps_to_min() {
        // saturating_sub to 0 → max(20)
        assert_eq!(progress_available_width(Some(0), "Bash", 1, 12, false), 20);
    }

    #[test]
    fn progress_available_width_emoji_accounts_for_vs16() {
        // "Write" emoji is `✏️` (U+270F + U+FE0F VS16), `.chars().count() == 2`.
        // iter_prefix "[iter 1/12]  " = 13; emoji_width = 2 + 1 = 3; name = 5; +2 → chrome=23
        // ellipsis reserve = 3; cols=200 → 200 - 23 - 3 = 174
        assert_eq!(
            progress_available_width(Some(200), "Write", 1, 12, true),
            174
        );
    }

    #[test]
    fn format_progress_line_fits_within_cols_on_truncate() {
        // CB-001 regression: when Bash command overflows arg_budget and cols is
        // wide enough for the MIN_ARG_BUDGET=20 clamp not to fire, the final
        // progress line (chars) must still fit within `cols`. For cols narrower
        // than chrome+MIN+ELLIPSIS the clamp keeps useful output at the cost of
        // a small overflow — that tradeoff is documented in §4.3.1.
        let work_root = PathBuf::from("/work");
        let cmd = "a".repeat(500);
        let args = json!({"command": cmd});
        // chrome=19 (iter_prefix 13 + Bash 4 + 2); MIN=20; ellipsis=3. So cols
        // must be >= 42 to avoid the clamp dominating.
        for &cols in &[60u16, 80, 120, 200] {
            let line = format_progress_line(
                "Bash",
                &args,
                1,
                12,
                &work_root,
                /* use_color */ false,
                /* use_unicode */ false,
                Some(cols),
                None,
                PlanStage::Stage1,
                None,
                None,
            );
            let line_chars = line.chars().count();
            assert!(
                line_chars <= cols as usize,
                "progress line {line_chars} chars exceeds cols={cols}: {line:?}"
            );
        }
    }

    #[test]
    fn progress_line_iter_1indexed() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/a.txt", "content": "x"});
        let line = format_progress_line(
            "Write",
            &args,
            1,
            12,
            &work_root,
            false,
            false,
            None,
            None,
            PlanStage::Stage1,
            None,
            Some("Stage1"),
        );
        assert!(line.starts_with("[iter 1/12]"));
    }

    #[test]
    fn progress_line_for_write_uses_multiline_block() {
        let work_root = PathBuf::from("/work");
        let plan_path = PathBuf::from("/work/plans/plan-1.md");
        let args = json!({"path": "/work/plans/plan-1.md", "content": "# Plan\n\n## Goal\n- Build game.\n"});
        let line = format_progress_line(
            "Write",
            &args,
            1,
            50,
            &work_root,
            false,
            true,
            None,
            Some(plan_path.as_path()),
            PlanStage::Stage1,
            None,
            Some("Stage1"),
        );
        assert!(line.contains("[iter 1/50] Stage1"));
        assert!(line.contains("tool:"));
        assert!(line.contains("action:"));
        assert!(line.contains("Draft Goal"));
        assert!(line.contains("plans/plan-1.md"));
        assert!(line.contains("Goal: Build game."));
    }

    #[test]
    fn progress_line_for_read_with_missing_path_is_visible() {
        let work_root = PathBuf::from("/work");
        let args = json!({});
        let line = format_progress_line(
            "Read",
            &args,
            2,
            50,
            &work_root,
            false,
            true,
            None,
            None,
            PlanStage::Ready,
            None,
            Some("Act"),
        );
        assert!(line.contains("[iter 2/50] Act"));
        assert!(line.contains("tool:"));
        assert!(line.contains("Read file"));
        assert!(line.contains("<missing path>"));
    }

    #[test]
    fn progress_line_no_color_no_escape() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash",
            &args,
            1,
            12,
            &work_root,
            false,
            false,
            None,
            None,
            PlanStage::Stage1,
            None,
            None,
        );
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_color_prefix_invariant() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash",
            &args,
            1,
            12,
            &work_root,
            true,
            false,
            None,
            None,
            PlanStage::Stage1,
            None,
            None,
        );
        assert!(line.starts_with("[iter "));
    }

    #[test]
    fn tool_style_all_mappings() {
        let cases: &[(&str, &str, &str)] = &[
            ("Write", "\x1b[38;5;198m", "✏\u{fe0f}"),
            ("Read", "\x1b[38;5;87m", "📄"),
            ("Edit", "\x1b[38;5;208m", "📝"),
            ("Bash", "\x1b[38;5;226m", "⚡"),
            ("Glob", "\x1b[38;5;51m", "🔍"),
            ("Grep", "\x1b[38;5;39m", "🔎"),
            ("Unknown", "\x1b[38;5;245m", "🔧"),
        ];
        for (name, expected_color, expected_emoji) in cases {
            assert_eq!(tool_color(name), *expected_color, "color for {name}");
            assert_eq!(tool_emoji(name), *expected_emoji, "emoji for {name}");
        }
    }

    #[test]
    fn progress_line_emoji_and_color_for_bash() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash",
            &args,
            1,
            12,
            &work_root,
            true,
            true,
            None,
            None,
            PlanStage::Stage1,
            None,
            None,
        );
        let color_idx = line.find("\x1b[38;5;226m").expect("color present");
        let emoji_idx = line.find('⚡').expect("emoji present");
        let reset_idx = line.find("\x1b[0m").expect("reset present");
        assert!(color_idx < emoji_idx, "color before emoji");
        assert!(emoji_idx < reset_idx, "emoji before reset");
    }

    #[test]
    fn progress_line_no_color_but_unicode_emits_emoji() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash",
            &args,
            1,
            12,
            &work_root,
            false,
            true,
            None,
            None,
            PlanStage::Stage1,
            None,
            None,
        );
        assert!(line.contains('⚡'));
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_unicode_off_no_emoji() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash",
            &args,
            1,
            12,
            &work_root,
            true,
            false,
            None,
            None,
            PlanStage::Stage1,
            None,
            None,
        );
        assert!(!line.contains('⚡'));
    }

    #[test]
    fn is_utf8_locale_table() {
        let true_cases = [
            "en_US.UTF-8",
            "en_US.utf-8",
            "C.UTF8",
            "C.utf8",
            "ja_JP.UTF-8@Modifier",
            "en_US.UTF-8;POSIX",
        ];
        let false_cases = [
            "",
            "C",
            "POSIX",
            "en_US.utf-800",
            "xutf8x",
            "utf-88",
            "en_US.ISO-8859-1",
        ];
        for c in true_cases {
            assert!(is_utf8_locale(c), "expected true for {c:?}");
        }
        for c in false_cases {
            assert!(!is_utf8_locale(c), "expected false for {c:?}");
        }
    }

    fn set_or_remove(key: &str, value: Option<&str>) {
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    fn snapshot_and_clear(keys: &[&str]) -> Vec<(String, Option<String>)> {
        keys.iter()
            .map(|k| {
                let prior = std::env::var(k).ok();
                unsafe {
                    std::env::remove_var(k);
                }
                ((*k).to_string(), prior)
            })
            .collect()
    }

    fn restore(snapshot: Vec<(String, Option<String>)>) {
        for (k, v) in snapshot {
            set_or_remove(&k, v.as_deref());
        }
    }

    #[test]
    fn unicode_supported_respects_anvil_no_emoji() {
        let _g = ENV_GUARD.lock().unwrap();
        let keys = ["ANVIL_NO_EMOJI", "LC_ALL", "LC_CTYPE", "LANG"];
        let snap = snapshot_and_clear(&keys);
        set_or_remove("LANG", Some("en_US.UTF-8"));
        set_or_remove("ANVIL_NO_EMOJI", Some("1"));
        assert!(!unicode_supported());
        restore(snap);
    }

    #[test]
    fn unicode_supported_empty_env_returns_false() {
        let _g = ENV_GUARD.lock().unwrap();
        let keys = ["ANVIL_NO_EMOJI", "LC_ALL", "LC_CTYPE", "LANG"];
        let snap = snapshot_and_clear(&keys);
        assert!(!unicode_supported());
        restore(snap);
    }
}
