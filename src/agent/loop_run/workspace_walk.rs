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

pub(super) fn workspace_appears_empty(work_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return false;
    };
    !entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        !matches!(
            name.as_ref(),
            ".git" | ".anvil" | ".anvil-state" | "ANVIL.md" | "node_modules" | "target"
        )
    })
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
        {
            files.push(relative.to_path_buf());
            if files.len() > limit {
                return Ok(());
            }
        }
    }
    Ok(())
}
