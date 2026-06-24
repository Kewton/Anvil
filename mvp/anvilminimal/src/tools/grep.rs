use std::path::Path;

use globset::{Glob, GlobSetBuilder};
use regex::RegexBuilder;

pub fn run(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    case_sensitive: bool,
) -> anyhow::Result<String> {
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()?;
    let globset = if let Some(glob) = glob {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new(glob)?);
        Some(builder.build()?)
    } else {
        None
    };
    let mut hits = Vec::new();
    walk(root, root, &regex, globset.as_ref(), &mut hits)?;
    hits.sort();
    Ok(hits.join("\n"))
}

fn walk(
    root: &Path,
    dir: &Path,
    regex: &regex::Regex,
    globset: Option<&globset::GlobSet>,
    hits: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy() == ".git" {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, regex, globset, hits)?;
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if globset.is_some_and(|set| !set.is_match(rel)) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                hits.push(format!(
                    "{}:{}:{}",
                    rel.to_string_lossy().replace('\\', "/"),
                    idx + 1,
                    line
                ));
            }
        }
    }
    Ok(())
}
