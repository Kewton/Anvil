use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LoadedInstructions {
    pub project_path: Option<PathBuf>,
    pub project_text: Option<String>,
    pub memory_path: PathBuf,
    pub memory_text: String,
}

pub fn find_anvil_md(cwd: &Path) -> Option<PathBuf> {
    let mut cur = Some(cwd);
    while let Some(path) = cur {
        let candidate = path.join("ANVIL.md");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = path.parent();
    }
    None
}

pub fn load_instructions(cwd: &Path) -> anyhow::Result<LoadedInstructions> {
    let project_path = find_anvil_md(cwd);
    let project_text = match &project_path {
        Some(path) => Some(std::fs::read_to_string(path)?),
        None => None,
    };
    let memory_path = cwd.join("ANVIL-MEMORY.md");
    let memory_text = if memory_path.exists() {
        std::fs::read_to_string(&memory_path)?
    } else {
        "# ANVIL Memory\n".to_string()
    };
    Ok(LoadedInstructions {
        project_path,
        project_text,
        memory_path,
        memory_text,
    })
}
