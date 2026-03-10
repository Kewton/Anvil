use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

#[derive(Debug, Clone, Default)]
pub struct RepoInstructions {
    pub path: Option<PathBuf>,
    pub contents: Option<String>,
}

impl RepoInstructions {
    pub fn load(workspace_root: &Path) -> anyhow::Result<Self> {
        let path = workspace_root.join("anvil.md");
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path).with_context(|| {
            format!("failed to read repository instructions {}", path.display())
        })?;

        Ok(Self {
            path: Some(path),
            contents: Some(contents),
        })
    }

    pub fn is_present(&self) -> bool {
        self.contents.is_some()
    }

    pub fn as_context_block(&self) -> Option<crate::prompts::context::ContextBlock> {
        self.contents.as_ref().map(|contents| {
            let block = crate::prompts::context::ContextBlock::new(
                crate::runtime::trust::SourceType::AnvilMd,
                contents.clone(),
            );

            match &self.path {
                Some(path) => block.with_path(path.display().to_string()),
                None => block,
            }
        })
    }
}
