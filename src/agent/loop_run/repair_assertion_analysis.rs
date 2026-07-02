//! Small assertion/output analysis helpers used by verifier repair.
//!
//! This module is intentionally pure: it does not know about `RepairJob`,
//! filesystems, provider calls, or dispatch state.

use std::collections::HashMap;

pub(super) fn assert_line_update_has_observed_pair(
    deleted_line: &str,
    added_line: &str,
    observed_pairs: &[(String, String)],
) -> bool {
    let Some((old_lhs, old_expected)) = assert_equality_parts(deleted_line) else {
        return false;
    };
    let Some((new_lhs, new_expected)) = assert_equality_parts(added_line) else {
        return false;
    };
    old_lhs == new_lhs
        && observed_pairs
            .iter()
            .any(|(actual, expected)| actual == &new_expected && expected == &old_expected)
}

pub(super) fn changed_assert_lines(left: &str, right: &str) -> Vec<String> {
    let mut right_lines = line_count_map(right);
    let mut changed = Vec::new();
    for line in left.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(count) = right_lines.get_mut(line)
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        if line.starts_with("assert") {
            changed.push(line.to_string());
        }
    }
    changed
}

pub(super) fn changed_non_assert_lines(left: &str, right: &str) -> Vec<String> {
    let mut right_lines = line_count_map(right);
    let mut changed = Vec::new();
    for line in left.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(count) = right_lines.get_mut(line)
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        if !line.starts_with("assert") {
            changed.push(line.to_string());
        }
    }
    changed
}

pub(super) fn count_assert_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| line.trim_start().starts_with("assert "))
        .count()
}

pub(super) fn changed_assert_equalities(left: &str, right: &str) -> Vec<(String, String)> {
    let mut right_lines = line_count_map(right);
    let mut changed = Vec::new();
    for line in left.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(count) = right_lines.get_mut(line)
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        if let Some(parts) = assert_equality_parts(line) {
            changed.push(parts);
        }
    }
    changed
}

pub(super) fn changed_assert_comparisons(left: &str, right: &str) -> Vec<(String, String, String)> {
    let mut right_lines = line_count_map(right);
    let mut changed = Vec::new();
    for line in left.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(count) = right_lines.get_mut(line)
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        if let Some(parts) = assert_comparison_parts(line) {
            changed.push(parts);
        }
    }
    changed
}

pub(super) fn observed_assert_equal_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (_, tail) = line.split_once("assert ")?;
            let (actual, expected) = tail.split_once("==")?;
            Some((
                normalize_assert_literal_token(actual)?,
                normalize_assert_literal_token(expected)?,
            ))
        })
        .collect()
}

pub(super) fn observed_assert_not_equal_failed_values(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (_, tail) = line.split_once("assert ")?;
            let (actual, expected) = tail.split_once("!=")?;
            let actual = normalize_assert_literal_token(actual)?;
            let expected = normalize_assert_literal_token(expected)?;
            (actual == expected).then_some(actual)
        })
        .collect()
}

pub(super) fn pytest_output_suggests_shared_state_leak(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("left contains") || lower.contains("right contains") {
        return true;
    }
    observed_assert_equal_mismatches(text)
        .iter()
        .any(|mismatch| {
            let Some(lhs) = mismatch.source_lhs.as_deref() else {
                return false;
            };
            if !lhs.trim_start().starts_with("len(") {
                return false;
            }
            let Ok(actual) = mismatch.actual.parse::<i64>() else {
                return false;
            };
            let Ok(expected) = mismatch.expected.parse::<i64>() else {
                return false;
            };
            actual > expected
        })
}

pub(super) fn python_assert_line_observes_fixture_state(line: &str, fixture_name: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("assert ")
        && (trimmed.contains(&format!("len({fixture_name})"))
            || trimmed.contains(&format!("{fixture_name} =="))
            || trimmed.contains(&format!("{fixture_name} !="))
            || trimmed.contains(&format!("{fixture_name}[")))
}

pub(super) fn python_assert_line_is_obvious_weakening(line: &str) -> bool {
    let trimmed = line.trim().trim_end_matches(';').trim();
    matches!(trimmed, "assert True" | "assert true" | "assert!(true)")
}

