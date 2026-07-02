use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::util::file_classify::{is_implementation_file, is_setup_file, is_test_file};
use crate::util::workspace_paths::is_workspace_artifact_admitted_relative_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    files: BTreeMap<PathBuf, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoVerification {
    /// Display-capped changed files. Kept small for summaries and telemetry.
    pub changed_files: Vec<String>,
    /// Complete changed file list for protocol evidence. Do not render this
    /// directly in user-facing summaries without an explicit cap.
    pub all_changed_files: Vec<String>,
    pub implementation_files_changed: usize,
    pub test_files_changed: usize,
    pub setup_files_changed: usize,
    pub other_files_changed: usize,
    pub deleted_files_changed: usize,
}

impl RepoVerification {
    pub fn made_any_progress(&self) -> bool {
        self.implementation_files_changed > 0
            || self.test_files_changed > 0
            || self.setup_files_changed > 0
            || self.other_files_changed > 0
    }

    pub fn total_changed_files(&self) -> usize {
        self.implementation_files_changed
            + self.test_files_changed
            + self.setup_files_changed
            + self.other_files_changed
            + self.deleted_files_changed
    }
}

pub fn capture_repo_snapshot(root: &Path) -> RepoSnapshot {
    let mut files = BTreeMap::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if should_skip_path(root, path) {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok().map(Path::to_path_buf) else {
            continue;
        };
        if let Ok(bytes) = fs::read(path) {
            files.insert(relative, stable_hash(&bytes));
        }
    }

    RepoSnapshot { files }
}

pub fn verify_repo_progress(before: &RepoSnapshot, root: &Path) -> RepoVerification {
    let after = capture_repo_snapshot(root);
    let mut changed_files = Vec::new();
    let mut all_changed_files = Vec::new();
    let mut implementation_files_changed = 0usize;
    let mut test_files_changed = 0usize;
    let mut setup_files_changed = 0usize;
    let mut other_files_changed = 0usize;
    let mut deleted_files_changed = 0usize;

    // modified or created files
    for (path, after_hash) in &after.files {
        let changed = before.files.get(path) != Some(after_hash);
        if !changed {
            continue;
        }
        let display = path.display().to_string();
        all_changed_files.push(display.clone());
        if changed_files.len() < 16 {
            changed_files.push(display.clone());
        }
        if is_test_file(path) {
            test_files_changed += 1;
        } else if is_setup_file(path) {
            setup_files_changed += 1;
        } else if is_implementation_file(path) {
            implementation_files_changed += 1;
        } else {
            other_files_changed += 1;
        }
    }

    // deleted files (present before but absent after)
    for path in before.files.keys() {
        if after.files.contains_key(path) {
            continue;
        }
        let display = format!("{} (deleted)", path.display());
        all_changed_files.push(display.clone());
        if changed_files.len() < 16 {
            changed_files.push(display);
        }
        deleted_files_changed += 1;
    }

    RepoVerification {
        changed_files,
        all_changed_files,
        implementation_files_changed,
        test_files_changed,
        setup_files_changed,
        other_files_changed,
        deleted_files_changed,
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn should_skip_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|relative| !is_workspace_artifact_admitted_relative_path(relative))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_snapshot_ignores_controller_owned_state() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(
            temp.path()
                .join(".anvil-state/verifier-python/site/_pytest"),
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("app")).unwrap();
        fs::write(
            temp.path()
                .join(".anvil-state/verifier-python/site/_pytest/__init__.py"),
            "internal",
        )
        .unwrap();
        fs::write(temp.path().join("app/main.py"), "print('ok')\n").unwrap();

        let snapshot = capture_repo_snapshot(temp.path());

        assert!(snapshot.files.contains_key(Path::new("app/main.py")));
        assert!(!snapshot.files.contains_key(Path::new(
            ".anvil-state/verifier-python/site/_pytest/__init__.py"
        )));
    }

    #[test]
    fn repo_progress_does_not_count_anvil_state_changes() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("app")).unwrap();
        fs::write(temp.path().join("app/main.py"), "print('before')\n").unwrap();
        let before = capture_repo_snapshot(temp.path());

        fs::create_dir_all(
            temp.path()
                .join(".anvil-state/verifier-python/site/_pytest"),
        )
        .unwrap();
        fs::write(
            temp.path()
                .join(".anvil-state/verifier-python/site/_pytest/__init__.py"),
            "internal",
        )
        .unwrap();
        fs::write(temp.path().join("app/main.py"), "print('after')\n").unwrap();

        let progress = verify_repo_progress(&before, temp.path());

        assert_eq!(progress.total_changed_files(), 1);
        assert_eq!(progress.all_changed_files, vec!["app/main.py"]);
    }
}

// Issue #456 / DR1-007: file classification helpers were moved to
// `crate::util::file_classify` so AnvilScore computation can re-use them
// without `session/` depending on `agent/`.
