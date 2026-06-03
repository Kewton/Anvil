//! Focused-edit recovery helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts the guidance-note builders, page-component anchor extractors, and
//! conversation history shapers used by the focused-edit recovery flow:
//! `focused_edit_guidance_note_for_policy`, `focused_edit_compact_anchor_note`,
//! `focused_edit_first_slice_note`, `focused_edit_second_slice_note`,
//! `focused_edit_exact_recovery_anchor`, `focused_edit_compact_recovery_anchor`,
//! `focused_edit_history`, `focused_edit_exact_anchor_history`, plus the
//! private numbered-read parsing helpers (`extract_page_copy_block_*`,
//! `extract_page_intro_*`, `extract_compact_edit_anchor_*`,
//! `strip_read_line_number_prefix`, `format_numbered_read_block`,
//! `latest_page_copy_block_from_read`, `latest_page_intro_copy_line_from_read`,
//! `focused_edit_first_slice_uses_exact_anchor`, `focused_edit_minimal_history`,
//! `is_page_component_target`, `focused_edit_guidance_note`).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use crate::agent::recovery;
use crate::ollama::xml_fallback::ToolCall;
use crate::session::store::ConversationMessage;

use super::tool_history::latest_read_exchange_for_target;
use super::tool_policy::{EffectiveToolPolicy, EffectiveToolPolicyReason};

pub(super) fn is_page_component_target(relative: &str) -> bool {
    matches!(relative, "app/page.tsx" | "src/app/page.tsx")
        || relative.ends_with("/app/page.tsx")
        || relative.ends_with("/src/app/page.tsx")
}

pub(super) fn focused_edit_guidance_note(
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> String {
    // PR #930 review (High-2): mask + cap the path embedded into this recovery prompt.
    let path = super::task_contract::mask_and_cap_recovery_field(
        &target
            .strip_prefix(work_root)
            .unwrap_or(target)
            .to_string_lossy()
            .replace('\\', "/"),
    );
    if target_already_read {
        format!(
            "[Focused Edit Recovery] The target file {path} has already been read. The only available tool for this turn is Edit. Do not call Read again. Use exactly one compact Edit on that file now. Replace only one contiguous block from the last Read. Do not attempt a full-file rewrite, multi-file change, scaffold command, or dev-server command."
        )
    } else if !target.is_file() {
        format!(
            "[Focused Edit Recovery] The target file {path} does not exist yet. The only available tool for this turn is Write on that exact path. Do not call Read, Bash, Glob, or Grep; create the missing artifact directly and keep the body focused on the requested role."
        )
    } else {
        format!(
            "[Focused Edit Recovery] Keep this turn minimal. If you need context, do one Read on {path} first; otherwise use exactly one compact Edit. Do not attempt a full-file rewrite, multi-file change, scaffold command, or dev-server command. Replace only one contiguous block from the last Read and move the implementation forward with the first concrete slice."
        )
    }
}

pub(super) fn focused_edit_guidance_note_for_policy(
    policy: &EffectiveToolPolicy,
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> String {
    if policy.reason() == EffectiveToolPolicyReason::VerifierRepair {
        let path = super::task_contract::mask_and_cap_recovery_field(
            &target
                .strip_prefix(work_root)
                .unwrap_or(target)
                .to_string_lossy()
                .replace('\\', "/"),
        );
        match policy.allowed_tool_names_for_prompt() {
            Some(["Read"]) => {
                return format!(
                    "[Verifier Repair] The verifier failure target is {path}, but the current file contents have not been read since the latest target edit. The only available tool for this turn is Read. Emit exactly one Read on that file now. Do not call Edit, Bash, Glob, Grep, or answer in prose."
                );
            }
            Some(["Edit"]) => {
                return format!(
                    "[Verifier Repair] The verifier failure target is {path} and its current contents have been read. The only available tool for this turn is Edit. Emit exactly one compact Edit on that file now. Do not call Read again, Bash, or answer in prose."
                );
            }
            Some(["Write"]) => {
                return format!(
                    "[Verifier Repair] The verifier failure target {path} is missing. The only available tool for this turn is Write on that exact path. Do not call Read, Bash, Glob, Grep, or answer in prose."
                );
            }
            _ => {}
        }
    }
    focused_edit_guidance_note(target, work_root, target_already_read)
}

pub(super) fn focused_edit_compact_anchor_note(target: &Path, work_root: &Path) -> String {
    let path = super::task_contract::mask_and_cap_recovery_field(
        &target
            .strip_prefix(work_root)
            .unwrap_or(target)
            .to_string_lossy()
            .replace('\\', "/"),
    );
    format!(
        "[Focused Edit Recovery / Compact Anchor] The Read result for {path} is intentionally only a tiny exact anchor from the real file, not the whole file. Use that anchor only for `old_string`. Keep `new_string` similarly small: at most 3 lines and under 240 characters. Do not insert imports, hooks, component definitions, or full-file content. If the anchor is CTA or placeholder text, replace only that text with a short task-specific label or copy."
    )
}

pub(super) fn focused_edit_first_slice_note(
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

pub(super) fn focused_edit_first_slice_uses_exact_anchor(
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

pub(super) fn focused_edit_exact_recovery_anchor(
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

pub(super) fn focused_edit_second_slice_note(
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

pub(super) fn latest_page_copy_block_from_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_page_copy_block_from_numbered_read(&tool.content)
}

pub(super) fn latest_page_intro_copy_line_from_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_page_intro_copy_line_from_numbered_read(&tool.content)
        .or_else(|| extract_page_intro_paragraph_from_numbered_read(&tool.content))
}

pub(super) fn focused_edit_compact_recovery_anchor(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    let (_, tool) = latest_read_exchange_for_target(messages, target, work_root)?;
    extract_compact_edit_anchor_from_numbered_read(&tool.content)
}

pub(super) fn extract_compact_edit_anchor_from_numbered_read(contents: &str) -> Option<String> {
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

pub(super) fn extract_page_copy_block_from_numbered_read(contents: &str) -> Option<String> {
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

pub(super) fn extract_page_intro_paragraph_from_numbered_read(contents: &str) -> Option<String> {
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

pub(super) fn extract_page_intro_copy_line_from_numbered_read(contents: &str) -> Option<String> {
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

pub(super) fn strip_read_line_number_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some((prefix, rest)) = trimmed.split_once(": ")
        && !prefix.is_empty()
        && prefix.chars().all(|ch| ch.is_ascii_digit())
    {
        return rest.to_string();
    }
    line.to_string()
}

pub(super) fn focused_edit_history(
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

pub(super) fn focused_edit_minimal_history(
    messages: &[ConversationMessage],
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
    filtered
}

pub(super) fn focused_edit_exact_anchor_history(
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

pub(super) fn format_numbered_read_block(contents: &str) -> String {
    contents
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{:>4}: {line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}
