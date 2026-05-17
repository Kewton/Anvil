//! Issue #608 (Phase α-2 / AP-08): pure formatter for pytest / cargo test /
//! npm test output.
//!
//! Public API (4-function SSOT):
//!   1. [`detect_framework`] — pytest → cargo → npm precedence (first match).
//!   2. [`parse_failed_tests`] — framework-dispatched regex extraction of
//!      failed test names. UTF-8 safe; results are mask-applied via
//!      [`crate::session::feedback::mask_secrets`] before return.
//!   3. [`trim_to_tail`] — `> threshold` lines → keep `tail` last lines +
//!      truncation marker; `<= threshold` is passed through unchanged.
//!   4. [`format_summary`] — render `FAILED: N` summary header + trimmed body.
//!
//! The combined [`format_for_tool_result`] convenience entry point applies the
//! 4-function pipeline in the contractual order:
//!   1. detect framework (pytest/cargo/npm)
//!   2. parse failed test names (masked)
//!   3. trim (>1000 lines → tail 200 + marker)
//!   4. format summary
//!
//! ## Caps / safety (DR4-004, design §6.2)
//!
//! * Failed test names: max [`MAX_FAILED_TEST_NAMES`] entries, each capped to
//!   [`MAX_FAILED_TEST_NAME_BYTES`] bytes at a UTF-8 char boundary.
//! * Summary lines and trimmed body are constructed from mask-applied
//!   strings — no raw stdout/stderr lands in the summary header.
//! * `trim_to_tail` returns the tail by line, which preserves UTF-8 boundaries
//!   trivially because line splitting is on `\n` (always a char boundary).

use std::sync::OnceLock;

use regex::Regex;

use crate::session::feedback::{mask_header_family, mask_secrets};

/// AP-08 trim threshold (Stage 6 unification, design §4.6 / decision B).
/// Outputs with > [`TRIM_THRESHOLD_LINES`] lines are trimmed; ≤ pass through.
pub const TRIM_THRESHOLD_LINES: usize = 1000;

/// AP-08 tail-preserve size when trimming.
pub const TRIM_TAIL_LINES: usize = 200;

/// Cap on number of failed test names emitted into the FAILED summary header
/// (DR4-004 / design §6.2). Excess entries are silently dropped.
pub const MAX_FAILED_TEST_NAMES: usize = 50;

/// UTF-8 safe byte cap per failed test name (DR4-004 / design §6.2).
pub const MAX_FAILED_TEST_NAME_BYTES: usize = 300;

/// Framework detected from test output. `Unknown` short-circuits to a
/// no-op formatter that returns the input unchanged (after trim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestFramework {
    Pytest,
    CargoTest,
    NpmTest,
    Unknown,
}

// --- detect_framework -------------------------------------------------------

/// AP-08 framework detection. Precedence order (Issue #608, AP-08): pytest →
/// cargo → npm. The first framework whose signature is found in `output`
/// wins; mixed-output cases default to the first match.
///
/// Pure function. No allocations beyond regex shared `OnceLock`.
pub fn detect_framework(output: &str) -> TestFramework {
    if pytest_signature_regex().is_match(output) {
        return TestFramework::Pytest;
    }
    if cargo_test_signature_regex().is_match(output) {
        return TestFramework::CargoTest;
    }
    if npm_test_signature_regex().is_match(output) {
        return TestFramework::NpmTest;
    }
    TestFramework::Unknown
}

/// pytest signature: `FAILED tests/...`, `=== FAILURES ===`,
/// `short test summary info`, `failed in`, ratio lines like `1 failed`.
/// The `regex` crate is built without `unicode-case`, so ASCII case is
/// spelled explicitly.
fn pytest_signature_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^(?:FAILED |=+ FAILURES =+|=+ short test summary info|=+ ERRORS =+|\d+ failed[, ])")
            .expect("valid static pytest signature regex")
    })
}

