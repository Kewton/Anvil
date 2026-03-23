//! Diff preview generation for file.write and file.edit approval prompts.
//!
//! Generates plain-text unified diffs (without ANSI colour) so that the
//! presentation layer can apply its own styling.

use super::{ToolInput, resolve_sandbox_path};
use std::path::Path;

/// Maximum number of lines shown for a brand-new file preview.
const NEW_FILE_PREVIEW_MAX_LINES: usize = 20;
/// Maximum characters per line before truncation.
const LINE_MAX_CHARS: usize = 200;
/// When the total number of added + deleted lines exceeds this threshold the
/// diff is truncated with a summary line.
const DIFF_TRUNCATE_THRESHOLD: usize = 50;
/// Files larger than this are skipped for diff generation.
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB
/// Number of leading bytes inspected for binary (NUL byte) detection.
const BINARY_CHECK_BYTES: usize = 8192;
/// `new_content` strings larger than this are skipped for diff generation.
const MAX_NEW_CONTENT_SIZE: usize = 1_048_576; // 1 MB

/// Generate a diff preview string for a tool input, if applicable.
///
/// Returns `Some(diff_text)` for `FileWrite` and `FileEdit` inputs, and
/// `None` for all other tool input variants.
pub fn generate_diff_preview(workspace_root: &Path, tool_input: &ToolInput) -> Option<String> {
    match tool_input {
        ToolInput::FileWrite { path, content } => {
            generate_file_write_diff(workspace_root, path, content)
        }
        ToolInput::FileEdit {
            old_string,
            new_string,
            ..
        } => generate_file_edit_diff(old_string, new_string),
        ToolInput::FileEditAnchor { params, .. } => {
            generate_file_edit_diff(&params.old_content, &params.new_content)
        }
        // MCP tools do not have diff previews
        ToolInput::Mcp { .. } => None,
        // Agent tools do not have diff previews
        ToolInput::AgentExplore { .. } | ToolInput::AgentPlan { .. } => None,
        _ => None,
    }
}

/// Check whether content appears to be binary by looking for NUL bytes in the
/// first [`BINARY_CHECK_BYTES`].
pub fn is_binary_content(content: &[u8]) -> bool {
    let check_len = content.len().min(BINARY_CHECK_BYTES);
    content[..check_len].contains(&0)
}

/// Generate a diff preview for `file.write`.
fn generate_file_write_diff(
    workspace_root: &Path,
    path: &str,
    new_content: &str,
) -> Option<String> {
    // Guard: new_content size
    if new_content.len() > MAX_NEW_CONTENT_SIZE {
        return None;
    }

    // Resolve path within sandbox
    let resolved = match resolve_sandbox_path(workspace_root, path) {
        Ok(p) => p,
        Err(_) => return None,
    };

    // Try to read the existing file
    match std::fs::metadata(&resolved) {
        Ok(meta) => {
            if meta.len() > MAX_FILE_SIZE {
                return Some(format!(
                    "(file too large for diff preview: {} bytes)",
                    meta.len()
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // New file
            return Some(format_new_file_preview(new_content));
        }
        Err(_) => return None,
    }

    // Read existing content
    let existing_bytes = match std::fs::read(&resolved) {
        Ok(bytes) => bytes,
        Err(_) => return None,
    };

    if is_binary_content(&existing_bytes) {
        return Some("(binary file - diff not available)".to_string());
    }

    let existing_content = match String::from_utf8(existing_bytes) {
        Ok(s) => s,
        Err(_) => return None,
    };

    // Generate unified diff
    let diff = similar::TextDiff::from_lines(existing_content.as_str(), new_content);
    let diff_text = diff
        .unified_diff()
        .context_radius(3)
        .header("a (existing)", "b (new)")
        .to_string();

    if diff_text.trim().is_empty() {
        return Some("(no changes)".to_string());
    }

    Some(truncate_diff(&diff_text))
}

/// Generate a diff preview for `file.edit` (old_string -> new_string).
///
/// This does not require any file I/O — it works purely on the provided
/// strings.
pub(crate) fn generate_file_edit_diff(old_string: &str, new_string: &str) -> Option<String> {
    if old_string == new_string {
        return Some("(no changes)".to_string());
    }

    let diff = similar::TextDiff::from_lines(old_string, new_string);
    let diff_text = diff
        .unified_diff()
        .context_radius(3)
        .header("a (old)", "b (new)")
        .to_string();

    if diff_text.trim().is_empty() {
        return Some("(no changes)".to_string());
    }

    Some(truncate_diff(&diff_text))
}

/// Truncate a diff if the number of added/deleted lines exceeds the threshold.
fn truncate_diff(diff_text: &str) -> String {
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for line in diff_text.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }

    if additions + deletions <= DIFF_TRUNCATE_THRESHOLD {
        return diff_text.to_string();
    }

    // Keep the header and first few lines, then add a truncation notice
    let mut result = String::new();
    let mut change_count = 0usize;
    for line in diff_text.lines() {
        let is_change = (line.starts_with('+') && !line.starts_with("+++"))
            || (line.starts_with('-') && !line.starts_with("---"));
        if is_change {
            change_count += 1;
        }
        if change_count > DIFF_TRUNCATE_THRESHOLD {
            break;
        }
        result.push_str(line);
        result.push('\n');
    }
    result.push_str(&format!(
        "... (+{} lines added, -{} lines deleted)\n",
        additions, deletions
    ));
    result
}

/// Format a preview for a brand-new file (showing the first N lines).
fn format_new_file_preview(content: &str) -> String {
    let mut lines = Vec::new();
    lines.push("(new file)".to_string());

    let all_lines: Vec<&str> = content.lines().collect();
    for (i, line) in all_lines.iter().enumerate() {
        if i >= NEW_FILE_PREVIEW_MAX_LINES {
            lines.push(format!(
                "... ({} more lines)",
                all_lines.len() - NEW_FILE_PREVIEW_MAX_LINES
            ));
            break;
        }
        if line.chars().count() > LINE_MAX_CHARS {
            let truncated: String = line.chars().take(LINE_MAX_CHARS).collect();
            lines.push(format!("+{truncated}..."));
        } else {
            lines.push(format!("+{line}"));
        }
    }

    lines.join("\n")
}
