use globset::{GlobBuilder, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::Path;

use crate::util::workspace_paths::WorkspacePolicy;

pub fn run(
    root: &Path,
    pattern: &str,
    workspace_policy: WorkspacePolicy,
) -> Result<String, String> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|err| format!("invalid glob pattern: {err}"))?;
    let mut set = GlobSetBuilder::new();
    set.add(glob);
    let set = set
        .build()
        .map_err(|err| format!("failed to build glob set: {err}"))?;

    let walk_root = root.to_path_buf();
    let filter_root = walk_root.clone();
    let mut matches = Vec::new();
    for entry in WalkBuilder::new(&walk_root)
        .hidden(false)
        .filter_entry(move |entry| {
            let path = entry.path();
            if path == filter_root {
                return true;
            }
            path.strip_prefix(&filter_root)
                .map(|relative| workspace_policy.allows_model_read_relative_path(relative))
                .unwrap_or(true)
        })
        .build()
    {
        let entry = entry.map_err(|err| format!("walk error: {err}"))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let relative = path.strip_prefix(&walk_root).unwrap_or(path);
        if !workspace_policy.allows_model_read_relative_path(relative) {
            continue;
        }
        if set.is_match(relative) {
            matches.push(relative.display().to_string());
        }
    }
    matches.sort();
    Ok(matches.join("\n"))
}
