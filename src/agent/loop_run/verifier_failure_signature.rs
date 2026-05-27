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
