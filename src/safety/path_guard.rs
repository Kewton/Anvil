use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn resolve_user_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.contains('\0') {
        return Err("path contains NUL byte".to_string());
    }

    let root = fs::canonicalize(root)
        .map_err(|err| format!("failed to canonicalize root {}: {err}", root.display()))?;
    let input = Path::new(raw);
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(strip_redundant_root_prefix(&root, input))
    };
    let normalized = canonicalize_with_missing_tail(&candidate)?;
    if !normalized.starts_with(&root) {
        return Err(format!("path escapes workspace: {}", candidate.display()));
    }
    Ok(normalized)
}

fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut cursor = path;

    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        missing.push(name.to_os_string());
        cursor = cursor
            .parent()
            .ok_or_else(|| format!("invalid path parent: {}", path.display()))?;
    }

    let mut resolved = fs::canonicalize(cursor)
        .map_err(|err| format!("failed to resolve {}: {err}", cursor.display()))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }

    Ok(normalize_components(resolved))
}

fn normalize_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn strip_redundant_root_prefix<'a>(root: &Path, input: &'a Path) -> &'a Path {
    let Some(root_name) = root.file_name() else {
        return input;
    };

    let mut stripped = input;
    loop {
        let mut components = stripped.components();
        let Some(Component::Normal(first)) = components.next() else {
            return stripped;
        };
        if first != root_name {
            return stripped;
        }

        let remainder = components.as_path();
        if remainder.as_os_str().is_empty() {
            return remainder;
        }

        let original = root.join(stripped);
        let normalized_original = normalize_components(original.clone());
        let normalized_remainder = normalize_components(root.join(remainder));
        let original_exists = normalized_original.exists();
        let remainder_exists = normalized_remainder.exists();
        let remainder_parent_exists = normalized_remainder
            .parent()
            .map(Path::exists)
            .unwrap_or(false);

        if !original_exists && (remainder_exists || remainder_parent_exists) {
            stripped = remainder;
            continue;
        }
        return stripped;
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_user_path;
    use tempfile::tempdir;

    #[test]
    fn strips_duplicate_project_prefix_after_root_switch() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("space-invaders");
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::write(
            root.join("src/app/page.tsx"),
            "export default function Page(){}",
        )
        .unwrap();

        let resolved = resolve_user_path(&root, "space-invaders/src/app/page.tsx").unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(root.join("src/app/page.tsx")).unwrap()
        );
    }

    #[test]
    fn keeps_normal_relative_paths_unchanged() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn x() {}").unwrap();

        let resolved = resolve_user_path(dir.path(), "src/lib.rs").unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path().join("src/lib.rs")).unwrap()
        );
    }
}
