pub(super) fn verifier_failure_error_kind(output: &str) -> Option<String> {
    let failed_tests = verifier_failed_tests_signature(output);
    let exception = verifier_primary_exception_signature(output);
    match (failed_tests, exception) {
        (Some(failed_tests), Some(exception)) => {
            return Some(compact_verifier_failure_text(
                &format!("{failed_tests} exception:{exception}"),
                220,
            ));
        }
        (Some(failed_tests), None) => return Some(failed_tests),
        (None, Some(exception)) => return Some(exception),
        (None, None) => {}
    }
    for line in output.lines() {
        let trimmed = line.trim();
        let candidate = trimmed
            .strip_prefix("E   ")
            .or_else(|| trimmed.strip_prefix("E "))
            .or_else(|| trimmed.strip_prefix("error:"))
            .or_else(|| trimmed.strip_prefix("Error:"))
            .map(str::trim);
        if let Some(candidate) = candidate
            && !candidate.is_empty()
        {
            return Some(compact_verifier_failure_text(candidate, 160));
        }
    }
    output
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && (line.contains("Error")
                    || line.contains("error")
                    || line.contains("FAILED")
                    || line.contains("Assertion"))
        })
        .map(|line| compact_verifier_failure_text(line, 160))
}

fn verifier_primary_exception_signature(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        let candidate = trimmed
            .strip_prefix("E   ")
            .or_else(|| trimmed.strip_prefix("E "))
            .map(str::trim)
            .unwrap_or(trimmed);
        let exception = candidate
            .split_once(':')
            .map(|(head, _)| head)
            .unwrap_or(candidate);
        let exception = exception.trim();
        if exception.is_empty() || exception.starts_with("assert ") {
            continue;
        }
        let last_segment = exception.rsplit('.').next().unwrap_or(exception);
        let looks_like_exception = last_segment.ends_with("Error")
            || last_segment.ends_with("Exception")
            || last_segment == "Failed";
        if looks_like_exception
            && exception
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
        {
            return Some(compact_verifier_failure_text(exception, 120));
        }
    }
    None
}

fn verifier_failed_tests_signature(output: &str) -> Option<String> {
    let framework = crate::tools::test_output::detect_framework(output);
    let failed_tests = crate::tools::test_output::parse_failed_tests(output, framework);
    if failed_tests.raw_count == 0 || failed_tests.names.is_empty() {
        return None;
    }
    let names = failed_tests
        .names
        .iter()
        .take(8)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    Some(compact_verifier_failure_text(
        &format!("failed_tests:{}:{names}", failed_tests.raw_count),
        220,
    ))
}

pub(super) fn verifier_failure_signature(
    output: &str,
    target_path: Option<&str>,
    target_line: Option<usize>,
    error_kind: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if let Some(path) = target_path {
        let mut path = path.to_string();
        if let Some(line) = target_line {
            path.push(':');
            path.push_str(&line.to_string());
        }
        parts.push(path);
    }
    if let Some(error_kind) = error_kind
        && !error_kind.is_empty()
    {
        parts.push(error_kind.to_string());
    }
    if parts.is_empty() {
        if let Some(line) = output.lines().map(str::trim).find(|line| !line.is_empty()) {
            parts.push(compact_verifier_failure_text(line, 160));
        } else {
            parts.push("verifier_failed".to_string());
        }
    }
    compact_verifier_failure_text(&parts.join(" "), 220)
}

pub(super) fn verifier_failure_count(output: &str) -> Option<usize> {
    let mut summary_total = 0usize;
    let mut saw_summary_count = false;
    for line in output.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        for index in 1..tokens.len() {
            if !verifier_failure_count_word(tokens[index]) {
                continue;
            }
            let Some(count) = verifier_failure_count_number(tokens[index - 1]) else {
                continue;
            };
            saw_summary_count = true;
            summary_total = summary_total.saturating_add(count);
        }
    }
    if saw_summary_count && summary_total > 0 {
        return Some(summary_total);
    }

    let line_count = output
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start().to_ascii_lowercase();
            trimmed.starts_with("failed ")
                || trimmed.starts_with("error ")
                || trimmed.starts_with("error:")
                || trimmed.starts_with("e   ")
                || (trimmed.starts_with("thread '") && trimmed.contains("panicked"))
        })
        .count();
    if line_count > 0 {
        Some(line_count)
    } else if !output.trim().is_empty() {
        Some(1)
    } else {
        None
    }
}

