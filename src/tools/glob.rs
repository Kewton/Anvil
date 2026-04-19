use globset::{GlobBuilder, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::Path;

pub fn run(root: &Path, pattern: &str) -> Result<String, String> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|err| format!("invalid glob pattern: {err}"))?;
    let mut set = GlobSetBuilder::new();
    set.add(glob);
    let set = set
        .build()
        .map_err(|err| format!("failed to build glob set: {err}"))?;

    let mut matches = Vec::new();
    for entry in WalkBuilder::new(root).hidden(false).build() {
        let entry = entry.map_err(|err| format!("walk error: {err}"))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        if set.is_match(relative) {
            matches.push(relative.display().to_string());
        }
    }
    matches.sort();
    Ok(matches.join("\n"))
}
