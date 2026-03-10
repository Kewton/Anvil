use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PathPolicy {
    workspace_root: PathBuf,
    writable_roots: Vec<PathBuf>,
}

impl PathPolicy {
    pub fn new(workspace_root: PathBuf, writable_roots: Vec<PathBuf>) -> Self {
        Self {
            workspace_root,
            writable_roots,
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn allows_write(&self, path: &Path) -> bool {
        let candidate = normalize(path);
        let workspace_root = normalize(&self.workspace_root);

        candidate.starts_with(&workspace_root)
            || self
                .writable_roots
                .iter()
                .map(|root| normalize(root.as_path()))
                .any(|root| candidate.starts_with(root))
    }
}

fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }

    let mut cursor = absolute.as_path();
    let mut suffix = Vec::<OsString>::new();

    while !cursor.exists() {
        if let Some(name) = cursor.file_name() {
            suffix.push(name.to_os_string());
        }

        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => return absolute,
        }
    }

    let mut resolved = std::fs::canonicalize(cursor).unwrap_or_else(|_| cursor.to_path_buf());
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    resolved
}
