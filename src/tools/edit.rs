use std::fs;
use std::path::Path;

pub fn run(path: &Path, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    if !contents.contains(old) {
        return Err(format!("target text not found in {}", path.display()));
    }

    let updated = if replace_all {
        contents.replace(old, new)
    } else {
        contents.replacen(old, new, 1)
    };
    fs::write(path, updated).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(format!("edited {}", path.display()))
}