fn verifier_failure_count_word(token: &str) -> bool {
    matches!(
        token
            .trim_matches(|ch: char| !ch.is_ascii_alphabetic())
            .to_ascii_lowercase()
            .as_str(),
        "failed" | "failure" | "failures" | "error" | "errors" | "panic" | "panics"
    )
}

fn verifier_failure_count_number(token: &str) -> Option<usize> {
    let number = token
        .trim_matches(|ch: char| !ch.is_ascii_digit())
        .parse::<usize>()
        .ok()?;
    Some(number)
}

pub(super) fn compact_verifier_failure_text(input: &str, max_chars: usize) -> String {
    let masked = crate::session::feedback::mask_secrets(input);
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, max_chars)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = s.chars().take(max.saturating_sub(1)).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_uses_failed_test_names_not_assertion_values() {
        let output_a = "FAILED tests/test_health.py::test_list_todos_empty - AssertionError\n\
                        E   assert [{'id': 1, 'created_at': '2026-05-19'}] == []\n";
        let output_b = "FAILED tests/test_health.py::test_list_todos_empty - AssertionError\n\
                        E   assert [{'id': 4, 'created_at': '2026-05-20'}] == []\n";

        let sig_a = verifier_failure_signature(
            output_a,
            Some("tests/test_health.py"),
            None,
            verifier_failure_error_kind(output_a).as_deref(),
        );
        let sig_b = verifier_failure_signature(
            output_b,
            Some("tests/test_health.py"),
            None,
            verifier_failure_error_kind(output_b).as_deref(),
        );

        assert_eq!(sig_a, sig_b);
        assert!(sig_a.contains("failed_tests:1"));
        assert!(!sig_a.contains("2026-05"));
    }

    #[test]
    fn signature_includes_exception_type_without_assertion_literals() {
        let output_a = "FAILED tests/test_main.py::TestCreateItem::test_create_item - fastapi.exceptions.ResponseValidationError\n\
                        E   fastapi.exceptions.ResponseValidationError: 1 validation errors:\n\
                        E   {'type': 'float_type', 'input': None}\n";
        let output_b = "FAILED tests/test_main.py::TestCreateItem::test_create_item - AssertionError\n\
                        E   AssertionError: unexpected status\n\
                        E   assert 201 == 200\n";

        let sig_a = verifier_failure_signature(
            output_a,
            Some("tests/test_main.py"),
            None,
            verifier_failure_error_kind(output_a).as_deref(),
        );
        let sig_b = verifier_failure_signature(
            output_b,
            Some("tests/test_main.py"),
            None,
            verifier_failure_error_kind(output_b).as_deref(),
        );

        assert_ne!(sig_a, sig_b);
        assert!(sig_a.contains("failed_tests:1"));
        assert!(sig_a.contains("ResponseValidationError"));
        assert!(sig_b.contains("AssertionError"));
        assert!(!sig_a.contains("float_type"));
        assert!(!sig_b.contains("201"));
    }

    #[test]
    fn verifier_failure_count_parses_generic_failure_summaries() {
        assert_eq!(
            verifier_failure_count(
                "=========================== short test summary info ===========================\n\
                 FAILED tests/test_api.py::test_create - AssertionError\n\
                 ERROR tests/test_api.py::test_import - ImportError\n\
                 ================= 1 failed, 1 error in 0.12s ================="
            ),
            Some(2)
        );
        assert_eq!(
            verifier_failure_count("test result: FAILED. 1718 passed; 6 failed; 0 ignored"),
            Some(6)
        );
        assert_eq!(
            verifier_failure_count("error[E0425]: cannot find value `x` in this scope"),
            Some(1)
        );
    }
}
