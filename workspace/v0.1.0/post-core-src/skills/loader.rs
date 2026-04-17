use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct SkillLibrary {
    root: PathBuf,
}

impl SkillLibrary {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn list(&self) -> Result<Vec<String>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut skills = fs::read_dir(&self.root)
            .map_err(|err| format!("failed to read skill dir {}: {err}", self.root.display()))?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                let is_markdown = path.extension().and_then(|ext| ext.to_str()) == Some("md");
                if !is_markdown {
                    return None;
                }
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(|name| name.to_string())
            })
            .collect::<Vec<_>>();
        skills.sort();
        Ok(skills)
    }

    pub fn load(&self, name: &str) -> Result<SkillDocument, String> {
        let path = self.root.join(format!("{name}.md"));
        if !path.exists() {
            return Err(format!("skill not found: {}", path.display()));
        }
        let content = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        Ok(SkillDocument {
            name: name.to_string(),
            path,
            content,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
