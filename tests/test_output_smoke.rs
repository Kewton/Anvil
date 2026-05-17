//! Issue #608 (Phase α-2 / AP-08): integration smoke tests for the
//! `tools::test_output` formatter pipeline.
//!
//! These tests exercise the public 4-function SSOT exposed by the new module
//! (`detect_framework` / `parse_failed_tests` / `trim_to_tail` /
//! `format_summary`) plus the convenience entry point `format_for_tool_result`
//! that bash.rs / auto_test.rs wire up.
//!
//! They pin:
//!   * pytest / cargo test / npm test long-output → FAILED summary + tail
//!     trim (>1000 lines threshold) (VR-08).
//!   * Truncation marker shape (`(truncated, original N lines)`) (VR-08).
//!   * Secret-bearing failed test names never leak raw tokens through the
//!     pipeline (DR4-004 / VR-15).
//!   * Bash metadata (`exit_code=`) stays at the head of the tool result,
//!     FAILED summary appears at the head of the body (VR-08).

use anvil::tools::test_output::{
    FailedTests, MAX_FAILED_TEST_NAME_BYTES, MAX_FAILED_TEST_NAMES, TRIM_TAIL_LINES,
    TRIM_THRESHOLD_LINES, TestFramework, detect_framework, format_for_tool_result, format_summary,
    parse_failed_tests, trim_to_tail,
};

// ----------------------- AP-08 framework hook ----------------------------

#[test]
fn pytest_long_output_summarized_with_failed_header_and_tail_trim() {
    // Construct >1000 lines of synthetic pytest output with the FAILED at the
    // top and a tail that contains a useful debugging line.
    let mut s = String::new();
    s.push_str("FAILED tests/test_user.py::test_login - assertion error\n");
    s.push_str("FAILED tests/test_user.py::test_logout - RuntimeError\n");
    for i in 0..2000 {
        s.push_str(&format!("noisy stack frame {i}\n"));
    }
    s.push_str("=== short test summary info ===\n");
    s.push_str("FAILED tests/test_user.py::test_signup - boom\n");
    let out = format_for_tool_result(&s);
    // FAILED header appears at head of body.
    assert!(out.starts_with("FAILED:"), "output head: {:?}", &out[..50]);
    assert!(out.contains("test_login"));
    assert!(out.contains("test_logout"));
    assert!(out.contains("test_signup"));
    // Truncation marker is present and reports the original line count.
    let original_lines = s.lines().count();
    assert!(
        out.contains(&format!("(truncated, original {original_lines} lines)")),
        "missing truncation marker in: {:?}",
        out
    );
    // Earliest noisy line is trimmed away.
    assert!(!out.contains("noisy stack frame 0\n"));
    // Tail preserved.
    assert!(out.contains("=== short test summary info ===\n"));
}

#[test]
fn cargo_test_long_output_summarized() {
    let mut s = String::new();
    s.push_str("running 3 tests\n");
    s.push_str("test foo::bar ... FAILED\n");
    for i in 0..1200 {
        s.push_str(&format!("noise line {i}\n"));
    }
    s.push_str("test result: FAILED. 1 passed; 1 failed; 0 ignored\n");
    let out = format_for_tool_result(&s);
    assert!(out.starts_with("FAILED: 1 test(s)"), "got: {:?}", out);
    assert!(out.contains("foo::bar"));
    // tail preserved
    assert!(out.contains("test result: FAILED."));
}

#[test]
fn npm_test_long_output_summarized() {
    let mut s = String::new();
    s.push_str(" FAIL src/components/Foo.test.tsx\n");
    s.push_str(" FAIL src/components/Bar.test.tsx\n");
    for i in 0..1300 {
        s.push_str(&format!("line {i}\n"));
    }
    s.push_str("Tests:       2 failed, 5 passed, 7 total\n");
    let out = format_for_tool_result(&s);
    assert!(out.starts_with("FAILED: 2 test(s)"), "got head: {:?}", out);
    assert!(out.contains("Foo.test.tsx"));
    assert!(out.contains("Bar.test.tsx"));
    assert!(out.contains("Tests:       2 failed"));
}

// ----------------------- VR-15 secret masking through pipeline -----------

