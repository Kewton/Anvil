use std::path::Path;

use globset::{Glob, GlobSetBuilder};

use super::workspace_policy::{WorkspacePolicy, should_skip_path};

pub fn run(root: &Path, pattern: &str, policy: WorkspacePolicy) -> anyhow::Result<String> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern)?);
    let set = builder.build()?;
    let mut out = Vec::new();
    walk(root, root, &set, policy, &mut out)?;
    out.sort();
    Ok(out.join("\n"))
}

fn walk(
    root: &Path,
    dir: &Path,
    set: &globset::GlobSet,
    policy: WorkspacePolicy,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if should_skip_path(root, &path, policy) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, set, policy, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if set.is_match(rel) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}
