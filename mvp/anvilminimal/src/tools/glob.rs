use std::path::Path;

use globset::{Glob, GlobSetBuilder};

pub fn run(root: &Path, pattern: &str) -> anyhow::Result<String> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern)?);
    let set = builder.build()?;
    let mut out = Vec::new();
    walk(root, root, &set, &mut out)?;
    out.sort();
    Ok(out.join("\n"))
}

fn walk(
    root: &Path,
    dir: &Path,
    set: &globset::GlobSet,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy() == ".git" {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, set, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if set.is_match(rel) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}
