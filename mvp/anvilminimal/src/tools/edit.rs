use std::path::Path;

use anyhow::bail;

pub fn run(path: &Path, old: &str, new: &str, replace_all: bool) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(path)?;
    if old == new {
        bail!("edit_noop: old_string and new_string are identical");
    }
    if !content.contains(old) && !new.is_empty() && content.contains(new) {
        return Ok(format!("edit_already_applied {}", path.display()));
    }
    let occurrences = content.matches(old).count();
    if occurrences > 1 && !replace_all {
        bail!(
            "edit_ambiguous_anchor: old_string appears {occurrences} times; Read the file and provide a more specific anchor"
        );
    }
    if !content.contains(old) {
        if let Some(edited) = normalized_line_fallback(&content, old, new) {
            std::fs::write(path, edited)?;
            return Ok(format!(
                "edited {} via normalized-line fallback",
                path.display()
            ));
        }
        if let Some(edited) = token_anchor_fallback(&content, old, new) {
            std::fs::write(path, edited)?;
            return Ok(format!(
                "edited {} via token-anchor fallback",
                path.display()
            ));
        }
        bail!(
            "edit_anchor_not_found: exact anchor mismatch; Read the file again and retry with a smaller exact anchor"
        );
    }
    let edited = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    std::fs::write(path, edited)?;
    Ok(format!("edited {}", path.display()))
}

fn normalized_line_fallback(content: &str, old: &str, new: &str) -> Option<String> {
    let old_lines = old.lines().collect::<Vec<_>>();
    if old_lines.len() != 1 {
        return None;
    }
    let old_normalized = normalize_ws(old_lines[0]);
    let mut matches = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if normalize_ws(line) == old_normalized {
            matches.push(idx);
        }
    }
    if matches.len() != 1 {
        return None;
    }
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    lines[matches[0]] = new.to_string();
    let mut edited = lines.join("\n");
    if content.ends_with('\n') {
        edited.push('\n');
    }
    Some(edited)
}

fn token_anchor_fallback(content: &str, old: &str, new: &str) -> Option<String> {
    let tokens = old.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    let first = tokens.first()?;
    let last = tokens.last()?;
    let start = content.find(first)?;
    if content[start + first.len()..].contains(first) {
        return None;
    }
    let after_first = start + first.len();
    let rel_end = content[after_first..].find(last)?;
    let end = after_first + rel_end + last.len();
    if content[end..].contains(last) {
        return None;
    }
    let mut edited = String::new();
    edited.push_str(&content[..start]);
    edited.push_str(new);
    edited.push_str(&content[end..]);
    Some(edited)
}

fn normalize_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_already_applied_is_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "new").unwrap();
        let output = run(&path, "old", "new", false).unwrap();
        assert!(output.contains("already_applied"));
    }

    #[test]
    fn edit_noop_returns_recoverable_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "same").unwrap();
        let err = run(&path, "same", "same", false).unwrap_err().to_string();
        assert!(err.contains("edit_noop"));
    }

    #[test]
    fn edit_normalized_line_fallback_applies_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "const  x = 1;\n").unwrap();
        run(&path, "const x = 1;", "const x = 2;", false).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "const x = 2;\n");
    }

    #[test]
    fn edit_token_anchor_fallback_applies_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha middle omega").unwrap();
        run(&path, "alpha changed omega", "done", false).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "done");
    }

    #[test]
    fn edit_fallback_does_not_apply_when_multiple_candidates_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha one omega\nalpha two omega\n").unwrap();
        let err = run(&path, "alpha changed omega", "done", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("edit_anchor_not_found"));
    }
}
