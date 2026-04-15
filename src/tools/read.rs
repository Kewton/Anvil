use std::fs;
use std::path::Path;

use crate::tools::registry::truncate_output;

pub fn run(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|err| format!("failed to list {}: {err}", path.display()))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().to_string()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read dir {}: {err}", path.display()))?;
        entries.sort();
        return Ok(entries.join("\n"));
    }

    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
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
