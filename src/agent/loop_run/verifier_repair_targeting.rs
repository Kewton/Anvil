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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn verifier_diagnostic_path_input_rejects_unsafe_shapes() {
        assert!(!verifier_diagnostic_path_input_is_safe(""));
        assert!(!verifier_diagnostic_path_input_is_safe("../app/main.py"));
        assert!(!verifier_diagnostic_path_input_is_safe("/tmp/app/main.py"));
        assert!(!verifier_diagnostic_path_input_is_safe("app/with\0nul.py"));
        assert!(!verifier_diagnostic_path_input_is_safe(
            ".anvil-state/sessions/job.json"
        ));
        assert!(verifier_diagnostic_path_input_is_safe("app/main.py"));
    }

    #[test]
    fn python_missing_external_dependency_ignores_local_modules() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("localpkg.py"), "").unwrap();

        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named 'requests'"
            )
            .as_deref(),
            Some("requests")
        );
        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named 'localpkg'"
            ),
            None
        );
        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named '../unsafe'"
            ),
            None
        );
    }

    #[test]
    fn missing_python_module_workspace_path_requires_package_parent() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app").join("services")).unwrap();
        std::fs::write(work_root.join("app").join("__init__.py"), "").unwrap();

        assert_eq!(
            missing_python_module_workspace_path(work_root, "app.services.worker").as_deref(),
            Some("app/services/worker.py")
        );
        assert_eq!(
            missing_python_module_workspace_path(work_root, "app.missing.worker"),
            None
        );
        assert_eq!(
            missing_python_module_workspace_path(work_root, "other.services.worker"),
            None
        );
    }
}
