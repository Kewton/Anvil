use std::path::Path;

use anyhow::Context;
use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;

use super::workspace_policy::{WorkspacePolicy, should_skip_path};

pub fn run(root: &Path, pattern: &str, policy: WorkspacePolicy) -> anyhow::Result<String> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern).with_context(|| format!("invalid glob pattern: {pattern}"))?);
    let set = builder.build().context("invalid glob pattern")?;
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .parents(true)
        .build();
    for entry in walker {
        let entry = entry?;
        let path = entry.path();
        if path == root || should_skip_path(root, path, policy) || path.is_dir() {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        if set.is_match(rel) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    out.sort();
    Ok(out.join("\n"))
}
