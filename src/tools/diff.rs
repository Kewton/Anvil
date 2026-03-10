use std::path::Path;
use std::process::Command;

use anyhow::Context;

#[derive(Debug, Default)]
pub struct DiffTool;

impl DiffTool {
    pub fn diff(&self, workspace_root: &Path) -> anyhow::Result<String> {
        let output = Command::new("git")
            .args(["diff", "--", "."])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git diff")?;

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}
