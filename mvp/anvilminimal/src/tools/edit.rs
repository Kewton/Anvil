use std::path::Path;

use anyhow::bail;

pub fn run(path: &Path, old: &str, new: &str, replace_all: bool) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(path)?;
    if !content.contains(old) {
        bail!("exact anchor mismatch");
    }
    let edited = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    std::fs::write(path, edited)?;
    Ok(format!("edited {}", path.display()))
}
