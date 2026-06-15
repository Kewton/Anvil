use std::fs;
use std::path::Path;

use crate::tools::registry::truncate_output;
use crate::util::workspace_paths::WorkspacePolicy;

const LARGE_READ_OUTPUT_THRESHOLD_CHARS: usize = 6_000;
const LARGE_READ_EXCERPT_LINES: usize = 30;
const LARGE_READ_LINE_CHAR_LIMIT: usize = 240;

pub fn run(
    root: &Path,
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
    workspace_policy: WorkspacePolicy,
) -> Result<String, String> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.is_dir() {
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(path).map_err(|err| format!("failed to list {}: {err}", path.display()))?
        {
            let entry =
                entry.map_err(|err| format!("failed to read dir {}: {err}", path.display()))?;
            let child = entry.path();
            if let Ok(relative) = child
                .strip_prefix(&canonical_root)
                .or_else(|_| child.strip_prefix(root))
                && !workspace_policy.allows_model_read_relative_path(relative)
            {
                continue;
            }
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        return Ok(entries.join("\n"));
    }

    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    if start_line.is_none()
        && end_line.is_none()
        && contents.chars().count() > LARGE_READ_OUTPUT_THRESHOLD_CHARS
    {
        return Ok(summarize_large_read(root, path, &contents));
    }

    let lines = contents.lines().collect::<Vec<_>>();
    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or(lines.len()).min(lines.len());
    let slice = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let line_no = index + 1;
            line_no >= start && line_no <= end
        })
        .map(|(index, line)| format!("{:>4}: {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(truncate_output(&slice, 20_000))
}

fn summarize_large_read(root: &Path, path: &Path, contents: &str) -> String {
    let lines = contents.lines().collect::<Vec<_>>();
    let line_count = lines.len();
    let char_count = contents.chars().count();
    let display_path = path
        .strip_prefix(root)
        .ok()
        .unwrap_or(path)
        .to_string_lossy();
    let head = lines
        .iter()
        .take(LARGE_READ_EXCERPT_LINES)
        .enumerate()
        .map(|(index, line)| format_read_excerpt_line(index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let tail_start = line_count.saturating_sub(LARGE_READ_EXCERPT_LINES);
    let tail = lines
        .iter()
        .enumerate()
        .skip(tail_start)
        .map(|(index, line)| format_read_excerpt_line(index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "[anvil] Large Read output summarized to keep the session context small.\n\
path: {display_path}\n\
chars: {char_count}\n\
lines: {line_count}\n\
Call Read with start_line/end_line, or use rg/Grep, for focused inspection.\n\
--- head ({}) ---\n{head}\n\
--- tail ({}) ---\n{tail}",
        LARGE_READ_EXCERPT_LINES.min(line_count),
        line_count.saturating_sub(tail_start)
    )
}

fn format_read_excerpt_line(line_no: usize, line: &str) -> String {
    let truncated = truncate_line_chars(line, LARGE_READ_LINE_CHAR_LIMIT);
    format!("{line_no:>4}: {truncated}")
}

fn truncate_line_chars(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut out = line.chars().take(max_chars).collect::<String>();
    out.push_str(" ...[line truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::util::workspace_paths::WorkspacePolicy;
    use tempfile::tempdir;

    #[test]
    fn small_read_output_passes_through() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("small.txt");
        std::fs::write(&file, "alpha\nbeta\n").unwrap();

        let out = run(temp.path(), &file, None, None, WorkspacePolicy::default()).unwrap();

        assert!(out.contains("   1: alpha"));
        assert!(out.contains("   2: beta"));
        assert!(!out.contains("Large Read output summarized"));
    }

    #[test]
    fn large_full_read_output_is_summarized() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("large.tsx");
        let mut body = String::new();
        for index in 1..=500 {
            body.push_str(&format!("line-{index:03}: {}\n", "x".repeat(40)));
        }
        std::fs::write(&file, body).unwrap();

        let out = run(temp.path(), &file, None, None, WorkspacePolicy::default()).unwrap();

        assert!(out.contains("Large Read output summarized"));
        assert!(out.contains("path: large.tsx"));
        assert!(out.contains("lines: 500"));
        assert!(out.contains("line-001"));
        assert!(out.contains("line-500"));
        assert!(!out.contains("line-250"));
        assert!(
            out.chars().count() < 5_000,
            "summary should stay compact, got {} chars",
            out.chars().count()
        );
    }

    #[test]
    fn explicit_read_range_is_not_summarized() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("large.tsx");
        let mut body = String::new();
        for index in 1..=500 {
            body.push_str(&format!("line-{index:03}: {}\n", "x".repeat(40)));
        }
        std::fs::write(&file, body).unwrap();

        let out = run(
            temp.path(),
            &file,
            Some(240),
            Some(260),
            WorkspacePolicy::default(),
        )
        .unwrap();

        assert!(!out.contains("Large Read output summarized"));
        assert!(out.contains(" 240: line-240"));
        assert!(out.contains(" 260: line-260"));
    }
}
