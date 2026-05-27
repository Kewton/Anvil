use std::path::Path;

use crate::util::workspace_paths::is_ignored_workspace_display_path;

pub(super) fn verifier_diagnostic_path_input_is_safe(raw_path: &str) -> bool {
    let path = raw_path.trim();
    if path.is_empty()
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || is_ignored_workspace_display_path(path)
    {
        return false;
    }
    !Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

pub(super) fn diagnostic_missing_setup_path_is_controller_writable(path: &str) -> bool {
    matches!(path, "pyproject.toml")
}

pub(super) fn python_missing_external_dependency_name(
    work_root: &Path,
    output: &str,
) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(marker_index) = lower.find("no module named") else {
            continue;
        };
        let candidate = line[marker_index + "no module named".len()..].trim();
        let candidate = candidate
            .trim_matches(|ch: char| {
                ch == '\''
                    || ch == '"'
                    || ch == ':'
                    || ch == '.'
                    || ch == ','
                    || ch == '`'
                    || ch == ' '
            })
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| {
                ch == '\'' || ch == '"' || ch == ':' || ch == '.' || ch == ','
            });
        if !python_dependency_module_name_is_safe(candidate) {
            continue;
        }
        let top_level = candidate.split('.').next().unwrap_or_default();
        if top_level.is_empty()
            || work_root.join(format!("{top_level}.py")).is_file()
            || work_root.join(top_level).is_dir()
        {
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

fn python_dependency_module_name_is_safe(module: &str) -> bool {
    !module.is_empty()
        && !module.starts_with('.')
        && module.split('.').all(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            (first == '_' || first.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
}

pub(super) fn missing_python_module_workspace_path(
    work_root: &Path,
    module: &str,
) -> Option<String> {
    let mut parts = module.split('.');
    let top = parts.next()?;
    if !work_root.join(top).is_dir() {
        return None;
    }
    if !work_root.join(top).join("__init__.py").is_file() {
        return None;
    }
    let relative = format!("{}.py", module.replace('.', "/"));
    let parent = Path::new(&relative).parent()?;
    if !work_root.join(parent).is_dir() {
        return None;
    }
    Some(relative)
}
