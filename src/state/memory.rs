use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> anyhow::Result<String> {
        if !self.path.exists() {
            return Ok("# ANVIL Memory\n".to_string());
        }
        Ok(std::fs::read_to_string(&self.path)?)
    }

    pub fn add_entry(&self, text: &str) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut current = self.load()?;
        if !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str("- ");
        current.push_str(text.trim());
        current.push('\n');
        std::fs::write(&self.path, current)?;
        Ok(())
    }
}
