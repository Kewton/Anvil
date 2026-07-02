use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePolicy {
    Normal,
    NormalTask,
    ControllerMetadataAllowed,
    GeneratedArtifactsAllowed,
}

pub fn canonical_workspace_root(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize workspace root {}", path.display()))
}

impl WorkspacePolicy {
    pub fn for_task_request() -> Self {
        Self::NormalTask
    }

    pub fn allows_component(self, component: &str) -> bool {
        match self {
            WorkspacePolicy::ControllerMetadataAllowed => component != ".git",
            WorkspacePolicy::GeneratedArtifactsAllowed => !matches!(component, ".git" | ".anvil"),
            WorkspacePolicy::Normal | WorkspacePolicy::NormalTask => {
                !is_blocked_component(component)
            }
        }
    }
}

pub fn ensure_tool_path_allowed(
    root: &Path,
    path: &Path,
    policy: WorkspacePolicy,
) -> anyhow::Result<()> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    for component in rel.components() {
        let Some(part) = component.as_os_str().to_str() else {
            continue;
        };
        if !policy.allows_component(part) {
            bail!("workspace_policy_blocked: path component `{part}` is hidden from normal tasks");
        }
    }
    Ok(())
}

pub fn should_skip_path(root: &Path, path: &Path, policy: WorkspacePolicy) -> bool {
    ensure_tool_path_allowed(root, path, policy).is_err()
}

fn is_blocked_component(component: &str) -> bool {
    matches!(
        component,
        ".git"
            | ".anvil"
            | ".next"
            | "target"
            | "node_modules"
            | "coverage"
            | ".cache"
            | "__pycache__"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_workspace_policy_blocks_metadata_components() {
        let root = Path::new("/tmp/work");
        assert!(
            ensure_tool_path_allowed(
                root,
                Path::new("/tmp/work/.anvil/session.json"),
                WorkspacePolicy::NormalTask,
            )
            .is_err()
        );
        assert!(
            ensure_tool_path_allowed(
                root,
                Path::new("/tmp/work/my-node_modules-note.md"),
                WorkspacePolicy::NormalTask,
            )
            .is_ok()
        );
    }

    #[test]
    fn controller_metadata_policy_allows_anvil_but_not_git() {
        let root = Path::new("/tmp/work");
        assert!(
            ensure_tool_path_allowed(
                root,
                Path::new("/tmp/work/.anvil/plan.yaml"),
                WorkspacePolicy::ControllerMetadataAllowed,
            )
            .is_ok()
        );
        assert!(
            ensure_tool_path_allowed(
                root,
                Path::new("/tmp/work/.git/config"),
                WorkspacePolicy::ControllerMetadataAllowed,
            )
            .is_err()
        );
    }
}
