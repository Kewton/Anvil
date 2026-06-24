use std::path::Path;

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
        lines[start - 1..end].join("\n")
    };
    Ok(format!(
        "{}\n{}",
        crate::tools::path_guard::relative_display(root, path),
        selected
    ))
}