#[test]
fn failed_test_names_with_kv_secrets_are_masked_in_summary() {
    let s = "FAILED tests/test_a.py::test[api_key=sk_live_supersecretvalue]\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("sk_live_supersecretvalue"),
        "raw kv secret must not appear in formatter output: {:?}",
        out
    );
    assert!(
        out.contains("FAILED: 1 test(s)"),
        "summary header missing: {:?}",
        out
    );
}

#[test]
fn failed_test_names_with_url_credentials_are_masked() {
    let s = "FAILED tests/test_a.py::test[remote=https://user:hunter2@example.com]\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("hunter2"),
        "raw URL userinfo must not appear in formatter output: {:?}",
        out
    );
}

// ----------------------- VR-08 trim boundary --------------------------------

#[test]
fn trim_boundary_at_threshold_passes_through() {
    let s = "x\n".repeat(TRIM_THRESHOLD_LINES);
    let (out, marker) = trim_to_tail(&s, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
    assert!(marker.is_none());
    assert_eq!(out, s);
}

#[test]
fn trim_boundary_above_threshold_trims_to_tail() {
    let mut s = String::new();
    for i in 0..(TRIM_THRESHOLD_LINES + 1) {
        s.push_str(&format!("line_{i}\n"));
    }
    let (out, marker) = trim_to_tail(&s, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
    assert_eq!(marker, Some(TRIM_THRESHOLD_LINES + 1));
    assert!(out.starts_with("... (truncated, original"));
}

// ----------------------- format_summary direct check ------------------------

#[test]
fn format_summary_renders_header_for_named_tests() {
    let failed = FailedTests {
        raw_count: 2,
        names: vec!["foo::bar".into(), "baz::qux".into()],
    };
    let out = format_summary(&failed, "body\n");
    assert!(out.contains("FAILED: 2 test(s)"));
    assert!(out.contains("- foo::bar\n"));
    assert!(out.contains("- baz::qux\n"));
    assert!(out.contains("body\n"));
}

// ----------------------- VR-08 cap enforcement ------------------------------

#[test]
fn parse_caps_at_max_count_to_prevent_unbounded_summary() {
    let mut s = String::new();
    for i in 0..(MAX_FAILED_TEST_NAMES + 50) {
        s.push_str(&format!("test n::t{i} ... FAILED\n"));
    }
    let failed = parse_failed_tests(&s, TestFramework::CargoTest);
    // CB-004: raw_count reports the true total (cap + 50), not the bounded
    // names.len(). CB-005: names allocation is bounded by MAX_FAILED_TEST_NAMES.
    assert_eq!(failed.raw_count, MAX_FAILED_TEST_NAMES + 50);
    assert_eq!(failed.names.len(), MAX_FAILED_TEST_NAMES);
}

#[test]
fn parse_caps_per_name_byte_length() {
    // Use a real per-test FAILED line with a very long signature.
    let huge = "a".repeat(MAX_FAILED_TEST_NAME_BYTES + 200);
    let s = format!("test n::{huge} ... FAILED\n");
    let failed = parse_failed_tests(&s, TestFramework::CargoTest);
    assert_eq!(failed.raw_count, 1);
    assert_eq!(failed.names.len(), 1);
    assert!(failed.names[0].len() <= MAX_FAILED_TEST_NAME_BYTES);
}

// ----------------------- CB-001 header-family redaction --------------------

/// CB-001: failed test outputs that contain Authorization / Cookie /
/// X-API-Key / X-Auth-Token header lines (either in the trimmed body or in
/// the failed test name itself) must not leak the raw credential through
/// the formatter. Header lines are rewritten to `Header: <REDACTED>`.
#[test]
fn format_for_tool_result_redacts_authorization_header_in_body() {
    let s = "FAILED tests/test_a.py::test_x - boom\n\
             curl -H 'Authorization: Bearer secret_bearer_token_value_XYZ' http://x\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("secret_bearer_token_value_XYZ"),
        "raw bearer token must not appear in tool result: {:?}",
        out
    );
    assert!(
        out.contains("Authorization") && out.contains("<REDACTED>"),
        "Authorization redaction marker missing: {:?}",
        out
    );
}

#[test]
fn format_for_tool_result_redacts_cookie_header_in_body() {
    let s = "FAILED tests/test_a.py::test_x - boom\n\
             Cookie: session=eat_my_cookie_token_42; theme=dark\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("eat_my_cookie_token_42"),
        "raw cookie must not appear in tool result: {:?}",
        out
    );
    assert!(out.contains("Cookie") && out.contains("<REDACTED>"));
}

