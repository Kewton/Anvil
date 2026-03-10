use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub content: String,
}

pub fn write_files(base: &Path, files: &[GeneratedFile]) -> anyhow::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for file in files {
        let dest = base.join(&file.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.content)?;
        written.push(dest);
    }
    Ok(written)
}

pub fn read_file(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}
