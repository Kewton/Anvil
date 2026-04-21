use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct GitCheckpointManager {
    root: PathBuf,
    pub checkpoints: Vec<String>,
}

impl GitCheckpointManager {
    pub fn new(root: impl Into<PathBuf>, checkpoints: Vec<String>) -> Self {
        Self {
            root: root.into(),
            checkpoints,
        }
    }

    pub fn is_git_repo(&self) -> bool {
        self.git(["rev-parse", "--is-inside-work-tree"])
            .map(|output| output.trim() == "true")
            .unwrap_or(false)
    }

    pub fn create_checkpoint(&mut self, label: &str) -> Result<Option<String>, String> {
        if !self.is_git_repo() {
            return Err("not a git repository".to_string());
        }

        let output = self.git(["stash", "push", "--include-untracked", "--message", label])?;
        if output.contains("No local changes to save") {
            return Ok(None);
        }

        let sha = self.git(["rev-parse", "refs/stash"])?;
        let sha = sha.trim().to_string();
        self.git(["stash", "apply", "--index", &sha])?;
        self.checkpoints.push(sha.clone());
        Ok(Some(sha))
    }

    pub fn rollback_latest(&mut self) -> Result<String, String> {
        let sha = self
            .checkpoints
            .last()
            .cloned()
            .ok_or_else(|| "no checkpoint available".to_string())?;
        self.restore_worktree_to_head()?;
        self.git(["stash", "apply", "--index", &sha])?;
        self.checkpoints.pop();
        Ok(sha)
    }

    fn restore_worktree_to_head(&self) -> Result<(), String> {
        if !self.is_git_repo() {
            return Err("not a git repository".to_string());
        }

        let status = Command::new("git")
            .args(["restore", "--source=HEAD", "--staged", "--worktree", "."])
            .current_dir(&self.root)
            .status()
            .map_err(|err| format!("failed to run git restore: {err}"))?;
        if !status.success() {
            return Err("git restore failed".to_string());
        }

        let status = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(&self.root)
            .status()
            .map_err(|err| format!("failed to run git clean: {err}"))?;
        if !status.success() {
            return Err("git clean failed".to_string());
        }
        Ok(())
    }

    fn git<I, S>(&self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        run_git(&self.root, args)
    }
}

pub fn run_git<I, S>(root: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|err| format!("failed to spawn git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