fn line_count_map(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        *counts.entry(line.to_string()).or_insert(0) += 1;
    }
    counts
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedAssertMismatch {
    source_lhs: Option<String>,
    actual: String,
    expected: String,
}

fn observed_assert_equal_mismatches(text: &str) -> Vec<ObservedAssertMismatch> {
    let mut last_source: Option<(String, String)> = None;
    let mut mismatches = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('>') {
            if let Some((_, tail)) = trimmed.split_once("assert ") {
                let source_line = format!("assert {tail}");
                last_source = assert_equality_parts(&source_line);
            }
            continue;
        }
        if !trimmed.starts_with('E') {
            continue;
        }
        let Some((_, tail)) = trimmed.split_once("assert ") else {
            continue;
        };
        let Some((actual, expected)) = observed_assert_pair_from_tail(tail) else {
            continue;
        };
        let source_lhs = last_source
            .as_ref()
            .filter(|(_, source_expected)| source_expected == &expected)
            .map(|(lhs, _)| lhs.clone());
        mismatches.push(ObservedAssertMismatch {
            source_lhs,
            actual,
            expected,
        });
    }
    mismatches
}

fn observed_assert_pair_from_tail(tail: &str) -> Option<(String, String)> {
    let (actual, expected) = tail.split_once("==")?;
    Some((
        normalize_assert_literal_token(actual)?,
        normalize_assert_literal_token(expected)?,
    ))
}

fn assert_equality_parts(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("assert ") {
        return None;
    }
    let body = trimmed.strip_prefix("assert ")?;
    let (lhs, rhs) = body.split_once("==")?;
    Some((lhs.trim().to_string(), normalize_assert_literal_token(rhs)?))
}

fn assert_comparison_parts(line: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix("assert ")?;
    for operator in ["==", "!="] {
        if let Some((lhs, rhs)) = body.split_once(operator) {
            return Some((
                lhs.trim().to_string(),
                operator.to_string(),
                normalize_assert_literal_token(rhs)?,
            ));
        }
    }
    None
}

fn normalize_assert_literal_token(raw: &str) -> Option<String> {
    let token = raw
        .trim()
        .trim_start_matches('(')
        .chars()
        .take_while(|ch| !ch.is_whitespace() && !matches!(ch, ',' | ')' | ']' | '}' | ':' | ';'))
        .collect::<String>();
    let normalized = token.trim_matches(['\'', '"']).to_string();
    if normalized.is_empty() {
        return None;
    }
    let safe = normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    safe.then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_assert_lines_ignores_unchanged_duplicates() {
        let before = "assert len(items) == 2\nassert status == 200\nassert status == 200\n";
        let after = "assert len(items) == 3\nassert status == 200\n";

        assert_eq!(
            changed_assert_lines(before, after),
            vec![
                "assert len(items) == 2".to_string(),
                "assert status == 200".to_string()
            ],
        );
    }

    #[test]
    fn shared_state_leak_detects_growing_len_assertion() {
        let output = r#"
>       assert len(items) == 2
E       assert 3 == 2
"#;

        assert!(pytest_output_suggests_shared_state_leak(output));
    }

    #[test]
    fn observed_pair_matches_literal_update() {
        let pairs = observed_assert_equal_pairs("E       assert 3 == 2");

        assert!(assert_line_update_has_observed_pair(
            "assert len(items) == 2",
            "assert len(items) == 3",
            &pairs,
        ));
    }

    #[test]
    fn observed_not_equal_failure_extracts_equal_value() {
        let values =
            observed_assert_not_equal_failed_values("E       AssertionError: assert 0 != 0");

        assert_eq!(values, vec!["0".to_string()]);
    }

    #[test]
    fn changed_assert_comparisons_reports_operator_updates() {
        let before = "assert result.returncode != 0\n";
        let after = "assert result.returncode == 0\n";

        assert_eq!(
            changed_assert_comparisons(before, after),
            vec![(
                "result.returncode".to_string(),
                "!=".to_string(),
                "0".to_string()
            )]
        );
        assert_eq!(
            changed_assert_comparisons(after, before),
            vec![(
                "result.returncode".to_string(),
                "==".to_string(),
                "0".to_string()
            )]
        );
    }
}
