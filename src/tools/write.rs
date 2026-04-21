use std::fs;
use std::path::Path;

pub fn run(path: &Path, content: &str) -> Result<String, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    fs::write(path, content).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(format!(
        "wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
}
