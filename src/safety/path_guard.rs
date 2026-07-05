use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn resolve_user_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    resolve_user_path_with_required_paths(root, raw, &[])
}

pub fn resolve_user_path_with_required_paths(
    root: &Path,
    raw: &str,
    required_paths: &[String],
) -> Result<PathBuf, String> {
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
    let normalized_result = canonicalize_with_missing_tail(&candidate);
    if let Ok(normalized) = normalized_result.as_ref()
        && normalized.starts_with(&root)
    {
        return Ok(normalized.clone());
    }
    if input.is_absolute() {
        let anchor_match = root_anchor_match(&root, input);
        if let Some((relative, anchor)) = root_anchor_suffix_candidate(&root, input) {
            let salvaged = root.join(&relative);
            if let Ok(normalized) = canonicalize_with_missing_tail(&salvaged)
                && normalized.starts_with(&root)
            {
                log_tool_args_path_salvaged(raw, &relative, &anchor);
                return Ok(normalized);
            }
        }
        if let Some(relative) = required_path_filename_candidate(input, required_paths) {
            let salvaged = root.join(&relative);
            if let Ok(normalized) = canonicalize_with_missing_tail(&salvaged)
                && normalized.starts_with(&root)
            {
                log_tool_args_path_salvaged(raw, &relative, "required_path");
                return Ok(normalized);
            }
        }
        if anchor_match || !required_paths.is_empty() {
            let suggestion = closest_required_path_suggestion(input, required_paths)
                .map(|candidate| format!(" Did you mean {candidate}?"))
                .unwrap_or_default();
            return Err(format!(
                "tool_args_path_corrupted_workspace_prefix: absolute path appears to contain a corrupted workspace prefix: {}. Use a workspace-relative path instead.{}",
                candidate.display(),
                suggestion
            ));
        }
    }

    match normalized_result {
        Ok(normalized) => Err(format!("path escapes workspace: {}", normalized.display())),
        Err(err) => Err(err),
    }
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

fn root_anchor_suffix_candidate(root: &Path, input: &Path) -> Option<(PathBuf, String)> {
    let root_components = normal_component_strings(root);
    if root_components.len() < 2 {
        return None;
    }
    let anchor_first = &root_components[root_components.len() - 2];
    let anchor_second = &root_components[root_components.len() - 1];
    let input_components = input.components().collect::<Vec<_>>();
    let matches = input_components
        .windows(2)
        .enumerate()
        .filter_map(|(index, window)| match (&window[0], &window[1]) {
            (Component::Normal(first), Component::Normal(second))
                if *first == anchor_first.as_os_str() && *second == anchor_second.as_os_str() =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [anchor_index] = matches.as_slice() else {
        return None;
    };
    let trailing = &input_components[anchor_index + 2..];
    let relative = clean_relative_from_components(trailing)?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let anchor = format!(
        "{}/{}",
        anchor_first.to_string_lossy(),
        anchor_second.to_string_lossy()
    );
    Some((relative, anchor))
}

fn root_anchor_match(root: &Path, input: &Path) -> bool {
    let root_components = normal_component_strings(root);
    if root_components.len() < 2 {
        return false;
    }
    let anchor_first = &root_components[root_components.len() - 2];
    let anchor_second = &root_components[root_components.len() - 1];
    input
        .components()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|window| {
            matches!(
                (&window[0], &window[1]),
                (Component::Normal(first), Component::Normal(second))
                    if *first == anchor_first.as_os_str()
                        && *second == anchor_second.as_os_str()
            )
        })
}

fn clean_relative_from_components(components: &[Component<'_>]) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in components {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => relative.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

fn required_path_filename_candidate(input: &Path, required_paths: &[String]) -> Option<PathBuf> {
    let input_name = input.file_name()?;
    let matches = required_paths
        .iter()
        .filter_map(|path| clean_relative_path(Path::new(path)))
        .filter(|path| path.file_name() == Some(input_name))
        .collect::<std::collections::BTreeSet<_>>();
    let mut matches = matches.into_iter();
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

fn closest_required_path_suggestion(input: &Path, required_paths: &[String]) -> Option<String> {
    let clean = required_paths
        .iter()
        .filter_map(|path| clean_relative_path(Path::new(path)))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    if clean.is_empty() {
        return None;
    }
    if clean.len() == 1 {
        return clean.into_iter().next();
    }

    let input_ext = input.extension().and_then(|ext| ext.to_str());
    let mut same_ext = clean
        .iter()
        .filter(|path| Path::new(path).extension().and_then(|ext| ext.to_str()) == input_ext)
        .cloned()
        .collect::<Vec<_>>();
    same_ext.sort();
    same_ext.into_iter().next().or_else(|| {
        let mut sorted = clean;
        sorted.sort();
        sorted.into_iter().next()
    })
}

fn clean_relative_path(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => relative.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

fn normal_component_strings(path: &Path) -> Vec<std::ffi::OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect()
}

fn log_tool_args_path_salvaged(original: &str, relative: &Path, anchor: &str) {
    crate::logging::log_llm_event(
        "tool_args_path_salvaged",
        serde_json::json!({
            "original": original,
            "salvaged": relative.to_string_lossy().replace('\\', "/"),
            "anchor": anchor,
        }),
    );
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
    use super::{resolve_user_path, resolve_user_path_with_required_paths};
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

    #[test]
    fn salvages_corrupted_absolute_prefix_by_last_two_root_components() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("share/work/localcommandagent_mvp/62646565");
        std::fs::create_dir_all(&root).unwrap();

        let raw = "/Users/maenokota/share/localcommandagent_mvp/62646565/next.config.js";
        let resolved = resolve_user_path(&root, raw).unwrap();

        assert_eq!(
            resolved,
            std::fs::canonicalize(&root).unwrap().join("next.config.js")
        );
    }

    #[test]
    fn salvages_absolute_prefix_by_unique_required_filename() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let resolved = resolve_user_path_with_required_paths(
            dir.path(),
            "/outside/project/next.config.js",
            &["next.config.js".to_string()],
        )
        .unwrap();

        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path())
                .unwrap()
                .join("next.config.js")
        );
    }

    #[test]
    fn corrupted_suffix_rejects_with_kind_and_candidate_hint() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("share/work/localcommandagent_mvp/62646565");
        std::fs::create_dir_all(&root).unwrap();

        let raw = "/Users/maenokota/share/localcommandagent_mvp/62646565/../next.confg.js";
        let err =
            resolve_user_path_with_required_paths(&root, raw, &["next.config.js".to_string()])
                .unwrap_err();

        assert!(err.contains("tool_args_path_corrupted_workspace_prefix"));
        assert!(err.contains("Did you mean next.config.js?"));
    }

    #[test]
    fn outside_workspace_without_anchor_still_rejects_as_escape() {
        let dir = tempdir().unwrap();
        let err = resolve_user_path(dir.path(), "/tmp/not-anvil-file.txt").unwrap_err();

        assert!(err.contains("path escapes workspace"), "got: {err}");
    }

    #[test]
    fn symlink_escape_still_rejects() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(outside.path(), dir.path().join("link")).unwrap();

        let err = resolve_user_path(dir.path(), "link/escape.txt").unwrap_err();

        assert!(err.contains("path escapes workspace"), "got: {err}");
    }
}
