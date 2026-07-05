use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

const EXPECTED_PATH_FORM: &str = "use workspace-relative paths";

pub fn validate_workspace_relative(raw: &str) -> anyhow::Result<()> {
    if raw.is_empty() {
        bail!("path is empty");
    }
    if raw.as_bytes().contains(&0) {
        bail!("path contains NUL byte; {EXPECTED_PATH_FORM}");
    }
    if looks_like_windows_absolute(raw) {
        bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}");
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}");
    }
    for component in path.components() {
        match component {
            Component::ParentDir => bail!("path may not contain ..; {EXPECTED_PATH_FORM}"),
            Component::Prefix(_) | Component::RootDir => {
                bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}")
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn normalize_absolute_workspace_path(root: &Path, raw: &str) -> anyhow::Result<Option<String>> {
    if raw.as_bytes().contains(&0) {
        bail!("path contains NUL byte; {EXPECTED_PATH_FORM}");
    }
    if looks_like_windows_absolute(raw) {
        bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}");
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Ok(None);
    }
    reject_parent_components(path)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    let canonical = canonicalize_with_missing_leaf(path).with_context(|| {
        format!("path escapes workspace or parent is not accessible; {EXPECTED_PATH_FORM}")
    })?;
    ensure_inside(&root, &canonical)?;
    let relative = canonical
        .strip_prefix(&root)
        .context("path escapes workspace")?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    Ok(Some(if relative.is_empty() {
        ".".to_string()
    } else {
        relative
    }))
}

pub fn normalize_absolute_workspace_glob(root: &Path, raw: &str) -> anyhow::Result<Option<String>> {
    if raw.as_bytes().contains(&0) {
        bail!("path contains NUL byte; {EXPECTED_PATH_FORM}");
    }
    if looks_like_windows_absolute(raw) {
        bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}");
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Ok(None);
    }
    reject_parent_components(path)?;
    let Some((base, suffix)) = split_absolute_glob_base(raw) else {
        return normalize_absolute_workspace_path(root, raw);
    };
    let Some(base_relative) = normalize_absolute_workspace_path(root, base)? else {
        return Ok(None);
    };
    let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
    if base_relative == "." {
        Ok(Some(suffix.to_string()))
    } else if suffix.is_empty() {
        Ok(Some(base_relative))
    } else {
        Ok(Some(format!("{base_relative}/{suffix}")))
    }
}

pub fn resolve_existing(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    let normalized = strip_redundant_root_prefix(&root, raw);
    let candidate = root.join(&normalized);
    let canonical = candidate
        .canonicalize()
        .with_context(|| missing_path_message(&root, raw))?;
    ensure_inside(&root, &canonical)?;
    Ok(canonical)
}

pub fn resolve_for_create(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    let normalized = strip_redundant_root_prefix(&root, raw);
    let candidate = root.join(normalized);
    let existing_parent = nearest_existing_parent(&candidate)?;
    let parent_canonical = existing_parent
        .canonicalize()
        .with_context(|| format!("parent is not accessible: {}", existing_parent.display()))?;
    ensure_inside(&root, &parent_canonical)?;
    Ok(candidate)
}

pub fn resolve_optional_existing(root: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    validate_workspace_relative(raw)?;
    let normalized = strip_redundant_root_prefix(root, raw);
    let candidate = root.join(&normalized);
    if candidate.exists() {
        resolve_existing(root, normalized.to_string_lossy().as_ref())
    } else {
        resolve_for_create(root, normalized.to_string_lossy().as_ref())
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
        bail!("path escapes workspace; {EXPECTED_PATH_FORM}");
    }
    Ok(())
}

fn reject_parent_components(path: &Path) -> anyhow::Result<()> {
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            bail!("path may not contain ..; {EXPECTED_PATH_FORM}");
        }
    }
    Ok(())
}

fn canonicalize_with_missing_leaf(path: &Path) -> anyhow::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        let Some(name) = cursor.file_name() else {
            bail!("path escapes workspace or parent is not accessible; {EXPECTED_PATH_FORM}");
        };
        missing.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            bail!("path escapes workspace or parent is not accessible; {EXPECTED_PATH_FORM}");
        };
        cursor = parent;
    }
    let mut resolved = cursor.canonicalize().with_context(|| {
        format!("path escapes workspace or parent is not accessible; {EXPECTED_PATH_FORM}")
    })?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn split_absolute_glob_base(raw: &str) -> Option<(&str, &str)> {
    let meta = raw.find(['*', '?', '[', '{'])?;
    let split = raw[..meta].rfind('/')?;
    if split == 0 {
        Some(("/", &raw[split + 1..]))
    } else {
        Some((&raw[..split], &raw[split + 1..]))
    }
}

fn strip_redundant_root_prefix(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    let Some(root_name) = root.file_name() else {
        return path.to_path_buf();
    };
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return path.to_path_buf();
    };
    if first != root_name {
        return path.to_path_buf();
    }
    let stripped = components.as_path();
    if stripped.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        stripped.to_path_buf()
    }
}

fn missing_path_message(root: &Path, raw: &str) -> String {
    if let Some(candidate) = missing_path_candidate(root, raw) {
        format!(
            "path_not_found_recoverable: path does not exist: {raw}; did you mean `{candidate}`?"
        )
    } else {
        format!("path does not exist: {raw}")
    }
}

fn missing_path_candidate(root: &Path, raw: &str) -> Option<String> {
    let path = Path::new(raw);
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return None;
    };
    if first != "workdir" {
        return None;
    }
    let tail = components.as_path();
    if tail.as_os_str().is_empty() {
        return None;
    }
    let candidate = root.join(tail);
    if !candidate.exists() {
        return None;
    }
    Some(tail.to_string_lossy().replace('\\', "/"))
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
    fn strips_redundant_workspace_root_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ok").unwrap();
        let raw = format!(
            "{}/a.txt",
            dir.path().file_name().unwrap().to_string_lossy()
        );
        let path = resolve_existing(dir.path(), &raw).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "ok");
    }

    #[test]
    fn suggests_workdir_prefix_without_silent_normalization() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ok").unwrap();
        let err = resolve_existing(dir.path(), "workdir/a.txt").unwrap_err();
        assert!(err.to_string().contains("path_not_found_recoverable"));
        assert!(err.to_string().contains("did you mean `a.txt`"));
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
