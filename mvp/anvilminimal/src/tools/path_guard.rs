use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

pub fn validate_workspace_relative(raw: &str) -> anyhow::Result<()> {
    if raw.is_empty() {
        bail!("path is empty");
    }
    if raw.as_bytes().contains(&0) {
        bail!("path contains NUL byte");
    }
    if looks_like_windows_absolute(raw) {
        bail!("absolute path is not allowed");
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        bail!("absolute path is not allowed");
    }
    for component in path.components() {
        match component {
            Component::ParentDir => bail!("path may not contain .."),
            Component::Prefix(_) | Component::RootDir => bail!("absolute path is not allowed"),
            _ => {}
        }
    }
    Ok(())
}

pub fn resolve_existing(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    let candidate = root.join(raw);
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("path does not exist: {raw}"))?;
    ensure_inside(&root, &canonical)?;
    Ok(canonical)
}

pub fn resolve_for_create(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    let candidate = root.join(raw);
    let existing_parent = nearest_existing_parent(&candidate)?;
    let parent_canonical = existing_parent
        .canonicalize()
        .with_context(|| format!("parent is not accessible: {}", existing_parent.display()))?;
    ensure_inside(&root, &parent_canonical)?;
    Ok(candidate)
}

pub fn resolve_optional_existing(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let candidate = root.join(raw);
    if candidate.exists() {
        resolve_existing(root, raw)
    } else {
        resolve_for_create(root, raw)
    }
}

pub fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn nearest_existing_parent(path: &Path) -> anyhow::Result<PathBuf> {
    let mut current = path.parent().unwrap_or(path).to_path_buf();
    loop {
        if current.exists() {
            return Ok(current);
        }
        if !current.pop() {
            bail!("no existing parent for {}", path.display());
        }
    }
}

fn ensure_inside(root: &Path, candidate: &Path) -> anyhow::Result<()> {
    if !candidate.starts_with(root) {
        bail!("path escapes workspace");
    }
    Ok(())
}

fn looks_like_windows_absolute(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    raw.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape_paths() {
        for raw in [
            "/etc/passwd",
            "../secret",
            "a/../../secret",
            "C:\\Users\\x",
            "\\\\server\\share",
        ] {
            assert!(validate_workspace_relative(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn allows_missing_child_under_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = resolve_for_create(dir.path(), "notes/new.md").unwrap();
        assert!(path.ends_with("notes/new.md"));
    }

    #[test]
    fn rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
            assert!(resolve_existing(dir.path(), "out").is_err());
        }
    }
}
