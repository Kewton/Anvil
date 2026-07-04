//! Deterministic fallback-plan generator extracted from `turn.rs`
//! (parent #680). Used by the planning-model timeout recovery path
//! in `scaffold_pipeline.rs::maybe_materialize_plan_after_timeout`
//! when the main planning model fails to produce a usable plan.
//!
//! Hosts:
//!
//! * `deterministic_timeout_fallback_plan(task, profile, work_root)
//!   -> String` — renders a markdown plan with `## Goal` / `## Constraints`
//!   / `## First Action` / `## Verification` sections, parameterised on
//!   detected port / framework / worktree name.
//! * `fallback_plan_request_label(task) -> String` — picks
//!   "the requested Next.js app" vs. "the requested deliverable".
//! * `fallback_plan_platform_label(task) -> &'static str` — picks
//!   "Next.js app" vs. "local app".
//! * `extract_requested_port(task) -> Option<String>` — shared contextual
//!   port extractor.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use crate::agent::text_tokens;
use crate::modes::plan_act::TaskProfile;

pub(super) fn extract_requested_port(task: &str) -> Option<String> {
    text_tokens::requested_port(task).map(|port| port.to_string())
}

pub(super) fn fallback_plan_request_label(task: &str) -> String {
    let lower = task.to_ascii_lowercase();
    if lower.contains("next.js") {
        "the requested Next.js app".to_string()
    } else {
        "the requested deliverable".to_string()
    }
}

pub(super) fn fallback_plan_platform_label(task: &str) -> &'static str {
    if task.to_ascii_lowercase().contains("next.js") {
        "Next.js app"
    } else {
        "local app"
    }
}

pub(super) fn deterministic_timeout_fallback_plan(
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
