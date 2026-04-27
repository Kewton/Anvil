use std::collections::HashSet;
use std::fs;
use std::path::Path;

const TOKEN_MIN_LEN: usize = 4;
const TOKEN_MIN_SCORE: usize = 1;

pub fn run(path: &Path, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_string must not be empty".to_string());
    }
    if old == new {
        return Err("old_string and new_string are identical".to_string());
    }

    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let (updated, fallback_label) = if contents.contains(old) {
        let updated = if replace_all {
            contents.replace(old, new)
        } else {
            contents.replacen(old, new, 1)
        };
        (updated, None)
    } else if let Some(updated) = normalized_line_replace(&contents, old, new, replace_all) {
        (updated, Some("normalized-line fallback"))
    } else if !replace_all {
        if let Some(updated) = token_anchor_replace(&contents, old, new) {
            (updated, Some("token-anchor fallback"))
        } else {
            return Err(format!("target text not found in {}", path.display()));
        }
    } else {
        return Err(format!("target text not found in {}", path.display()));
    };

    fs::write(path, updated).map_err(|err| format!("failed to write {}: {err}", path.display()))?;

    let suffix = fallback_label
        .map(|label| format!(" ({label})"))
        .unwrap_or_default();
    Ok(format!("edited {}{}", path.display(), suffix))
}

#[derive(Clone, Copy)]
struct LineSpan<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn normalized_line_replace(
    contents: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let spans = line_spans(contents);
    let old_lines = normalized_lines(old);
    if spans.is_empty() || old_lines.is_empty() || old_lines.len() > spans.len() {
        return None;
    }

    let mut matches = Vec::new();
    for start in 0..=spans.len() - old_lines.len() {
        let window = &spans[start..start + old_lines.len()];
        if window
            .iter()
            .map(|line| normalize_line(line.text))
            .eq(old_lines.iter().copied())
        {
            matches.push((start, start + old_lines.len()));
        }
    }

    if matches.is_empty() {
        return None;
    }
    if !replace_all && matches.len() > 1 {
        return None;
    }

    let mut updated = contents.to_string();
    for (start, end) in matches.into_iter().rev() {
        replace_line_span(&mut updated, &spans, start, end, new);
    }
    Some(updated)
}

fn token_anchor_replace(contents: &str, old: &str, new: &str) -> Option<String> {
    let spans = line_spans(contents);
    let line_count = old.lines().count().max(1);
    let start = token_anchor_start(&spans, old)?;
    let end = (start + line_count).min(spans.len());
    let mut updated = contents.to_string();
    replace_line_span(&mut updated, &spans, start, end, new);
    Some(updated)
}

fn token_anchor_start(spans: &[LineSpan<'_>], old: &str) -> Option<usize> {
    let tokens = extract_tokens(old);
    if tokens.is_empty() {
        return None;
    }

    let mut best = None;
    let mut best_score = 0usize;
    let mut tied = false;

    for (index, line) in spans.iter().enumerate() {
        let lower = line.text.to_ascii_lowercase();
        let score = tokens
            .iter()
            .filter(|token| lower.contains(token.as_str()))
            .count();
        if score > best_score {
            best = Some(index);
            best_score = score;
            tied = false;
        } else if score > 0 && score == best_score {
            tied = true;
        }
    }

    if best_score >= TOKEN_MIN_SCORE && !tied {
        best
    } else {
        None
    }
}

fn replace_line_span(
    updated: &mut String,
    spans: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
    replacement: &str,
) {
    let start = spans[start_line].start;
    let end = spans[end_line - 1].end;
    updated.replace_range(start..end, replacement);
}

fn line_spans(contents: &str) -> Vec<LineSpan<'_>> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for chunk in contents.split_inclusive('\n') {
        let end = offset + chunk.len();
        spans.push(LineSpan {
            start: offset,
            end,
            text: chunk.trim_end_matches('\n').trim_end_matches('\r'),
        });
        offset = end;
    }

    if spans.is_empty() {
        spans.push(LineSpan {
            start: 0,
            end: contents.len(),
            text: contents,
        });
    }

    spans
}

fn normalized_lines(input: &str) -> Vec<&str> {
    input.lines().map(normalize_line).collect()
}

fn normalize_line(line: &str) -> &str {
    line.trim()
}

fn extract_tokens(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    input
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|token| token.len() >= TOKEN_MIN_LEN)
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::run;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn rejects_noop_edit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        let err = run(&path, "fn main() {}", "fn main() {}", false).unwrap_err();
        assert!(err.contains("identical"));
    }

    #[test]
    fn normalizes_indentation_and_line_endings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(&path, "fn main() {\r\n    println!(\"hello\");\r\n}\r\n").unwrap();
        let result = run(
            &path,
            "fn main() {\nprintln!(\"hello\");\n}",
            "fn main() {\n    println!(\"bye\");\n}\n",
            false,
        )
        .unwrap();
        assert!(result.contains("normalized-line fallback"));
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("bye"));
    }

    #[test]
    fn uses_token_anchor_for_single_line_drift() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(
            &path,
            "fn main() {\n    let my_special_variable = compute_result(42);\n}\n",
        )
        .unwrap();
        let result = run(
            &path,
            "let my_special_variable = compute_result(input_value);",
            "let my_special_variable = compute_result(7);",
            false,
        )
        .unwrap();
        assert!(result.contains("token-anchor fallback"));
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("compute_result(7)"));
    }

    #[test]
    fn rejects_ambiguous_token_anchor_matches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(
            &path,
            "let result_value = compute_result(1);\nlet result_value = compute_result(2);\n",
        )
        .unwrap();
        let err = run(
            &path,
            "let result_value = compute_result(input_value);",
            "let result_value = compute_result(7);",
            false,
        )
        .unwrap_err();
        assert!(err.contains("target text not found"));
    }
}
