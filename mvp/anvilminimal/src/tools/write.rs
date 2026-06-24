use std::path::Path;

pub fn run(path: &Path, content: &str) -> anyhow::Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(format!("wrote {}", path.display()))
}