/// cargo test signature: `test result: FAILED`, `failures:` block,
/// `test foo::bar ... FAILED` lines, `error[E####]:` rust compiler errors.
fn cargo_test_signature_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^(?:test result: FAILED|failures:$|test [^\n]+ \.\.\. FAILED|error\[E\d+\]:)",
        )
        .expect("valid static cargo test signature regex")
    })
}

/// npm/jest/vitest signature: `Tests:`, `FAIL `, `npm ERR!`, `× ` (vitest fail
/// marker). Keep conservative — we only need to distinguish from pytest/cargo.
/// Jest indents its FAIL marker with a single space, so the leading
/// whitespace is optional.
fn npm_test_signature_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*(?:FAIL |Tests:[ \t]+\d|npm ERR!|×[ \t]+)")
            .expect("valid static npm signature regex")
    })
}

// --- parse_failed_tests -----------------------------------------------------

/// AP-08 failed-test extraction result. Carries both the **raw** failure
/// count (uncapped, the real number of FAILED matches in the output) and
/// the bounded, mask-applied list of test names suitable for embedding into
/// the FAILED summary header.
///
/// CB-004 / CB-005:
///   * `raw_count` is the true number of failed tests parsed from `output`,
///     even when `names.len()` is capped at [`MAX_FAILED_TEST_NAMES`].
///   * `names` is bounded by [`MAX_FAILED_TEST_NAMES`] entries collected
///     via a `take(...)`-throttled iterator so an adversarial test output
///     cannot force an unbounded `Vec<String>` allocation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FailedTests {
    /// Real number of failed test matches in the input (NOT capped).
    /// Used as `FAILED: {raw_count} test(s)` in the summary header so
    /// readers see the actual failure count even when names are truncated
    /// for display.
    pub raw_count: usize,
    /// Bounded list of mask-applied failed test names (max
    /// [`MAX_FAILED_TEST_NAMES`] entries, each ≤
    /// [`MAX_FAILED_TEST_NAME_BYTES`] bytes at a UTF-8 boundary).
    pub names: Vec<String>,
}

/// AP-08 failed-test-name extraction. Dispatches on `framework` to a
/// framework-specific regex. Returns a [`FailedTests`] carrying:
///   * `raw_count` — the true failure count (NOT capped);
///   * `names` — a bounded, mask-applied list (≤ [`MAX_FAILED_TEST_NAMES`]).
///
/// CB-001: returned names pass through both [`mask_secrets`] (token / kv /
/// URL-userinfo) AND [`mask_header_family`] (Authorization / Cookie /
/// X-API-Key / X-Auth-Token) so parametrized test names carrying header
/// credentials do not leak raw values into the FAILED summary header or
/// `llm-io.log`.
///
/// CB-005 (DoS hardening): name collection is done with `take(...)` on the
/// `captures_iter` so the underlying `Vec<String>` is allocated at most
/// [`MAX_FAILED_TEST_NAMES`] entries long, regardless of how many FAILED
/// lines the input contains. The raw count is derived from a separate
/// `captures_iter().count()` pass that does NOT materialize strings.
///
/// `TestFramework::Unknown` returns `FailedTests::default()` (zero count,
/// empty list).
pub fn parse_failed_tests(output: &str, framework: TestFramework) -> FailedTests {
    let (raw_count, raw_names) = match framework {
        TestFramework::Pytest => parse_pytest_failed_bounded(output),
        TestFramework::CargoTest => parse_cargo_failed_bounded(output),
        TestFramework::NpmTest => parse_npm_failed_bounded(output),
        TestFramework::Unknown => (0, Vec::new()),
    };
    let names: Vec<String> = raw_names
        .into_iter()
        .map(|name| {
            // Stack both maskers: kv / token / URL-userinfo first, then
            // header-family (Authorization / Cookie / X-API-Key /
            // X-Auth-Token). Then UTF-8 safe cap per entry.
            cap_test_name(&mask_header_family(&mask_secrets(&name)))
        })
        .collect();
    FailedTests { raw_count, names }
}

