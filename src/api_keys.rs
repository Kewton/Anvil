use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) fn load_api_key(workspace_root: &Path, key: &str) -> Result<String, String> {
    if let Ok(value) = std::env::var(key)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    let mut candidates = Vec::new();
    push_env_candidates(&mut candidates, workspace_root);
    if let Ok(current) = std::env::current_dir() {
        push_env_candidates(&mut candidates, &current);
    }

    for path in candidates {
        if let Some(value) = read_env_key(&path, key)? {
            return Ok(value);
        }
    }

    Err(format!(
        "{key} is not set; set it in the environment or a .env file"
    ))
}

fn push_env_candidates(candidates: &mut Vec<PathBuf>, root: &Path) {
    let mut seen = candidates.iter().cloned().collect::<BTreeSet<_>>();
    for ancestor in root.ancestors().take(8) {
        let candidate = ancestor.join(".env");
        if seen.insert(candidate.clone()) {
            candidates.push(candidate);
        }
    }
}

fn read_env_key(path: &Path, key: &str) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
        let Some((raw_key, raw_value)) = without_export.split_once('=') else {
            continue;
        };
        if raw_key.trim() != key {
            continue;
        }
        let value = raw_value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(None)
}
