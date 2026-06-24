use std::path::{Path, PathBuf};

use anyhow::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePolicy {
    Normal,
}

pub fn canonical_workspace_root(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize workspace root {}", path.display()))
}
