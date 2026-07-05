use std::path::Path;

use super::workspace_policy::{WorkspacePolicy, should_skip_path};

const MAX_READ_BYTES: usize = 24_000;
const MAX_READ_LINES: usize = 400;
const SUMMARY_THRESHOLD_BYTES: usize = 6_000;
const SUMMARY_HEAD_LINES: usize = 120;
const SUMMARY_TAIL_LINES: usize = 80;

pub fn run(
    root: &Path,
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
    policy: WorkspacePolicy,
) -> anyhow::Result<String> {
    if path.is_dir() {
        return list_directory(root, path, policy);
    }
    let content = std::fs::read_to_string(path)?;
    if start_line.is_none() && end_line.is_none() && content.len() > SUMMARY_THRESHOLD_BYTES {
        return Ok(format!(
            "{}\n{}",
            crate::tools::path_guard::relative_display(root, path),
            summarize_large_file(&content)
        ));
    }
    let lines: Vec<&str> = content.lines().collect();
    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or(lines.len()).min(lines.len());
    let selected = if start > end || lines.is_empty() {
        String::new()
    } else {
        let end = end.min(start.saturating_add(MAX_READ_LINES).saturating_sub(1));
        lines[start - 1..end].join("\n")
    };
    let selected = truncate_with_marker(&selected, MAX_READ_BYTES);
    Ok(format!(
        "{}\n{}",
        crate::tools::path_guard::relative_display(root, path),
        selected
    ))
}

fn list_directory(root: &Path, path: &Path, policy: WorkspacePolicy) -> anyhow::Result<String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if should_skip_path(root, &child, policy) {
            continue;
        }
        let mut label = entry.file_name().to_string_lossy().to_string();
        if child.is_dir() {
            label.push('/');
        }
        entries.push(label);
    }
    entries.sort();
    Ok(format!(
        "{}\n{}",
        crate::tools::path_guard::relative_display(root, path),
        entries.join("\n")
    ))
}

fn summarize_large_file(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= SUMMARY_HEAD_LINES + SUMMARY_TAIL_LINES {
        return truncate_with_marker(content, MAX_READ_BYTES);
    }
    let head = lines
        .iter()
        .take(SUMMARY_HEAD_LINES)
        .enumerate()
        .map(|(idx, line)| format!("{}: {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let tail_start = lines.len().saturating_sub(SUMMARY_TAIL_LINES);
    let tail = lines
        .iter()
        .enumerate()
        .skip(tail_start)
        .map(|(idx, line)| format!("{}: {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{head}\n[anvilminimal: file summarized; omitted {} middle lines]\n{tail}",
        lines
            .len()
            .saturating_sub(SUMMARY_HEAD_LINES + SUMMARY_TAIL_LINES)
    )
}

fn truncate_with_marker(value: &str, max_bytes: usize) -> String {
    crate::util::excerpt_with_newline_marker(
        value,
        max_bytes,
        &format!("[anvilminimal: output truncated at {max_bytes} bytes]"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_large_file_is_truncated_with_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        std::fs::write(&path, "x".repeat(MAX_READ_BYTES + 100)).unwrap();
        let output = run(dir.path(), &path, None, None, WorkspacePolicy::NormalTask).unwrap();
        assert!(output.contains("file summarized") || output.contains("output truncated"));
    }

    #[test]
    fn read_truncation_handles_multibyte_boundary() {
        let value = format!("{}{}", "x".repeat(MAX_READ_BYTES - 1), "日本語");
        let output = truncate_with_marker(&value, MAX_READ_BYTES);
        assert!(output.contains("output truncated"));
    }

    #[test]
    fn read_directory_lists_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ok").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let output = run(
            dir.path(),
            dir.path(),
            None,
            None,
            WorkspacePolicy::NormalTask,
        )
        .unwrap();
        assert!(output.contains("a.txt"));
        assert!(output.contains("sub/"));
    }

    #[test]
    fn explicit_range_is_not_large_file_summarized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let content = (0..500)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, content).unwrap();
        let output = run(
            dir.path(),
            &path,
            Some(10),
            Some(12),
            WorkspacePolicy::NormalTask,
        )
        .unwrap();
        assert!(output.contains("line 9"));
        assert!(!output.contains("file summarized"));
    }
}
