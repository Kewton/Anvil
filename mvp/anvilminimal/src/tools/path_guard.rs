use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

const EXPECTED_PATH_FORM: &str = "use workspace-relative paths";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePathNormalizationKind {
    AbsoluteInsideWorkspace,
    RootAnchorSalvage,
    MissingLeadingSlashRootAnchorSalvage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePathNormalization {
    pub relative: String,
    pub kind: WorkspacePathNormalizationKind,
}

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
    Ok(normalize_workspace_path(root, raw)?.map(|normalization| normalization.relative))
}

pub fn normalize_workspace_path(
    root: &Path,
    raw: &str,
) -> anyhow::Result<Option<WorkspacePathNormalization>> {
    if raw.as_bytes().contains(&0) {
        bail!("path contains NUL byte; {EXPECTED_PATH_FORM}");
    }
    if looks_like_windows_absolute(raw) {
        bail!("absolute path is not allowed; {EXPECTED_PATH_FORM}");
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        if looks_like_missing_leading_slash_absolute(raw) {
            let root = root
                .canonicalize()
                .context("workspace root is not accessible")?;
            if let Some(relative) = root_anchor_salvage(&root, path) {
                return Ok(Some(WorkspacePathNormalization {
                    relative,
                    kind: WorkspacePathNormalizationKind::MissingLeadingSlashRootAnchorSalvage,
                }));
            }
            bail!(
                "tool_args_path_malformed: path looks like an absolute path missing its leading slash; {EXPECTED_PATH_FORM}"
            );
        }
        return Ok(None);
    }
    reject_parent_components(path)?;
    let root = root
        .canonicalize()
        .context("workspace root is not accessible")?;
    match canonicalize_with_missing_leaf(path) {
        Ok(canonical) if ensure_inside(&root, &canonical).is_ok() => {
            let relative = canonical
                .strip_prefix(&root)
                .context("path escapes workspace")?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            Ok(Some(WorkspacePathNormalization {
                relative: if relative.is_empty() {
                    ".".to_string()
                } else {
                    relative
                },
                kind: WorkspacePathNormalizationKind::AbsoluteInsideWorkspace,
            }))
        }
        Ok(canonical) => {
            if looks_like_near_root_digit_variance(&root, &canonical)
                || looks_like_near_root_digit_variance(&root, path)
            {
                bail!(
                    "tool_args_path_near_root_corruption: current workspace root `{}`; rejected absolute path `{raw}` because it differs by digit variance; {EXPECTED_PATH_FORM}",
                    root.display()
                );
            }
            if let Some(relative) = root_anchor_salvage(&root, path) {
                Ok(Some(WorkspacePathNormalization {
                    relative,
                    kind: WorkspacePathNormalizationKind::RootAnchorSalvage,
                }))
            } else {
                bail!("path escapes workspace; {EXPECTED_PATH_FORM}")
            }
        }
        Err(err) => {
            if looks_like_near_root_digit_variance(&root, path) {
                bail!(
                    "tool_args_path_near_root_corruption: current workspace root `{}`; rejected absolute path `{raw}` because it differs by digit variance; {EXPECTED_PATH_FORM}",
                    root.display()
                );
            }
            if let Some(relative) = root_anchor_salvage(&root, path) {
                Ok(Some(WorkspacePathNormalization {
                    relative,
                    kind: WorkspacePathNormalizationKind::RootAnchorSalvage,
                }))
            } else {
                Err(err).with_context(|| {
                    format!(
                        "path escapes workspace or parent is not accessible; {EXPECTED_PATH_FORM}"
                    )
                })
            }
        }
    }
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

fn root_anchor_salvage(root: &Path, raw: &Path) -> Option<String> {
    let root_components = normal_components(root);
    let raw_components = normal_components(raw);
    if root_components.is_empty() || raw_components.is_empty() {
        return None;
    }
    let anchor_len = root_components.len().min(2);
    let anchor = &root_components[root_components.len() - anchor_len..];
    let mut matches = raw_components
        .windows(anchor_len)
        .enumerate()
        .filter_map(|(index, window)| (window == anchor).then_some(index))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let index = matches.pop()?;
    let tail = &raw_components[index + anchor_len..];
    let relative = if tail.is_empty() {
        ".".to_string()
    } else {
        tail.join("/")
    };
    if validate_workspace_relative(&relative).is_err() {
        return None;
    }
    Some(relative)
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
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

fn looks_like_missing_leading_slash_absolute(raw: &str) -> bool {
    let path = Path::new(raw);
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return false;
    };
    let first = first.to_string_lossy();
    matches!(first.as_ref(), "Users" | "home") && components.next().is_some()
}

fn looks_like_near_root_digit_variance(root: &Path, raw: &Path) -> bool {
    let root_components = normal_components(root);
    let raw_components = normal_components(raw);
    near_root_digit_variance_components(&root_components, &raw_components)
        || (root_components
            .first()
            .is_some_and(|component| component == "private")
            && near_root_digit_variance_components(&root_components[1..], &raw_components))
}

fn near_root_digit_variance_components(
    root_components: &[String],
    raw_components: &[String],
) -> bool {
    if raw_components.len() < root_components.len() || root_components.is_empty() {
        return false;
    }
    let mut digit_variance_components = 0usize;
    for (root, raw) in root_components.iter().zip(raw_components.iter()) {
        if root == raw {
            continue;
        }
        if same_shape_with_digit_variance(root, raw) {
            digit_variance_components += 1;
        } else {
            return false;
        }
    }
    digit_variance_components == 1
}

fn same_shape_with_digit_variance(left: &str, right: &str) -> bool {
    let mut saw_digit_difference = false;
    let mut left_chars = left.chars();
    let mut right_chars = right.chars();
    loop {
        match (left_chars.next(), right_chars.next()) {
            (Some(left), Some(right)) if left == right => {}
            (Some(left), Some(right)) if left.is_ascii_digit() && right.is_ascii_digit() => {
                saw_digit_difference = true;
            }
            (None, None) => return saw_digit_difference,
            _ => return false,
        }
    }
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
    fn salvages_absolute_path_with_unique_root_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0708_013");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        let raw = dir
            .path()
            .join("share/work/commandagent_mvp/01/test0708_013/package.json");
        let normalization = normalize_workspace_path(&root, raw.to_str().unwrap())
            .unwrap()
            .expect("salvaged");

        assert_eq!(normalization.relative, "package.json");
        assert_eq!(
            normalization.kind,
            WorkspacePathNormalizationKind::RootAnchorSalvage
        );
        let path = resolve_existing(&root, &normalization.relative).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{}");
    }

    #[test]
    fn salvages_missing_leading_slash_path_with_unique_root_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0709_camp_003");
        std::fs::create_dir_all(&root).unwrap();
        let raw =
            "Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_camp_003/src/app/page.tsx"
                .to_string();

        let normalization = normalize_workspace_path(&root, &raw).unwrap().unwrap();

        assert_eq!(normalization.relative, "src/app/page.tsx");
        assert_eq!(
            normalization.kind,
            WorkspacePathNormalizationKind::MissingLeadingSlashRootAnchorSalvage
        );
    }

    #[test]
    fn rejects_missing_leading_slash_path_without_unique_root_anchor_as_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0709_camp_003");
        std::fs::create_dir_all(&root).unwrap();

        let err = normalize_workspace_path(&root, "Users/maenokota/other/src/App.js").unwrap_err();

        assert!(
            err.to_string().contains("tool_args_path_malformed"),
            "{err}"
        );
    }

    #[test]
    fn rejects_current_root_digit_variance_without_cross_workspace_salvage() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0710_camp_002");
        std::fs::create_dir_all(&root).unwrap();
        let raw = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0710_camp_001/src/app/page.tsx");

        let err = normalize_workspace_path(&root, raw.to_str().unwrap()).unwrap_err();

        assert!(
            err.to_string()
                .contains("tool_args_path_near_root_corruption"),
            "{err}"
        );
        assert!(err.to_string().contains(root.to_str().unwrap()), "{err}");
    }

    #[test]
    fn root_anchor_salvage_requires_unique_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0708_013");
        std::fs::create_dir_all(&root).unwrap();
        let raw = dir
            .path()
            .join("share/01/test0708_013/archive/01/test0708_013/package.json");

        let err = normalize_workspace_path(&root, raw.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("path escapes workspace"), "{err}");
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
