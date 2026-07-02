use ignore::WalkBuilder;
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::tools::registry::truncate_output;
use crate::util::workspace_paths::WorkspacePolicy;

pub fn run(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    case_sensitive: bool,
    workspace_policy: WorkspacePolicy,
) -> Result<String, String> {
    let regex = if case_sensitive {
        Regex::new(pattern).ok()
    } else {
        Regex::new(&format!("(?i){pattern}")).ok()
    };
    let needle = if regex.is_none() {
        if case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_ascii_lowercase()
        }
    } else {
        String::new()
    };

    let matcher = glob
        .map(globset::Glob::new)
        .transpose()
        .map_err(|err| format!("invalid glob filter: {err}"))?;
    let matcher = matcher.map(|glob| glob.compile_matcher());

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
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let relative = path.strip_prefix(&walk_root).unwrap_or(path);
        if !workspace_policy.allows_model_read_relative_path(relative) {
            continue;
        }
        if let Some(matcher) = &matcher
            && !matcher.is_match(relative)
        {
            continue;
        }
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };

        for (index, line) in contents.lines().enumerate() {
            let haystack = if case_sensitive {
                line.to_string()
            } else {
                line.to_ascii_lowercase()
            };
            let is_match = regex
                .as_ref()
                .map(|regex| regex.is_match(line))
                .unwrap_or_else(|| haystack.contains(&needle));
            if is_match {
                matches.push(format!("{}:{}:{}", relative.display(), index + 1, line));
            }
            if matches.len() >= 200 {
                return Ok(truncate_output(&matches.join("\n"), 20_000));
            }
        }
    }

    Ok(truncate_output(&matches.join("\n"), 20_000))
}
