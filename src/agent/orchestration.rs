use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    files: BTreeMap<PathBuf, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoVerification {
    pub changed_files: Vec<String>,
    pub implementation_files_changed: usize,
    pub test_files_changed: usize,
    pub setup_files_changed: usize,
    pub deleted_files_changed: usize,
}

impl RepoVerification {
    pub fn made_any_progress(&self) -> bool {
        self.implementation_files_changed > 0
            || self.test_files_changed > 0
            || self.setup_files_changed > 0
    }

    pub fn total_changed_files(&self) -> usize {
        self.implementation_files_changed
            + self.test_files_changed
            + self.setup_files_changed
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
    let mut implementation_files_changed = 0usize;
    let mut test_files_changed = 0usize;
    let mut setup_files_changed = 0usize;
    let mut deleted_files_changed = 0usize;

    // modified or created files
    for (path, after_hash) in &after.files {
        let changed = before.files.get(path) != Some(after_hash);
        if !changed {
            continue;
        }
        let display = path.display().to_string();
        if changed_files.len() < 16 {
            changed_files.push(display.clone());
        }
        if is_test_file(path) {
            test_files_changed += 1;
        } else if is_setup_file(path) {
            setup_files_changed += 1;
        } else if is_implementation_file(path) {
            implementation_files_changed += 1;
        }
    }

    // deleted files (present before but absent after)
    for path in before.files.keys() {
        if after.files.contains_key(path) {
            continue;
        }
        let display = format!("{} (deleted)", path.display());
        if changed_files.len() < 16 {
            changed_files.push(display);
        }
        deleted_files_changed += 1;
    }

    RepoVerification {
        changed_files,
        implementation_files_changed,
        test_files_changed,
        setup_files_changed,
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
        .map(|relative| {
            relative.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some(".git" | ".anvil" | "node_modules" | "target")
                )
            })
        })
        .unwrap_or(false)
}

fn is_test_file(path: &Path) -> bool {
    let display = path.display().to_string();
    display.contains("__tests__") || display.contains(".test.") || display.contains(".spec.")
}

fn is_setup_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(
        file_name,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "tsconfig.json"
            | "jest.config.js"
            | "jest.config.ts"
            | "vitest.config.ts"
            | "vitest.config.js"
            | "next.config.ts"
            | "next.config.js"
            | "eslint.config.js"
            | "eslint.config.mjs"
    )
}

fn is_implementation_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "css"
                | "scss"
                | "html"
                | "mdx"
                | "vue"
                | "svelte"
        )
    )
}
