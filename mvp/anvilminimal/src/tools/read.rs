use std::path::Path;

const MAX_READ_BYTES: usize = 24_000;
const MAX_READ_LINES: usize = 400;

pub fn run(
    root: &Path,
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(path)?;
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

fn truncate_with_marker(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[anvilminimal: output truncated at {} bytes]",
        &value[..end],
        max_bytes
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
        let output = run(dir.path(), &path, None, None).unwrap();
        assert!(output.contains("output truncated"));
    }
}