/// Shared bounded-collection driver for `parse_*_failed_bounded` (CB-005 /
/// DRY): runs `re.captures_iter` twice over `output`, once to count
/// non-empty group-1 matches and once to collect their **trimmed** names
/// capped at [`MAX_FAILED_TEST_NAMES`] entries. Pure / no global state
/// beyond the supplied regex.
fn extract_failed_names_bounded(output: &str, re: &Regex) -> (usize, Vec<String>) {
    let trimmed_group1 = |cap: &regex::Captures<'_>| -> Option<String> {
        cap.get(1).map(|m| m.as_str().trim().to_string())
    };
    let raw_count = re
        .captures_iter(output)
        .filter(|cap| trimmed_group1(cap).map(|s| !s.is_empty()).unwrap_or(false))
        .count();
    let names: Vec<String> = re
        .captures_iter(output)
        .filter_map(|cap| trimmed_group1(&cap))
        .filter(|s| !s.is_empty())
        .take(MAX_FAILED_TEST_NAMES)
        .collect();
    (raw_count, names)
}

/// pytest: `FAILED tests/test_foo.py::test_bar - assertion error` lines.
/// Extract the part between `FAILED ` and ` - ` (or end of line). Returns
/// `(raw_count, bounded_names)` — names list is collected with
/// `take(MAX_FAILED_TEST_NAMES)` so the allocation is bounded even on
/// adversarial input. The raw count uses `captures_iter().count()` (no
/// allocation per match).
fn parse_pytest_failed_bounded(output: &str) -> (usize, Vec<String>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?m)^FAILED\s+([^\s][^\n]*?)(?:\s+-\s+[^\n]*)?$")
            .expect("valid pytest failed regex")
    });
    extract_failed_names_bounded(output, re)
}

/// cargo test: `test foo::bar ... FAILED` lines (when the trailing `... FAILED`
/// marker is at end-of-line). Bounded collection (CB-005). The `[^\s]+`
/// capture cannot contain surrounding whitespace, so the shared trim in
/// [`extract_failed_names_bounded`] is a no-op here.
fn parse_cargo_failed_bounded(output: &str) -> (usize, Vec<String>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?m)^test\s+([^\s]+)\s+\.\.\.\s+FAILED\b").expect("valid cargo failed regex")
    });
    extract_failed_names_bounded(output, re)
}

/// npm/jest/vitest: `FAIL src/components/Foo.test.ts` or `× should do X`.
/// Bounded collection (CB-005). Allow optional leading whitespace.
fn parse_npm_failed_bounded(output: &str) -> (usize, Vec<String>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*(?:FAIL|×)[ \t]+([^\n]+)$").expect("valid npm failed regex")
    });
    extract_failed_names_bounded(output, re)
}

