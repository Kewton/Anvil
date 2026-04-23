use super::interrupt::{InterruptEnv, InterruptFlag, InterruptMonitor};
use super::spinner::{Spinner, SpinnerStopSignal};
use super::summary::{ExitReason, LoopResult, LoopStats};
use super::*;
use crate::agent::orchestration::{RepoVerification, capture_repo_snapshot, verify_repo_progress};
use crate::logging::log_llm_event;
use crate::modes::plan_act::{PlanStage, TaskProfile};
use crate::ollama::xml_fallback::normalize_tool_call_arguments;
use crate::tools::registry::{ToolSpec, resolve_plan_mode_write_target};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// Maximum number of characters of tool-call arguments retained in trace logs.
const LOG_ARGS_MAX_CHARS: usize = 200;
const PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD: usize = 2;
const USER_INTERRUPT_ERROR: &str = "__anvil_user_interrupt__";
const QWEN35_NON_NATIVE_HARD_TIMEOUT_SECS: u64 = 90;

fn is_qwen35_family(model: &str) -> bool {
    model
        .trim()
        .to_ascii_lowercase()
        .starts_with("qwen3.5:")
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

fn fallback_plan_request_label(task: &str) -> String {
    let lower = task.to_ascii_lowercase();
    if lower.contains("space invader") || task.contains("スペースインベーダー") {
        "a stylish and highly playable Space Invaders game".to_string()
    } else if lower.contains("next.js") {
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
        "# Plan\n\n## Goal\n- Build {request_label} as a {platform_label} inside `{worktree_name}`.\n- Ensure the result runs locally on {port} and feels intentionally polished rather than placeholder-quality.\n\n## Constraints\n- Keep all work inside the current repository root and use repository-relative paths.\n- If the repository is empty, scaffold only the minimum project structure needed before implementing the requested feature.\n- Keep the implementation incremental: ship a working first slice before adding effects or polish.\n- Preserve a path to local verification so the final result can be launched and checked end-to-end.\n\n## Deliverables\n- A runnable {platform_label} that fulfills the user request.\n- The core interactive flow, supporting UI/state, and the minimum assets or styles needed for a polished first release.\n- Verification notes covering dependency install, local startup, and feature checks.\n\n## Acceptance Criteria\n- Dependency installation succeeds and the project can be started locally on {port}.\n- The default entry route renders the requested experience instead of a placeholder page.\n- The first playable slice is complete enough to demonstrate the core user interaction from start to finish.\n- The implementation includes a clear restart or recovery path when the primary interaction ends in failure or completion.\n\n## Quality Bar\n- The first minute of use should feel deliberate: cohesive visuals, readable HUD/text, and responsive controls.\n- Motion, feedback, and state updates should feel consistent rather than jarring or random.\n- The experience should be understandable without reading source code, and the main interaction should be enjoyable on the first try.\n- The code structure should leave obvious extension points for later tuning, polish, and debugging.\n\n## Execution Plan\n1. First slice: confirm or scaffold the base app, wire the main screen, implement the core interaction loop, and make the requested experience playable from start to finish.\n2. Next phases: improve presentation, tune difficulty/interaction balance, add richer feedback, and harden any supporting UI or state transitions.\n3. Review checkpoint: stop once the first playable slice runs locally on {port} and passes the core verification steps.\n\n## Verification Plan\n- Install dependencies and confirm the app boots locally on {port}.\n- Exercise the main interaction loop end-to-end, including the expected success and failure states.\n- Check that layout, controls, and status/UI updates remain readable at common desktop widths and degrade reasonably on smaller screens.\n\n## Risks / Fallbacks\n- If framework scaffolding is missing, create the smallest viable project structure first and defer non-essential polish.\n- If the requested polish threatens delivery, keep the core loop intact and add lighter-weight effects before heavier assets or integrations.\n- If performance or complexity becomes unstable, simplify update frequency and visual effects before cutting the core user interaction.\n\n<!-- runtime fallback plan: generated after repeated planning model timeouts; focus on {execution_focus}. -->\n"
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
    let mut deleted_changed = 0usize;

    for verif in accumulated.iter().chain(std::iter::once(&final_verif)) {
        for f in &verif.changed_files {
            all_changed.insert(f.clone());
        }
        impl_changed += verif.implementation_files_changed;
        test_changed += verif.test_files_changed;
        setup_changed += verif.setup_files_changed;
        deleted_changed += verif.deleted_files_changed;
    }

    let total_changed = impl_changed + test_changed + setup_changed + deleted_changed;
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
            self.maybe_compact_session(DEFAULT_KEEP_TAIL);
        }
        let _ = self.refresh_plan_stage();

        let action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
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

            let reply = match self.request_assistant_reply_with_retry(stream_output, &interrupt_flag)
            {
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

            let prepared_tool_calls = reply
                .tool_calls
                .into_iter()
                .map(|tool_call| self.prepare_tool_call(tool_call))
                .collect::<Vec<_>>();

            if !prepared_tool_calls.is_empty() {
                let mut plan_file_edit_calls_this_turn = 0usize;
                let mut plan_exploration_calls_this_turn = 0usize;
                let mut plan_ready_after_tool = false;
                let mut bash_only_tool_turn = true;
                let current_plan_stage = self.session.mode_state.plan_stage;
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
                            .and_then(|key| {
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
                                Some(*count >= PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD)
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
                        let write_retry_label = if self.session.mode_state.mode == ExecutionMode::Plan
                            && is_plan_file_tool_call(
                                &tool_name,
                                &tool_call.arguments,
                                &self.work_root,
                                self.session.mode_state.active_plan_path.as_deref(),
                            )
                        {
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
                            .is_some_and(|contents| self.plan_is_approval_ready_with_fallback(&contents))
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
                        let _ = self.refresh_plan_stage();
                        plan_progress_retries = 0;
                        plan_exploration_only_turns = 0;
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
                        if plan_exploration_only_turns >= 2 {
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
                    if plan_progress_retries >= 4 {
                        exit_reason = ExitReason::PlanIncomplete;
                        error_text = exit_reason.default_error_text().to_string();
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
                            "The model answered with prose only. Asked it to make the repository edits with tools.",
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
                    if self.plan_is_substantive_with_fallback(&plan_contents) {
                        final_prose = final_reply;
                        exit_reason = ExitReason::Done;
                        break 'outer;
                    }
                    let current_stage = lifecycle::current_plan_stage(&plan_contents);
                    let next_sections = lifecycle::plan_next_stage_sections(&plan_contents);
                    plan_progress_retries += 1;
                    if plan_progress_retries >= 4 {
                        exit_reason = ExitReason::PlanIncomplete;
                        error_text = exit_reason.default_error_text().to_string();
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

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn == 0
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
                        "The turn finished without repository edits. Asked the model to continue implementing changes.",
                        self.footer.current_cols(),
                    ),
                    true,
                );
                self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
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
                    if plan_progress_retries >= 4 {
                        exit_reason = ExitReason::PlanIncomplete;
                        error_text = exit_reason.default_error_text().to_string();
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
                    if lifecycle::is_tool_call_format_error(&err)
                        && tool_call_format_retries_remaining > 0
                    {
                        tool_call_format_retry_count += 1;
                        tool_call_format_retries_remaining -= 1;
                        self.push_system_note(recovery::tool_call_format_recovery_note(
                            &err,
                            tool_call_format_retry_count,
                        ));
                        continue;
                    }
                    if lifecycle::is_transport_error(&err) && extra_transport_retries > 0 {
                        if self.maybe_fallback_plan_model_after_timeout(&err) {
                            continue;
                        }
                        if let Some(reply) = self.maybe_materialize_plan_after_timeout(&err)? {
                            return Ok(reply);
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
        let use_streaming_transport = should_use_streaming_transport(
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
                        if stream_output
                            && let Some(sig) = &stop_signal
                        {
                            sig.trigger();
                        }
                        if stream_output {
                            write_stdout_rendered("assistant> ", false);
                        }
                        first_chunk = false;
                    }
                    if let Some(r) = renderer.as_mut() {
                        let out = r.push_chunk(chunk);
                        if !out.is_empty() {
                            if stream_output {
                                write_stdout_rendered(&out, false);
                            }
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
                if !tail.is_empty() {
                    if stream_output {
                        write_stdout_rendered(&tail, false);
                    }
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
            )
        }
    }

    fn request_assistant_reply_non_streaming(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        let client = self.client.clone();
        let model = model.to_string();
        let tool_specs = self.effective_tool_specs();
        let owned_messages = messages.to_vec();
        let timeout = Duration::from_secs(non_streaming_assistant_reply_timeout_secs(
            &model,
            native_tools_enabled,
            client.timeout_secs(),
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

    fn maybe_fallback_plan_model_after_timeout(&mut self, err: &str) -> bool {
        let Some(sidecar) = self.models.sidecar.as_ref().filter(|model| !model.trim().is_empty())
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
        let Some(plan_path) = self.session.mode_state.active_plan_path.clone() else {
            return Ok(None);
        };

        let current_contents = self.current_plan_contents()?.unwrap_or_default();
        if lifecycle::plan_is_substantive(&current_contents) {
            return Ok(None);
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
        log_llm_event(
            "agent.plan.timeout_fallback_materialized",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "plan_path": plan_path.display().to_string(),
                "task_profile": self.session.mode_state.task_profile.as_str(),
                "model_override": self.plan_model_override,
            }),
        );
        Ok(Some(AssistantReply {
            content: "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                .to_string(),
            tool_calls: Vec::new(),
        }))
    }

    fn build_request_messages(
        &mut self,
        protocol: prompting::ToolProtocol,
    ) -> Vec<ConversationMessage> {
        let mut messages = Vec::new();
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
        if let Some(memory_message) = self.working_memory_message() {
            messages.push(memory_message);
        }
        if let Some(repo_context_message) = self.repo_context_message() {
            messages.push(repo_context_message);
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
        messages.extend(prompting::runtime_context_messages(
            &self.config.cwd,
            &self.work_root,
            protocol,
        ));
        messages.extend(self.session.messages.clone());
        messages
    }

    fn effective_tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs = self.tool_registry.specs().to_vec();
        if self.forced_small_edit_recovery_target().is_some() {
            specs.retain(|spec| matches!(spec.function.name.as_str(), "Read" | "Edit"));
        }
        specs
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
        if has_successful_repo_edit(&self.session.messages) {
            return None;
        }
        let path = last_read_tool_path(&self.session.messages)?;
        let candidate = resolve_user_path(&self.work_root, &path).ok()?;
        candidate.is_file().then_some(candidate)
    }

    fn execute_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> String {
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
        self.refresh_working_memory();
        self.session
            .working_memory
            .format_for_prompt()
            .map(ConversationMessage::system)
    }

    fn repo_context_message(&mut self) -> Option<ConversationMessage> {
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
        PlanExplorationKey, assistant_model_for_mode, deterministic_timeout_fallback_plan,
        non_streaming_assistant_reply_timeout_secs, normalize_exploration_path,
        normalize_plan_exploration_key, should_fallback_plan_model_after_timeout,
        should_materialize_plan_after_timeout, should_use_streaming_transport,
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
    fn native_tool_models_still_use_streaming_transport() {
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
            non_streaming_assistant_reply_timeout_secs("qwen3.6:27b-coding-nvfp4", true, 120),
            120
        );
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
    fn repeated_plan_timeout_after_sidecar_override_materializes_fallback_plan() {
        assert!(should_materialize_plan_after_timeout(
            ExecutionMode::Plan,
            Some("qwen3.5:9b"),
            "assistant reply timed out after 90s",
        ));
        assert!(!should_materialize_plan_after_timeout(
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
    fn deterministic_timeout_fallback_plan_mentions_requested_port() {
        let temp = tempdir().unwrap();
        let plan = deterministic_timeout_fallback_plan(
            "スペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            TaskProfile::Coding,
            temp.path(),
        );
        assert!(plan.contains("3011"));
        assert!(plan.contains("## Quality Bar"));
        assert!(plan.contains("## Execution Plan"));
        assert!(plan.contains("runtime fallback plan"));
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

fn progress_path_display(raw_path: &str, work_root: &Path, plan_path: Option<&Path>, max_chars: usize) -> String {
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
    native_tools_enabled: bool,
    stream_output: bool,
    stdin_is_terminal: bool,
) -> bool {
    let wants_streaming = stream_output || stdin_is_terminal;
    if !wants_streaming {
        return false;
    }

    if !native_tools_enabled && is_qwen35_family(model) {
        return false;
    }

    true
}

fn non_streaming_assistant_reply_timeout_secs(
    model: &str,
    native_tools_enabled: bool,
    default_timeout_secs: u64,
) -> u64 {
    if !native_tools_enabled && is_qwen35_family(model) {
        return QWEN35_NON_NATIVE_HARD_TIMEOUT_SECS;
    }
    default_timeout_secs
}

fn should_materialize_plan_after_timeout(
    mode: ExecutionMode,
    plan_model_override: Option<&str>,
    err: &str,
) -> bool {
    mode == ExecutionMode::Plan
        && plan_model_override.is_some()
        && err.to_ascii_lowercase().contains("timed out")
}

fn assistant_model_for_mode(
    mode: ExecutionMode,
    main_model: &str,
    plan_model_override: Option<&str>,
) -> String {
    if mode == ExecutionMode::Plan && let Some(model) = plan_model_override {
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
    messages
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

fn has_successful_repo_edit(messages: &[ConversationMessage]) -> bool {
    messages.iter().any(|message| {
        message.role == "tool"
            && matches!(message.name.as_deref(), Some("Write" | "Edit"))
            && !message.content.trim_start().starts_with("Error:")
    })
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

fn plan_sections_with_content(contents: &str) -> Vec<&'static str> {
    [
        "Goal",
        "Constraints",
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
                    "1." | "2." | "3." | "1. First slice:" | "2. Next phases:" | "3. Review checkpoint:"
                )
            {
                continue;
            }
            let cleaned = line.trim_start_matches("- ").trim();
            return Some(format!("{section}: {}", truncate(&sanitize_for_progress(cleaned), 72)));
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
    } else if sections
        .iter()
        .any(|section| matches!(*section, "Execution Plan" | "Verification Plan" | "Risks / Fallbacks"))
    {
        "Finalize execution plan"
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
            PlanStage::Stage2 => "Define quality bar",
            PlanStage::Stage3 => "Finalize execution plan",
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
    let phase = plan_phase_from_sections(&focus_sections, current_stage, approval_ready)
        .to_string();
    let signature = format!(
        "{}|{}|{}",
        phase,
        action,
        note.clone().unwrap_or_default()
    );
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
                    (!text_preview(content, 72).is_empty()).then(|| {
                        format!("Preview: {}", text_preview(content, 72))
                    }),
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
                    (!text_preview(new_text, 72).is_empty()).then(|| {
                        format!("Preview: {}", text_preview(new_text, 72))
                    }),
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
                "1." | "2." | "3." | "1. First slice:" | "2. Next phases:" | "3. Review checkpoint:"
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
        "Risks/Fallbacks" => "Risks / Fallbacks",
        "実行計画" | "実装計画" | "実装フェーズ" => "Execution Plan",
        "検証計画" => "Verification Plan",
        "リスク/フォールバック" | "リスク・フォールバック" | "リスク / フォールバック" => {
            "Risks / Fallbacks"
        }
        other => other,
    }
}

fn read_line_suffix(arguments: &serde_json::Value) -> String {
    let start = arguments.get("start_line").and_then(serde_json::Value::as_u64);
    let end = arguments.get("end_line").and_then(serde_json::Value::as_u64);
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
    ((!preview.is_empty()).then_some(preview), Some(format!("{}B", metadata.len())))
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
    format!("{prefix}{}", truncate(&sanitize_for_progress(value), budget))
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
        format!("[iter {iter_human}/{max_iterations}]  {painted}  {}", display.action)
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
    let arg_budget = progress_available_width(
        cols,
        headline,
        iter_human,
        max_iterations,
        use_unicode,
    );
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
        format!("[iter {iter_human}/{max_iterations}]  {painted}  {}", display.action)
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

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
}

#[cfg(test)]
mod progress_tests {
    use super::{
        format_blocked_progress_line, format_progress_line, has_successful_repo_edit,
        is_utf8_locale, last_read_tool_path, progress_available_width,
        recent_truncated_tool_call_attempt, sanitize_for_progress, tool_color, tool_display,
        tool_emoji, unicode_supported,
    };
    use crate::modes::plan_act::PlanStage;
    use crate::ollama::xml_fallback::ToolCall;
    use crate::safety::path_guard::resolve_user_path;
    use crate::session::store::ConversationMessage;
    use serde_json::json;
    use std::path::PathBuf;
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
        assert!(display.note.as_deref().is_some_and(|note| note.contains("hello")));
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
        let resolved = resolve_user_path(&work_root, &last_read_tool_path(&messages).unwrap()).unwrap();
        assert!(resolved.ends_with("app/page.tsx"), "got: {}", resolved.display());
        assert_eq!(recent_truncated_tool_call_attempt(&messages), 1);
        assert!(!has_successful_repo_edit(&messages));
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
            "Bash", &args, 1, 12, &work_root, false, false, None, None, PlanStage::Stage1, None, None
        );
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_color_prefix_invariant() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash", &args, 1, 12, &work_root, true, false, None, None, PlanStage::Stage1, None, None
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
            "Bash", &args, 1, 12, &work_root, true, true, None, None, PlanStage::Stage1, None, None
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
            "Bash", &args, 1, 12, &work_root, false, true, None, None, PlanStage::Stage1, None, None
        );
        assert!(line.contains('⚡'));
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_unicode_off_no_emoji() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line(
            "Bash", &args, 1, 12, &work_root, true, false, None, None, PlanStage::Stage1, None, None
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
