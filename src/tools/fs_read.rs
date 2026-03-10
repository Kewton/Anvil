use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

#[derive(Debug, Default)]
pub struct FileReadTool;

impl FileReadTool {
    pub fn read(&self, path: &Path) -> anyhow::Result<FileReadResult> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read file {}", path.display()))?;
        Ok(FileReadResult {
            path: path.to_path_buf(),
            contents,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileReadResult {
    pub path: PathBuf,
    pub contents: String,
}
