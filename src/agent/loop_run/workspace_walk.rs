//! Workspace walker helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts the bounded workspace traversal used to surface scaffold /
//! artifact-recovery candidates (`meaningful_workspace_files`,
//! `collect_meaningful_workspace_files`) and the `workspace_appears_empty`
//! probe used by the empty-workspace scaffold gate. Honours
//! `task_workspace_scope::is_workspace_ignored_dir` as the SSOT ignore
//! list.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceFileClass {
    ProjectArtifact,
    ProtectedInputMetadata,
    GeneratedMetadata,
}

pub(super) fn workspace_appears_empty(work_root: &Path) -> bool {
    !workspace_contains_project_artifact(work_root, work_root).unwrap_or(true)
}

pub(super) fn is_protected_input_metadata_path(relative_path: &str) -> bool {
    classify_workspace_relative_path(Path::new(relative_path))
        == WorkspaceFileClass::ProtectedInputMetadata
}

pub(super) fn classify_workspace_relative_path(relative: &Path) -> WorkspaceFileClass {
    if relative.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_str().is_some_and(super::task_workspace_scope::is_workspace_ignored_dir))
    }) {
        return WorkspaceFileClass::GeneratedMetadata;
    }
    if first_component_name(relative).is_some_and(|name| name.eq_ignore_ascii_case("logs")) {
        return WorkspaceFileClass::GeneratedMetadata;
    }
    let Some(name) = top_level_file_name(relative) else {
        return WorkspaceFileClass::ProjectArtifact;
    };
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "prompt.md" | "prompt.txt" | "cmd.txt" | "command.txt"
    ) {
        return WorkspaceFileClass::ProtectedInputMetadata;
    }
    if matches!(
        lower.as_str(),
        "anvil.md" | "session.json" | "meta.json" | "metadata.json" | "llm-io.jsonl" | "log.jsonl"
    ) {
        return WorkspaceFileClass::GeneratedMetadata;
    }
    WorkspaceFileClass::ProjectArtifact
}

fn first_component_name(relative: &Path) -> Option<&str> {
    match relative.components().next()? {
        std::path::Component::Normal(name) => name.to_str(),
        _ => None,
    }
}

fn top_level_file_name(relative: &Path) -> Option<&str> {
    let mut components = relative.components();
    let first = match components.next()? {
        std::path::Component::Normal(name) => name.to_str()?,
        _ => return None,
    };
    components.next().is_none().then_some(first)
}

fn workspace_contains_project_artifact(root: &Path, current: &Path) -> std::io::Result<bool> {
    let Ok(entries) = std::fs::read_dir(current) else {
        return Ok(true);
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if super::task_workspace_scope::is_workspace_ignored_dir(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if workspace_contains_project_artifact(root, &path)? {
                return Ok(true);
            }
            continue;
        }
        if (file_type.is_file() || file_type.is_symlink())
            && let Ok(relative) = path.strip_prefix(root)
            && classify_workspace_relative_path(relative) == WorkspaceFileClass::ProjectArtifact
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn meaningful_workspace_files(work_root: &Path, limit: usize) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_meaningful_workspace_files(work_root, work_root, limit, &mut files).ok()?;
    Some(files)
}

fn collect_meaningful_workspace_files(
    root: &Path,
    current: &Path,
    limit: usize,
    files: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    if files.len() > limit {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Issue #646 (E1): single SSOT for ignored workspace directories.
        if super::task_workspace_scope::is_workspace_ignored_dir(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_meaningful_workspace_files(root, &path, limit, files)?;
        } else if path.is_file()
            && let Ok(relative) = path.strip_prefix(root)
            && classify_workspace_relative_path(relative) == WorkspaceFileClass::ProjectArtifact
        {
            files.push(relative.to_path_buf());
            if files.len() > limit {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prompt_and_cmd_metadata_do_not_make_workspace_project_non_empty() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("prompt.md"), "build a Rust CLI\n").unwrap();
        std::fs::write(temp.path().join("cmd.txt"), "cargo test\n").unwrap();
        std::fs::write(temp.path().join("session.json"), "{}\n").unwrap();
        std::fs::write(temp.path().join("meta.json"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("logs")).unwrap();
        std::fs::write(temp.path().join("logs/llm-io.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join(".anvil-state/sessions")).unwrap();

        assert!(workspace_appears_empty(temp.path()));

        std::fs::write(temp.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert!(!workspace_appears_empty(temp.path()));
    }

    #[test]
    fn meaningful_workspace_files_excludes_protected_input_metadata() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("prompt.txt"), "task\n").unwrap();
        std::fs::write(temp.path().join("command.txt"), "npm test\n").unwrap();
        std::fs::write(temp.path().join("llm-io.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("logs")).unwrap();
        std::fs::write(temp.path().join("logs/run.log"), "log\n").unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();

        let files = meaningful_workspace_files(temp.path(), 16).unwrap();

        assert_eq!(files, vec![PathBuf::from("src/main.rs")]);
    }
}
