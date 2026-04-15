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
        root.join(input)
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