/// UTF-8 safe truncation to at most `MAX_FAILED_TEST_NAME_BYTES`.
fn cap_test_name(name: &str) -> String {
    if name.len() <= MAX_FAILED_TEST_NAME_BYTES {
        return name.to_string();
    }
    let mut end = MAX_FAILED_TEST_NAME_BYTES;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

// --- trim_to_tail -----------------------------------------------------------

/// AP-08 trim: when `output` has > `threshold` lines, return only the last
/// `tail` lines preceded by a single truncation marker line. Otherwise return
/// the input unchanged.
///
/// Returns `(body, Some(original_line_count))` when trimming actually
/// happened, `(body, None)` for the pass-through path.
pub fn trim_to_tail(output: &str, threshold: usize, tail: usize) -> (String, Option<usize>) {
    let line_count = output.lines().count();
    if line_count <= threshold {
        return (output.to_string(), None);
    }
    let skip = line_count.saturating_sub(tail);
    let mut out = String::new();
    out.push_str(&format!(
        "... (truncated, original {line_count} lines) ...\n"
    ));
    for line in output.lines().skip(skip) {
        out.push_str(line);
        out.push('\n');
    }
    (out, Some(line_count))
}

// --- format_summary ---------------------------------------------------------

/// AP-08 summary header builder. Renders:
///
/// ```text
/// FAILED: N test(s)
/// - name_1
/// - name_2
/// ... and M more
/// ```
///
/// `failed` is expected to be the mask-applied / capped struct from
/// [`parse_failed_tests`]. The `trimmed` body is appended after a blank line.
///
/// CB-004: the header count reports the **raw** failure count
/// (`failed.raw_count`), NOT the capped `failed.names.len()`, so the
/// summary cannot under-report the real number of failures. When
/// `raw_count > names.len()` we append a `... and M more` marker so
/// readers know names were truncated for display.
///
/// Returns the combined string. Pure / safe to call with any UTF-8 input.
pub fn format_summary(failed: &FailedTests, trimmed: &str) -> String {
    let raw = failed.raw_count;
    let shown = failed.names.len();
    let mut out = String::new();
    if raw > 0 {
        out.push_str(&format!("FAILED: {raw} test(s)\n"));
        for name in &failed.names {
            out.push_str("- ");
            out.push_str(name);
            out.push('\n');
        }
        if raw > shown {
            let more = raw - shown;
            out.push_str(&format!("... and {more} more\n"));
        }
        out.push('\n');
    }
    out.push_str(trimmed);
    out
}

// --- combined convenience ---------------------------------------------------

/// AP-08 combined entry point. Applies the 4-function pipeline in design
/// order:
///   1. [`detect_framework`]
///   2. [`parse_failed_tests`] (mask-applied with both
///      [`mask_secrets`] and [`mask_header_family`])
///   3. [`trim_to_tail`] with [`TRIM_THRESHOLD_LINES`] / [`TRIM_TAIL_LINES`]
///   4. [`mask_secrets`] + [`mask_header_family`] applied to the trimmed
///      body (DR4-004 / VR-15 / CB-001 — the body still carries raw
///      stdout/stderr text after trim, so a token or Authorization /
///      Cookie / X-API-Key / X-Auth-Token header line in a tail line
///      could leak into the tool result without this pass).
///   5. [`format_summary`]
///
/// Bash metadata (`exit_code=` / `timed_out=` / `interrupted=` prefixes) is
/// the caller's responsibility — `bash.rs` prepends them BEFORE invoking
/// this function so the metadata stays at the head of the tool result.
///
/// This function does NOT apply a byte cap; the caller (bash.rs /
/// auto_test.rs) is expected to apply `truncate_output(_, 20_000)` or
/// `MAX_OUTPUT_BYTES` AFTER this transform (design §4.6 ordering invariant).
pub fn format_for_tool_result(combined: &str) -> String {
    let framework = detect_framework(combined);
    let failed_tests = parse_failed_tests(combined, framework);
    let (trimmed, _) = trim_to_tail(combined, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
    // CB-001: stack mask_secrets (token / kv / URL-userinfo) with
    // mask_header_family (Authorization / Cookie / X-API-Key /
    // X-Auth-Token). The header-family pass does NOT apply the 4096-byte
    // cap of `redact_verifier_command_for_storage` because the body is
    // already line-bounded by `trim_to_tail` and the outer 20_000-byte
    // cap is applied by the caller (bash.rs / auto_test.rs).
    let masked_body = mask_header_family(&mask_secrets(&trimmed));
    format_summary(&failed_tests, &masked_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_framework (≥ 3 cases) ----------------------------------------

    #[test]
    fn detect_pytest_signature() {
        let s = "============ test session starts ============\n\
                 FAILED tests/test_foo.py::test_bar - AssertionError\n\
                 =========== short test summary info ===========\n";
        assert_eq!(detect_framework(s), TestFramework::Pytest);
    }

    #[test]
    fn detect_cargo_test_signature() {
        let s = "running 3 tests\n\
                 test foo::bar ... FAILED\n\
                 test result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        assert_eq!(detect_framework(s), TestFramework::CargoTest);
    }

    #[test]
    fn detect_npm_test_signature() {
        let s = " FAIL src/components/Foo.test.tsx\n\
                 Tests:       1 failed, 0 passed, 1 total\n\
                 npm ERR! Test failed.\n";
        assert_eq!(detect_framework(s), TestFramework::NpmTest);
    }

    #[test]
    fn detect_unknown_signature_for_empty_input() {
        assert_eq!(detect_framework(""), TestFramework::Unknown);
        assert_eq!(detect_framework("hello world"), TestFramework::Unknown);
    }

    /// Precedence: pytest first when multiple signatures appear (Issue #608
    /// AP-08 precedence).
    #[test]
    fn detect_framework_precedence_pytest_first() {
        let mixed = "FAILED tests/test_foo.py::test_a - boom\n\
                     test result: FAILED. 0 passed; 1 failed\n";
        assert_eq!(detect_framework(mixed), TestFramework::Pytest);
    }

    // --- parse_failed_tests (≥ 6 cases) --------------------------------------

    #[test]
    fn parse_pytest_failed_basic() {
        let s = "FAILED tests/test_foo.py::test_bar - AssertionError\n\
                 FAILED tests/test_baz.py::test_qux - RuntimeError\n";
        let failed = parse_failed_tests(s, TestFramework::Pytest);
        assert_eq!(failed.raw_count, 2);
        assert_eq!(failed.names.len(), 2);
        assert!(failed.names[0].contains("test_bar"));
        assert!(failed.names[1].contains("test_qux"));
    }

    #[test]
    fn parse_pytest_failed_no_reason_suffix() {
        let s = "FAILED tests/test_a.py::test_x\n";
        let failed = parse_failed_tests(s, TestFramework::Pytest);
        assert_eq!(failed.raw_count, 1);
        assert_eq!(failed.names, vec!["tests/test_a.py::test_x".to_string()]);
    }

    #[test]
    fn parse_cargo_failed_basic() {
        let s = "running 2 tests\n\
                 test foo::bar ... FAILED\n\
                 test baz::qux ... ok\n";
        let failed = parse_failed_tests(s, TestFramework::CargoTest);
        assert_eq!(failed.raw_count, 1);
        assert_eq!(failed.names, vec!["foo::bar".to_string()]);
    }

    #[test]
    fn parse_cargo_failed_multiple() {
        let s = "test a::b ... FAILED\n\
                 test c::d ... FAILED\n\
                 test e::f ... ok\n";
        let failed = parse_failed_tests(s, TestFramework::CargoTest);
        assert_eq!(failed.raw_count, 2);
        assert_eq!(failed.names, vec!["a::b".to_string(), "c::d".to_string()]);
    }

    #[test]
    fn parse_npm_failed_basic() {
        let s = " FAIL src/components/Foo.test.tsx\n\
                 FAIL src/components/Bar.test.tsx\n";
        let failed = parse_failed_tests(s, TestFramework::NpmTest);
        assert_eq!(failed.raw_count, 2);
        assert_eq!(failed.names.len(), 2);
        assert!(failed.names[0].contains("Foo.test.tsx"));
        assert!(failed.names[1].contains("Bar.test.tsx"));
    }

    #[test]
    fn parse_unknown_framework_returns_empty() {
        let failed = parse_failed_tests("hello", TestFramework::Unknown);
        assert_eq!(failed.raw_count, 0);
        assert!(failed.names.is_empty());
    }

    // --- trim_to_tail (≥ 3 boundary cases) -----------------------------------

    #[test]
    fn trim_passes_short_input_through() {
        let s = "line1\nline2\nline3\n";
        let (out, n) = trim_to_tail(s, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
        assert_eq!(out, s);
        assert!(n.is_none());
    }

    #[test]
    fn trim_exactly_at_threshold_passes_through() {
        // line count == TRIM_THRESHOLD_LINES → pass-through (> only).
        let lines = "x\n".repeat(TRIM_THRESHOLD_LINES);
        let (out, n) = trim_to_tail(&lines, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
        assert_eq!(out, lines);
        assert!(n.is_none());
    }

    #[test]
    fn trim_above_threshold_keeps_tail_plus_marker() {
        // 1001 unique lines → trim should keep last TRIM_TAIL_LINES + marker.
        let mut s = String::new();
        for i in 0..1001 {
            s.push_str(&format!("line_{i}\n"));
        }
        let (out, n) = trim_to_tail(&s, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
        assert_eq!(n, Some(1001));
        assert!(out.starts_with("... (truncated, original 1001 lines) ..."));
        // Last line preserved.
        assert!(out.contains("line_1000\n"));
        // Earliest line trimmed away.
        assert!(!out.contains("line_0\n"));
    }

    #[test]
    fn trim_large_input_keeps_only_tail() {
        let mut s = String::new();
        for i in 0..5000 {
            s.push_str(&format!("line_{i}\n"));
        }
        let (out, n) = trim_to_tail(&s, TRIM_THRESHOLD_LINES, TRIM_TAIL_LINES);
        assert_eq!(n, Some(5000));
        // The result is marker + last 200 lines.
        let body_lines: Vec<&str> = out.lines().collect();
        assert_eq!(body_lines.len(), 1 + TRIM_TAIL_LINES);
        assert!(body_lines[0].starts_with("... (truncated"));
        assert_eq!(body_lines[1], "line_4800");
        assert_eq!(*body_lines.last().unwrap(), "line_4999");
    }

    // --- format_summary (≥ 3 cases) ------------------------------------------

    #[test]
    fn format_summary_empty_failed_renders_only_body() {
        let body = "hello\n";
        let out = format_summary(&FailedTests::default(), body);
        assert_eq!(out, "hello\n");
    }

    #[test]
    fn format_summary_one_failed_renders_header_and_body() {
        let failed = FailedTests {
            raw_count: 1,
            names: vec!["foo::bar".to_string()],
        };
        let out = format_summary(&failed, "body\n");
        assert!(out.starts_with("FAILED: 1 test(s)\n- foo::bar\n"));
        assert!(out.ends_with("body\n"));
    }

    #[test]
    fn format_summary_many_failed_renders_header_count() {
        let failed = FailedTests {
            raw_count: 3,
            names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        let out = format_summary(&failed, "");
        assert!(out.starts_with("FAILED: 3 test(s)\n- a\n- b\n- c\n"));
    }

    /// CB-004: when names are capped but raw_count is higher, the header
    /// reports the raw count and a `... and N more` marker is rendered.
    #[test]
    fn format_summary_uses_raw_count_with_truncation_marker() {
        let names: Vec<String> = (0..MAX_FAILED_TEST_NAMES)
            .map(|i| format!("t{i}"))
            .collect();
        let failed = FailedTests {
            raw_count: 80,
            names,
        };
        let out = format_summary(&failed, "body\n");
        // Header shows REAL count (80), not the capped 50.
        assert!(
            out.starts_with("FAILED: 80 test(s)\n"),
            "header should use raw_count=80, got: {:?}",
            &out[..out.find('\n').map(|i| i + 1).unwrap_or(out.len())]
        );
        // Marker is rendered for the truncated names.
        let extra = 80 - MAX_FAILED_TEST_NAMES;
        assert!(
            out.contains(&format!("... and {extra} more\n")),
            "truncation marker missing: {:?}",
            out
        );
    }

    // --- mask integration (DR4-004) -----------------------------------------

    /// Failed test names containing secrets must be redacted before they
    /// appear in the FAILED summary (design §6 / VR-15). `mask_secrets`
    /// keys on word boundaries, so we exercise the kv-shape (`api_key=...`)
    /// path that hits the kv_secret_regex SSOT.
    #[test]
    fn parse_masks_secrets_in_failed_names() {
        let leak = "FAILED tests/test_foo.py::test[api_key=sk_live_supersecretvalue]\n";
        let failed = parse_failed_tests(leak, TestFramework::Pytest);
        assert_eq!(failed.raw_count, 1);
        assert_eq!(failed.names.len(), 1);
        assert!(
            !failed.names[0].contains("sk_live_supersecretvalue"),
            "raw token must not leak through parse_failed_tests: {:?}",
            failed.names[0]
        );
        assert!(failed.names[0].contains("***"));
    }

    /// CB-001: failed test names carrying header-family credentials
    /// (Authorization / Cookie / X-API-Key / X-Auth-Token) must be redacted
    /// through `mask_header_family` so the raw value never lands in the
    /// FAILED summary.
    #[test]
    fn parse_masks_header_family_in_failed_names() {
        let leak = "FAILED tests/test_a.py::test[hdr=Authorization: Bearer abc123def456ghi]\n";
        let failed = parse_failed_tests(leak, TestFramework::Pytest);
        assert_eq!(failed.raw_count, 1);
        assert!(
            !failed.names[0].contains("abc123def456ghi"),
            "raw bearer token leaked: {:?}",
            failed.names[0]
        );
        assert!(
            failed.names[0].contains("<REDACTED>"),
            "header redaction marker missing: {:?}",
            failed.names[0]
        );
    }

    // --- combined pipeline (smoke) ------------------------------------------

    #[test]
    fn format_for_tool_result_pytest_short() {
        let s = "FAILED tests/test_a.py::test_x - AssertionError\n";
        let out = format_for_tool_result(s);
        assert!(out.starts_with("FAILED: 1 test(s)"));
        assert!(out.contains("test_x"));
    }

    #[test]
    fn format_for_tool_result_unknown_passes_through() {
        let s = "just normal output\n";
        let out = format_for_tool_result(s);
        // No FAILED header (Unknown framework → empty failed list).
        assert!(!out.starts_with("FAILED:"));
        assert_eq!(out, s);
    }

    #[test]
    fn cap_test_name_truncates_long_string() {
        let long = "a".repeat(MAX_FAILED_TEST_NAME_BYTES + 100);
        let capped = cap_test_name(&long);
        assert!(capped.len() <= MAX_FAILED_TEST_NAME_BYTES);
    }

    #[test]
    fn parse_failed_tests_caps_at_max_count() {
        let mut s = String::new();
        for i in 0..(MAX_FAILED_TEST_NAMES + 10) {
            s.push_str(&format!("test n::t{i} ... FAILED\n"));
        }
        let failed = parse_failed_tests(&s, TestFramework::CargoTest);
        // CB-004: raw_count reports the true total (MAX + 10), not the cap.
        assert_eq!(failed.raw_count, MAX_FAILED_TEST_NAMES + 10);
        // CB-005: names allocation is bounded at MAX_FAILED_TEST_NAMES.
        assert_eq!(failed.names.len(), MAX_FAILED_TEST_NAMES);
    }

    /// CB-004 regression: 80 failed tests produce header `FAILED: 80 test(s)`
    /// (raw count) with `... and 30 more` marker (80 - 50 cap = 30).
    #[test]
    fn parse_failed_tests_80_fails_reports_raw_count_in_header() {
        let mut s = String::new();
        for i in 0..80 {
            s.push_str(&format!("test n::t{i} ... FAILED\n"));
        }
        let failed = parse_failed_tests(&s, TestFramework::CargoTest);
        assert_eq!(failed.raw_count, 80);
        assert_eq!(failed.names.len(), MAX_FAILED_TEST_NAMES);
        let out = format_summary(&failed, "");
        assert!(out.starts_with("FAILED: 80 test(s)\n"));
        assert!(
            out.contains("... and 30 more\n"),
            "expected `... and 30 more`, got: {:?}",
            out
        );
    }

    /// CB-005 (DoS resistance): with very large adversarial output, the
    /// `names` Vec is still bounded at MAX_FAILED_TEST_NAMES. We sanity-check
    /// that 10_000 lines / 5_000 fails do not produce a 5_000-entry Vec.
    #[test]
    fn parse_failed_tests_huge_input_keeps_names_bounded() {
        let mut s = String::with_capacity(10_000 * 30);
        for i in 0..5_000 {
            s.push_str(&format!("test n::t{i} ... FAILED\n"));
        }
        for i in 0..5_000 {
            s.push_str(&format!("noise line {i}\n"));
        }
        let failed = parse_failed_tests(&s, TestFramework::CargoTest);
        assert_eq!(failed.raw_count, 5_000);
        assert_eq!(
            failed.names.len(),
            MAX_FAILED_TEST_NAMES,
            "names should remain bounded at MAX_FAILED_TEST_NAMES even on 5000-fail input"
        );
    }
}