#[test]
fn format_for_tool_result_redacts_x_api_key_in_body() {
    let s = "FAILED tests/test_a.py::test_x - boom\n\
             X-API-Key: live_api_key_value_777\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("live_api_key_value_777"),
        "raw X-API-Key value leaked: {:?}",
        out
    );
    assert!(out.contains("<REDACTED>"));
}

#[test]
fn format_for_tool_result_redacts_x_auth_token_in_body() {
    let s = "FAILED tests/test_a.py::test_x - boom\n\
             X-Auth-Token: live_auth_token_value_888\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("live_auth_token_value_888"),
        "raw X-Auth-Token value leaked: {:?}",
        out
    );
    assert!(out.contains("<REDACTED>"));
}

#[test]
fn format_for_tool_result_redacts_authorization_in_failed_test_name() {
    // The Authorization header appears INSIDE the FAILED test name
    // (parametrized test). It must be redacted in the FAILED summary
    // header / `- name` bullet line, not just the body.
    let s =
        "FAILED tests/test_a.py::test[hdr=Authorization: Bearer leaked_bearer_value_42] - boom\n";
    let out = format_for_tool_result(s);
    assert!(
        !out.contains("leaked_bearer_value_42"),
        "bearer leaked into FAILED summary bullet: {:?}",
        out
    );
    assert!(out.contains("<REDACTED>"));
    assert!(out.starts_with("FAILED: 1 test(s)"));
}

// ----------------------- CB-004 raw count + truncation marker --------------

/// CB-004: 80 failed tests must produce `FAILED: 80 test(s)` (the raw
/// count) plus `... and 30 more` marker (80 - MAX_FAILED_TEST_NAMES).
#[test]
fn format_summary_80_failed_reports_raw_count_with_more_marker() {
    let mut s = String::new();
    for i in 0..80 {
        s.push_str(&format!("test n::t{i} ... FAILED\n"));
    }
    let out = format_for_tool_result(&s);
    assert!(
        out.starts_with("FAILED: 80 test(s)\n"),
        "header should use raw count 80, got: {:?}",
        &out[..out.find('\n').map(|i| i + 1).unwrap_or(out.len())]
    );
    let extra = 80 - MAX_FAILED_TEST_NAMES;
    assert!(
        out.contains(&format!("... and {extra} more\n")),
        "expected `... and {extra} more` marker, got: {:?}",
        out
    );
}

// ----------------------- CB-005 bounded allocation on huge input ------------

/// CB-005: 10_000-line output with 5_000 FAILED lines must NOT produce
/// a 5_000-entry names Vec. raw_count still reports the true total so
/// the summary stays accurate.
#[test]
fn parse_failed_tests_huge_adversarial_input_keeps_names_bounded() {
    let mut s = String::with_capacity(10_000 * 32);
    for i in 0..5_000 {
        s.push_str(&format!("test n::t{i} ... FAILED\n"));
    }
    for i in 0..5_000 {
        s.push_str(&format!("noise line {i}\n"));
    }
    let failed = parse_failed_tests(&s, TestFramework::CargoTest);
    assert_eq!(failed.raw_count, 5_000);
    assert_eq!(failed.names.len(), MAX_FAILED_TEST_NAMES);
}

// ----------------------- detect_framework precedence ------------------------

#[test]
fn detect_framework_pytest_wins_when_signature_present() {
    let s = "FAILED tests/x.py::a - boom\n\
             test result: FAILED. 0 passed; 1 failed\n";
    assert_eq!(detect_framework(s), TestFramework::Pytest);
}

#[test]
fn detect_framework_unknown_for_random_text() {
    let s = "hello world\nplain text here\n";
    assert_eq!(detect_framework(s), TestFramework::Unknown);
}
