//! Workspace path filtering shared by controller evidence collectors.
//!
//! These helpers identify controller-owned or generated directories that must
//! never count as user artifacts, repo edits, verifier repair candidates, or
//! completion evidence.

use std::path::{Component, Path};

const IGNORED_WORKSPACE_DIRS: &[&str] = &[
    ".git",
    ".anvil",
    ".anvil-state",
    ".next",
    ".pytest_cache",
    "__pycache__",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
];

const CONTROL_METADATA_FILES: &[&str] = &[
    "prompt.md",
    "prompt.txt",
    "cmd.txt",
    "command.txt",
    "anvil.md",
    "session.json",
    "meta.json",
    "metadata.json",
    "llm-io.jsonl",
    "log.jsonl",
    "anvil.out",
    "anvil.err",
    "postcheck.out",
    "postcheck.err",
];

pub fn is_workspace_ignored_dir_name(name: &str) -> bool {
    IGNORED_WORKSPACE_DIRS.contains(&name)
}

pub fn is_control_metadata_file_name(name: &str) -> bool {
    CONTROL_METADATA_FILES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

pub fn is_ignored_workspace_relative_path(relative: &Path) -> bool {
    if relative.components().any(|component| match component {
        Component::Normal(name) => name.to_str().is_some_and(is_workspace_ignored_dir_name),
        _ => false,
    }) {
        return true;
    }
    let mut components = relative.components();
    let Some(Component::Normal(name)) = components.next() else {
        return false;
    };
    components.next().is_none() && name.to_str().is_some_and(is_control_metadata_file_name)
}

pub fn is_ignored_workspace_display_path(display_path: &str) -> bool {
    let path = display_path
        .strip_suffix(" (deleted)")
        .unwrap_or(display_path);
    is_ignored_workspace_relative_path(Path::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_workspace_relative_path_matches_components() {
        assert!(is_ignored_workspace_relative_path(Path::new(
            ".anvil-state/verifier-python/site/_pytest/__init__.py"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "app/__pycache__/main.cpython-39.pyc"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new("anvil.out")));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "postcheck.err"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new("prompt.md")));
        assert!(!is_ignored_workspace_relative_path(Path::new(
            "docs/anvil-state-notes.md"
        )));
        assert!(!is_ignored_workspace_relative_path(Path::new(
            "docs/anvil.out"
        )));
    }

    #[test]
    fn ignored_workspace_display_path_handles_deleted_suffix() {
        assert!(is_ignored_workspace_display_path(
            ".anvil-state/verifier-python/site/foo.py (deleted)"
        ));
        assert!(!is_ignored_workspace_display_path("README.md (deleted)"));
    }
}
