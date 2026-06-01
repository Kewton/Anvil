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

use crate::util::workspace_paths::{WorkspacePathClass, WorkspacePolicy};

pub(super) fn workspace_appears_empty(work_root: &Path) -> bool {
    !workspace_contains_user_deliverable(work_root, work_root).unwrap_or(true)
}

pub(super) fn is_user_deliverable_path(relative_path: &str) -> bool {
    WorkspacePolicy::default().is_user_deliverable_relative_path(Path::new(relative_path))
}

pub(super) fn classify_workspace_relative_path(relative: &Path) -> WorkspacePathClass {
    WorkspacePolicy::default().classify_relative_path(relative)
}

fn workspace_contains_user_deliverable(root: &Path, current: &Path) -> std::io::Result<bool> {
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
            if workspace_contains_user_deliverable(root, &path)? {
                return Ok(true);
            }
            continue;
        }
        if (file_type.is_file() || file_type.is_symlink())
            && let Ok(relative) = path.strip_prefix(root)
            && classify_workspace_relative_path(relative) == WorkspacePathClass::UserDeliverable
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
            && classify_workspace_relative_path(relative) == WorkspacePathClass::UserDeliverable
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
        std::fs::write(temp.path().join("anvil.out"), "controller stdout\n").unwrap();
        std::fs::write(temp.path().join("anvil.err"), "controller stderr\n").unwrap();
        std::fs::write(temp.path().join("postcheck.out"), "postcheck stdout\n").unwrap();
        std::fs::write(temp.path().join("postcheck.err"), "postcheck stderr\n").unwrap();
        std::fs::write(temp.path().join("postcheck.junit.xml"), "<testsuite />\n").unwrap();
        std::fs::write(temp.path().join("session.json"), "{}\n").unwrap();
        std::fs::write(temp.path().join("meta.json"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("logs")).unwrap();
        std::fs::write(temp.path().join("logs/llm-io.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join(".anvil")).unwrap();
        std::fs::write(temp.path().join(".anvil/session.json"), "{}\n").unwrap();
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
        std::fs::write(temp.path().join("anvil.out"), "controller stdout\n").unwrap();
        std::fs::write(temp.path().join("postcheck.err"), "postcheck stderr\n").unwrap();
        std::fs::write(temp.path().join("postcheck.junit.xml"), "<testsuite />\n").unwrap();
        std::fs::create_dir_all(temp.path().join("logs")).unwrap();
        std::fs::write(temp.path().join("logs/run.log"), "log\n").unwrap();
        std::fs::create_dir_all(temp.path().join(".anvil")).unwrap();
        std::fs::write(temp.path().join(".anvil/session.json"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();

        let files = meaningful_workspace_files(temp.path(), 16).unwrap();

        assert_eq!(files, vec![PathBuf::from("src/main.rs")]);
    }
}
