use ignore::WalkBuilder;
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::tools::registry::truncate_output;

pub fn run(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    case_sensitive: bool,
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

    let mut matches = Vec::new();
    for entry in WalkBuilder::new(root).hidden(false).build() {
        let entry = entry.map_err(|err| format!("walk error: {err}"))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
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
