use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

#[derive(Debug, Default)]
pub struct FileEditTool;

impl FileEditTool {
    pub fn write(&self, path: &Path, contents: &str) -> anyhow::Result<FileEditResult> {
        fs::write(path, contents)
            .with_context(|| format!("failed to write file {}", path.display()))?;
        Ok(FileEditResult {
            path: path.to_path_buf(),
            bytes_written: contents.len(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileEditResult {
    pub path: PathBuf,
    pub bytes_written: usize,
}
