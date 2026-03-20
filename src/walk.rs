//! Unified directory walker with .gitignore support.
//!
//! Provides a single walk function used by both `tooling` (file.search) and
//! `retrieval` (repo-find) to ensure consistent file discovery behavior.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Directories that Anvil always skips, regardless of .gitignore content.
pub const SKIP_DIRS: &[&str] = &[".git", "target", ".anvil"];

/// File extensions considered binary (never searched/indexed).
pub const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "pdf", "zip", "gz", "tar", "exe", "dll", "so", "dylib",
    "o", "a", "class", "pyc", "pyo", "wasm", "ico", "lock",
];

/// Returns `true` if the directory name matches a known skip target.
pub fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// Returns `true` if the file path has a binary extension.
pub fn is_binary(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| BINARY_EXTENSIONS.contains(&ext))
}

/// Walk the directory tree rooted at `root`, respecting .gitignore rules,
/// skipping known non-project directories and binary files.
///
/// Returns an iterator of file paths (directories are not yielded).
pub fn walk(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .max_depth(Some(20))
        .filter_entry(|entry| {
            // For directories, check if they should be skipped
            if entry.file_type().is_some_and(|ft| ft.is_dir())
                && let Some(name) = entry.file_name().to_str()
            {
                return !should_skip_dir(name);
            }
            true
        })
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .filter(|entry| !is_binary(entry.path()))
        .map(|entry| entry.into_path())
}
