use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::agent::prompting::load_project_instructions;
use crate::session::feedback::FeedbackKind;

/// Maximum bytes of combined stdout+stderr the auto_test path keeps in its
/// `AutoTestResult.output`. Issue #459 / DR2-009 keeps this private to the
/// auto_test path (Tester uses session/feedback excerpt cap instead).
pub(super) const MAX_OUTPUT_BYTES: usize = 12_000;

// Marker patterns shared between `classify_auto_test` and the count
// heuristics introduced in #457. Keeping these as the SSOT prevents drift
// where classification matches but counts return None (or vice versa).
// Naming convention: `MARKER_<LANG>_<KIND>` (with `_<SCOPE>` suffix when
// disambiguation is needed, e.g. `MARKER_PYTEST_FAILED_SUMMARY`).
const MARKER_CARGO_COMPILE_ERROR: &str = "error[";
const MARKER_NPM_TSC_ERROR: &str = "error ts";
const MARKER_CARGO_TEST_FAILED: &str = "test result: failed";
const MARKER_PYTEST_FAILED_SUMMARY: &str = " failed";

/// Build vs Test classification for an auto_test plan. Used by the
/// FeedbackFrame generator to decide between `BuildPass`/`TestPass` on
/// success, and to refine `CompileError`/`TestFailure`/etc. on failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoTestKind {
    Build,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoTestPlan {
    pub command: String,
    pub reason: String,
}

impl AutoTestPlan {
    pub(super) fn auto_test_kind(&self) -> AutoTestKind {
        infer_auto_test_kind(&self.command)
    }
}

fn infer_auto_test_kind(command: &str) -> AutoTestKind {
    let lower = command.trim().to_ascii_lowercase();
    if lower.starts_with("cargo build")
        || lower.starts_with("npm run build")
        || lower.starts_with("pnpm build")
        || lower.starts_with("yarn build")
        || lower.starts_with("python3 -m py_compile")
        || lower.starts_with("python -m py_compile")
        || lower.starts_with("make build")
        || lower.starts_with("cargo check")
    {
        AutoTestKind::Build
    } else {
        AutoTestKind::Test
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoTestResult {
    pub command: String,
    pub passed: bool,
    pub output: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AutoTestRunner;

impl AutoTestRunner {
    pub(super) fn detect(work_root: &Path, changed_files: &[String]) -> Option<AutoTestPlan> {
        if let Some(plan) = detect_project_instruction_test(work_root, changed_files) {
            return Some(plan);
        }
        if work_root.join("Cargo.toml").is_file() {
            return Some(AutoTestPlan {
                command: "cargo test".to_string(),
                reason: "Cargo.toml detected".to_string(),
            });
        }
        if work_root.join("package.json").is_file() {
            let has_test = package_json_has_test_script(work_root);
            let package = std::fs::read_to_string(work_root.join("package.json")).ok()?;
            let has_build = package_json_has_build_script(&package);
            if has_test && has_build {
                return Some(AutoTestPlan {
                    command: node_verifier_command(work_root, "npm test && npm run build"),
                    reason: "package.json test and build scripts detected".to_string(),
                });
            }
            if has_test {
                return Some(AutoTestPlan {
                    command: node_verifier_command(work_root, "npm test"),
                    reason: "package.json test script detected".to_string(),
                });
            }
            if has_build {
                return Some(AutoTestPlan {
                    command: node_verifier_command(work_root, "npm run build"),
                    reason: "package.json build script detected".to_string(),
                });
            }
        }
        if has_python_surface(work_root, changed_files) {
            if work_root.join("pytest.ini").is_file()
                || work_root.join("pyproject.toml").is_file()
                || work_root.join("tests").is_dir()
                || changed_files.iter().any(|path| {
                    path.starts_with("tests/")
                        || path.ends_with("_test.py")
                        || path.starts_with("test_")
                })
            {
                return Some(AutoTestPlan {
                    command: "python3 -m pytest".to_string(),
                    reason: "Python tests detected".to_string(),
                });
            }
            if let Some(script) = first_python_script(changed_files) {
                return Some(AutoTestPlan {
                    command: format!("python3 -m py_compile {}", shell_quote(&script)),
                    reason: "Python implementation detected without pytest".to_string(),
                });
            }
        }
        None
    }

    pub(super) fn run(work_root: &Path, plan: &AutoTestPlan) -> Result<AutoTestResult, String> {
        let output = Command::new("sh")
            .arg("-lc")
            .arg(&plan.command)
            .current_dir(work_root)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("failed to run auto test command: {err}"))?;
        // Always lossy-decode: invalid UTF-8 must not panic FeedbackFrame
        // creation downstream (Issue #450 / R5).
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let mut combined = String::new();
        combined.push_str(&stdout);
        if !stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }
        Ok(AutoTestResult {
            command: plan.command.clone(),
            passed: output.status.success(),
            output: truncate(&combined, MAX_OUTPUT_BYTES),
            exit_code: output.status.code(),
            stdout,
            stderr,
        })
    }
}

fn node_verifier_command(work_root: &Path, command: &str) -> String {
    let body = if work_root.join("node_modules").is_dir() {
        command.to_string()
    } else {
        format!("npm install && {command}")
    };
    format!("export CI=1 NUXT_IGNORE_LOCK=1; {body}")
}

/// Classify an auto_test outcome into the appropriate `FeedbackKind`.
/// This is a pure function over the plan + result text, separated so
/// AC1/AC2 (Issue #450) can be tested without spawning processes.
pub(super) fn classify_auto_test(plan: &AutoTestPlan, result: &AutoTestResult) -> FeedbackKind {
    if result.passed {
        return match plan.auto_test_kind() {
            AutoTestKind::Build => FeedbackKind::BuildPass,
            AutoTestKind::Test => FeedbackKind::TestPass,
        };
    }

    // Failure: look at output for category-specific markers.
    let combined = combined_output_for_classify(result);
    let lower = combined.to_ascii_lowercase();

    // Compile / build errors first.
    if lower.contains(MARKER_CARGO_COMPILE_ERROR)
        || lower.contains("could not compile")
        || lower.contains(MARKER_NPM_TSC_ERROR)
        || lower.contains("syntaxerror")
        || lower.contains("syntax error")
    {
        return FeedbackKind::CompileError;
    }

    // Type errors (mypy / pyright / tsc).
    if (lower.contains("error: ") && lower.contains("incompatible types"))
        || lower.contains("error: argument") && lower.contains("incompatible type")
        || lower.contains("type error")
        || lower.contains("typeerror:")
    {
        return FeedbackKind::TypeError;
    }

    // Lint failures (clippy / eslint / ruff).
    if lower.contains("clippy::")
        || (lower.contains("eslint") && lower.contains("error"))
        || lower.contains("ruff")
    {
        return FeedbackKind::LintFailure;
    }

    // Test failures (pytest / cargo test).
    if lower.contains("failed")
        || lower.contains("assert")
        || lower.contains(MARKER_CARGO_TEST_FAILED)
        || lower.contains("failing")
    {
        return FeedbackKind::TestFailure;
    }

    FeedbackKind::UnknownFailure
}

fn combined_output_for_classify(result: &AutoTestResult) -> String {
    if !result.stdout.is_empty() || !result.stderr.is_empty() {
        let mut s = String::new();
        s.push_str(&result.stdout);
        if !result.stderr.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&result.stderr);
        }
        s
    } else {
        result.output.clone()
    }
}

// Issue #457: heuristic count of compile errors observed in
// `AutoTestResult.stdout` / `stderr`. Plan-agnostic by design (DR1-001 of
// the design policy): the caller in turn.rs decides whether the count is
// relevant for the current `AutoTestKind` × `passed` combination. Returns
// `None` when no marker matches, when the count would overflow `i32::MAX`,
// or when the result genuinely has no compile-error signal.
pub(super) fn count_compile_errors(result: &AutoTestResult) -> Option<usize> {
    let combined = combined_output_for_classify(result);
    let lower = combined.to_ascii_lowercase();
    let cargo = count_marker_lines(&lower, MARKER_CARGO_COMPILE_ERROR);
    let npm = count_marker_lines(&lower, MARKER_NPM_TSC_ERROR);
    let total = cargo.saturating_add(npm);
    if total == 0 {
        return None;
    }
    if total > i32::MAX as usize {
        return None;
    }
    Some(total)
}

// Issue #457: heuristic count of test failures observed in
// `AutoTestResult.stdout` / `stderr`. Recognises:
// - cargo: `test result: FAILED. <pass> passed; <N> failed`
// - pytest: `=== <N> failed, ... ===` summary line
// Returns `None` when no recognised summary appears or parsing fails.
pub(super) fn count_test_failures(result: &AutoTestResult) -> Option<usize> {
    let combined = combined_output_for_classify(result);
    let lower = combined.to_ascii_lowercase();

    if let Some(n) = parse_cargo_failed(&lower) {
        return cap_count(n);
    }
    if let Some(n) = parse_pytest_failed(&lower) {
        return cap_count(n);
    }
    None
}

fn count_marker_lines(lower: &str, marker: &str) -> usize {
    if marker.is_empty() {
        return 0;
    }
    lower.lines().filter(|line| line.contains(marker)).count()
}

fn cap_count(n: usize) -> Option<usize> {
    if n > i32::MAX as usize { None } else { Some(n) }
}

// Parse `test result: FAILED. <pass> passed; <N> failed` (cargo test).
fn parse_cargo_failed(lower: &str) -> Option<usize> {
    let idx = lower.find(MARKER_CARGO_TEST_FAILED)?;
    let tail = &lower[idx..];
    parse_first_failed_count(tail)
}

// Parse pytest summary `==== <N> failed[, ...] ====`.
fn parse_pytest_failed(lower: &str) -> Option<usize> {
    for line in lower.lines() {
        let trimmed = line.trim_matches('=').trim();
        if !trimmed.contains(MARKER_PYTEST_FAILED_SUMMARY.trim_start()) {
            continue;
        }
        if let Some(n) = parse_first_failed_count(trimmed) {
            return Some(n);
        }
    }
    None
}

// Find a numeric token immediately preceding the literal `failed` in the
// supplied text. Plan-agnostic and panic-free.
fn parse_first_failed_count(text: &str) -> Option<usize> {
    let needle = MARKER_PYTEST_FAILED_SUMMARY.trim_start();
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(needle) {
        let abs = search_from + rel;
        let prefix = text[..abs].trim_end();
        let digit_byte_count = prefix.bytes().rev().take_while(u8::is_ascii_digit).count();
        let digits = &prefix[prefix.len() - digit_byte_count..];
        if let Ok(n) = digits.parse::<usize>() {
            return Some(n);
        }
        search_from = abs + needle.len();
    }
    None
}

fn detect_project_instruction_test(
    work_root: &Path,
    changed_files: &[String],
) -> Option<AutoTestPlan> {
    let instructions = load_project_instructions(work_root, work_root)?;
    let command =
        extract_safe_preferred_command(&instructions.global_content, work_root, changed_files)?;
    Some(AutoTestPlan {
        command,
        reason: "ANVIL.md preferred command".to_string(),
    })
}

fn extract_safe_preferred_command(
    text: &str,
    work_root: &Path,
    changed_files: &[String],
) -> Option<String> {
    for command in backtick_commands(text) {
        let normalized = command.trim().to_ascii_lowercase();
        if changed_files.iter().any(|path| path.ends_with(".py"))
            && (normalized.starts_with("python3 ") || normalized.starts_with("python "))
            && command_references_existing_local_file(&command, work_root)
        {
            return Some(command);
        }
        if work_root.join("Cargo.toml").is_file()
            && matches!(
                normalized.as_str(),
                "cargo test" | "cargo clippy --all-targets -- -d warnings" | "cargo check"
            )
        {
            return Some(command);
        }
        if work_root.join("package.json").is_file()
            && matches!(normalized.as_str(), "npm test" | "npm run build")
        {
            return Some(command);
        }
    }
    None
}

fn backtick_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut rest = text;
    while let Some((_, after_open)) = rest.split_once('`') {
        let Some((candidate, after_close)) = after_open.split_once('`') else {
            break;
        };
        let trimmed = candidate.trim();
        if !trimmed.is_empty() && trimmed.len() <= 200 {
            commands.push(trimmed.to_string());
        }
        rest = after_close;
    }
    commands
}

fn command_references_existing_local_file(command: &str, work_root: &Path) -> bool {
    command
        .split_whitespace()
        .filter(|token| token.ends_with(".py") || token.ends_with(".csv"))
        .map(|token| token.trim_matches(['"', '\'', '`']))
        .all(|token| {
            !token.contains('/')
                && !token.contains('\\')
                && !token.starts_with('.')
                && work_root.join(token).is_file()
        })
}

/// Returns true if the workspace surface looks like a Python project (a
/// changed `.py` file, `pyproject.toml`, or `requirements.txt`). Promoted
/// to `pub(super)` for Issue #459 so Tester can reuse the same heuristic
/// (DR1-001).
pub(super) fn has_python_surface(work_root: &Path, changed_files: &[String]) -> bool {
    changed_files.iter().any(|path| path.ends_with(".py"))
        || work_root.join("pyproject.toml").is_file()
        || work_root.join("requirements.txt").is_file()
}

/// Returns the first non-test `.py` path among `changed_files`. Promoted
/// to `pub(super)` for Issue #459 (DR1-001 / Tester::Python::surface_files
/// derivation).
pub(super) fn first_python_script(changed_files: &[String]) -> Option<PathBuf> {
    changed_files
        .iter()
        .find(|path| {
            path.ends_with(".py") && !path.starts_with("tests/") && !path.starts_with("test_")
        })
        .map(PathBuf::from)
}

/// Returns true when `work_root/Cargo.toml` exists and the workspace has
/// no `tests/` directory. Issue #459 / DR1-001: Tester treats a
/// "Cargo.toml without tests/" workspace as a candidate for smoke-test
/// generation (the inverse of what auto_test handles). Currently only
/// referenced by Tester (Phase 1b) and unit tests; `dead_code` allowed
/// for the duration of Phase 1a so clippy stays clean before the Tester
/// module lands.
#[allow(dead_code)]
pub(super) fn has_cargo_manifest(work_root: &Path) -> bool {
    let manifest = work_root.join("Cargo.toml");
    if !manifest.is_file() || work_root.join("tests").is_dir() {
        return false;
    }
    // CB-006 (Issue #459): a Cargo.toml that defines an explicit `[[test]]`
    // target section is an explicit test verifier — Tester must not generate
    // a smoke test in that case. Use a deliberately small line-level scan
    // (no TOML parser dep, mirroring `package_json_has_test_script`'s style)
    // and skip lines whose first non-whitespace char is `#` (comment) so a
    // documented `[[test]]` reference inside a comment does not trigger.
    if let Ok(raw) = std::fs::read_to_string(&manifest)
        && cargo_manifest_has_test_target_section(&raw)
    {
        return false;
    }
    true
}

/// Returns true when `Cargo.toml` source defines at least one `[[test]]`
/// section header. Cheap line-level scan: `[[test]]` must appear as the first
/// non-whitespace token of a non-comment line. CB-006 (Issue #459).
pub(super) fn cargo_manifest_has_test_target_section(raw: &str) -> bool {
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("[[test]]") {
            return true;
        }
    }
    false
}

/// Returns true when `work_root/package.json` defines a `scripts.test`
/// entry. Issue #459 / DR1-001: Tester takes the *false* branch (no test
/// script defined) as a candidate for smoke-test generation. The check is
/// substring-based on the raw JSON to mirror auto_test's existing
/// `package.contains("\"test\"")` heuristic — a deliberate match for
/// DR2-001 (no AST parsing dependency).
pub(super) fn package_json_has_test_script(work_root: &Path) -> bool {
    let path = work_root.join("package.json");
    let Ok(package) = std::fs::read_to_string(&path) else {
        return false;
    };
    package.contains("\"test\"")
}

fn package_json_has_build_script(package: &str) -> bool {
    package.contains("\"build\"")
}

/// Single-quote a path for safe inclusion in a `sh -lc` command line.
/// Promoted to `pub(super)` for Issue #459 so Tester's shell-template
/// builder can quote LLM-supplied filenames identically (DR1-001 /
/// DR2-008).
pub(super) fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

/// Returns true when `ANVIL_NO_AUTO_TEST` is set to a non-empty value.
/// Follows the same closure DI pattern as `case_record_disabled` (session/case_record.rs).
pub fn auto_test_disabled<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env("ANVIL_NO_AUTO_TEST"), Ok(v) if !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_cargo_test_first() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let plan = AutoTestRunner::detect(dir.path(), &[]).expect("plan");
        assert_eq!(plan.command, "cargo test");
    }

    #[test]
    fn detects_python_py_compile_without_tests() {
        let dir = tempdir().expect("tempdir");
        let plan =
            AutoTestRunner::detect(dir.path(), &["scripts/report.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m py_compile 'scripts/report.py'");
    }

    #[test]
    fn detects_pytest_when_tests_exist() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        let plan = AutoTestRunner::detect(dir.path(), &["app.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m pytest");
    }

    #[test]
    fn detects_safe_anvil_python_command() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `python3 project_csv_tool.py example.csv`\n",
        )
        .expect("anvil");
        std::fs::write(dir.path().join("project_csv_tool.py"), "print('ok')\n").expect("py");
        std::fs::write(dir.path().join("example.csv"), "Category,Amount\nA,1\n").expect("csv");

        let plan =
            AutoTestRunner::detect(dir.path(), &["project_csv_tool.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 project_csv_tool.py example.csv");
    }

    #[test]
    fn detects_node_test_and_build_with_install_when_dependencies_missing() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node smoke.mjs","build":"vite build"}}"#,
        )
        .expect("package");

        let plan = AutoTestRunner::detect(dir.path(), &[]).expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm test && npm run build"
        );
        assert_eq!(plan.reason, "package.json test and build scripts detected");
    }

    #[test]
    fn detects_node_test_and_build_without_install_when_dependencies_present() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("node_modules")).expect("node_modules");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node smoke.mjs","build":"vite build"}}"#,
        )
        .expect("package");

        let plan = AutoTestRunner::detect(dir.path(), &[]).expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm test && npm run build"
        );
    }

    #[test]
    fn ignores_unsafe_anvil_command() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `rm -rf .`\n",
        )
        .expect("anvil");
        let plan = AutoTestRunner::detect(dir.path(), &["tool.py".to_string()]);
        assert!(plan.is_some());
        assert_ne!(plan.unwrap().command, "rm -rf .");
    }

    // --- Issue #450 AC1 / AC2 / NoVerifierAvailable -----------------------

    fn cargo_plan() -> AutoTestPlan {
        AutoTestPlan {
            command: "cargo test".to_string(),
            reason: "test".to_string(),
        }
    }

    fn pytest_plan() -> AutoTestPlan {
        AutoTestPlan {
            command: "python3 -m pytest".to_string(),
            reason: "test".to_string(),
        }
    }

    fn build_plan() -> AutoTestPlan {
        AutoTestPlan {
            command: "cargo build".to_string(),
            reason: "build".to_string(),
        }
    }

    fn make_result(
        plan: &AutoTestPlan,
        passed: bool,
        stdout: &str,
        stderr: &str,
    ) -> AutoTestResult {
        AutoTestResult {
            command: plan.command.clone(),
            passed,
            output: format!("{stdout}\n{stderr}"),
            exit_code: if passed { Some(0) } else { Some(101) },
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    /// AC1: cargo compile error output classifies as `CompileError`.
    #[test]
    fn compile_error_classified_as_compile_error() {
        let plan = cargo_plan();
        let stderr = "error[E0308]: mismatched types\nerror: could not compile `crate` due to previous error";
        let result = make_result(&plan, false, "", stderr);
        assert_eq!(
            classify_auto_test(&plan, &result),
            FeedbackKind::CompileError
        );
    }

    /// AC2: pytest assertion failure classifies as `TestFailure`.
    #[test]
    fn pytest_assertion_classified_as_test_failure() {
        let plan = pytest_plan();
        let stdout = "============= test session starts =============\nFAILED tests/test_x.py::test_a - assert 1 == 2\n";
        let result = make_result(&plan, false, stdout, "");
        assert_eq!(
            classify_auto_test(&plan, &result),
            FeedbackKind::TestFailure
        );
    }

    #[test]
    fn build_pass_classified() {
        let plan = build_plan();
        let result = make_result(&plan, true, "", "");
        assert_eq!(classify_auto_test(&plan, &result), FeedbackKind::BuildPass);
    }

    #[test]
    fn test_pass_classified() {
        let plan = cargo_plan();
        let result = make_result(&plan, true, "test result: ok\n", "");
        assert_eq!(classify_auto_test(&plan, &result), FeedbackKind::TestPass);
    }

    /// `AutoTestRunner::detect` returning None plus
    /// `should_run_auto_test_for_success() == true` is the source signal
    /// for `NoVerifierAvailable`. The classification is performed in
    /// turn.rs but the input — `detect` returning None — is what we
    /// guarantee here.
    #[test]
    fn no_verifier_available_when_detect_returns_none() {
        let dir = tempdir().expect("tempdir");
        // Empty workspace, no Cargo.toml, no package.json, no python files.
        let plan = AutoTestRunner::detect(dir.path(), &[]);
        assert!(plan.is_none());
    }

    /// AC11 / R5: invalid UTF-8 in stdout/stderr does not panic
    /// `String::from_utf8_lossy` and downstream classify_auto_test still
    /// works. Driven directly through `AutoTestResult` because spawning
    /// a process that emits invalid UTF-8 reliably is platform-dependent.
    #[test]
    fn non_utf8_output_is_lossy_and_does_not_panic() {
        let plan = cargo_plan();
        let raw = b"hello\xFFworld";
        let lossy = String::from_utf8_lossy(raw).into_owned();
        let result = AutoTestResult {
            command: plan.command.clone(),
            passed: false,
            output: lossy.clone(),
            exit_code: Some(1),
            stdout: lossy,
            stderr: String::new(),
        };
        // No panic, valid kind output.
        let _kind = classify_auto_test(&plan, &result);
    }

    #[test]
    fn auto_test_kind_distinguishes_build_from_test() {
        assert_eq!(build_plan().auto_test_kind(), AutoTestKind::Build);
        assert_eq!(cargo_plan().auto_test_kind(), AutoTestKind::Test);
        assert_eq!(pytest_plan().auto_test_kind(), AutoTestKind::Test);
    }

    // --- Issue #459 / Task 1a.1: Tester-facing helper coverage ----------

    /// `has_cargo_manifest` is true only when `Cargo.toml` exists and there
    /// is no `tests/` directory (the "Tester candidate" shape).
    #[test]
    fn has_cargo_manifest_true_when_manifest_only() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        assert!(has_cargo_manifest(dir.path()));
    }

    #[test]
    fn has_cargo_manifest_false_when_tests_dir_present() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        assert!(!has_cargo_manifest(dir.path()));
    }

    #[test]
    fn has_cargo_manifest_false_when_no_manifest() {
        let dir = tempdir().expect("tempdir");
        assert!(!has_cargo_manifest(dir.path()));
    }

    /// CB-006: a Cargo.toml that defines an explicit `[[test]]` target is an
    /// explicit test verifier even without a `tests/` directory. Tester must
    /// not generate a smoke test in that case.
    #[test]
    fn has_cargo_manifest_false_when_explicit_test_target_section() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\n\n[[test]]\nname = \"smoke\"\npath = \"tests/smoke.rs\"\n",
        )
        .expect("write");
        assert!(!has_cargo_manifest(dir.path()));
    }

    /// CB-006: a Cargo.toml that lists `[[test]]` with leading whitespace and
    /// commented sections is still detected as an explicit verifier.
    #[test]
    fn has_cargo_manifest_false_when_test_section_indented() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\n# inline comment\n   [[test]]\nname = \"smoke\"\n",
        )
        .expect("write");
        assert!(!has_cargo_manifest(dir.path()));
    }

    /// CB-006: a `[[test]]` substring inside a string value (e.g. inside a
    /// metadata description) must NOT be treated as a real section header.
    #[test]
    fn has_cargo_manifest_true_when_test_substring_inside_string() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\ndescription = \"docs about [[test]] sections\"\n",
        )
        .expect("write");
        assert!(has_cargo_manifest(dir.path()));
    }

    /// `package_json_has_test_script` is true iff the JSON literally
    /// contains the `"test"` token (mirrors auto_test's existing
    /// substring heuristic).
    #[test]
    fn package_json_has_test_script_true_when_test_defined() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"test": "vitest"}}"#,
        )
        .expect("write");
        assert!(package_json_has_test_script(dir.path()));
    }

    #[test]
    fn package_json_has_test_script_false_when_test_missing() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"build": "tsc"}}"#,
        )
        .expect("write");
        assert!(!package_json_has_test_script(dir.path()));
    }

    #[test]
    fn package_json_has_test_script_false_when_file_missing() {
        let dir = tempdir().expect("tempdir");
        assert!(!package_json_has_test_script(dir.path()));
    }

    // --- Issue #457: count_compile_errors / count_test_failures -----------

    #[test]
    fn count_compile_errors_cargo_returns_count() {
        let plan = build_plan();
        let stderr = "error[E0308]: mismatched types\n\
                      error[E0382]: borrow of moved value\n\
                      error: could not compile `crate`";
        let result = make_result(&plan, false, "", stderr);
        assert_eq!(count_compile_errors(&result), Some(2));
    }

    #[test]
    fn count_compile_errors_npm_tsc_returns_count() {
        let plan = AutoTestPlan {
            command: "npm run build".to_string(),
            reason: "build".to_string(),
        };
        let stdout = "src/a.ts(10,5): error TS2304: Cannot find name 'foo'.\n\
                      src/b.ts(3,1): error TS1005: ',' expected.\n";
        let result = make_result(&plan, false, &stdout.to_ascii_lowercase(), "");
        assert_eq!(count_compile_errors(&result), Some(2));
    }

    #[test]
    fn count_compile_errors_no_match_returns_none() {
        let plan = pytest_plan();
        let result = make_result(&plan, false, "everything fine\n", "");
        assert_eq!(count_compile_errors(&result), None);
    }

    #[test]
    fn count_test_failures_cargo_returns_count() {
        let plan = cargo_plan();
        let stdout = "running 5 tests\n\
             test foo ... ok\n\
             test bar ... FAILED\n\
             test result: FAILED. 4 passed; 1 failed; 0 ignored\n";
        let result = make_result(&plan, false, stdout, "");
        assert_eq!(count_test_failures(&result), Some(1));
    }

    #[test]
    fn count_test_failures_pytest_returns_count() {
        let plan = pytest_plan();
        let stdout = "============= test session starts =============\n\
             FAILED tests/test_x.py::test_a - assert 1 == 2\n\
             FAILED tests/test_x.py::test_b - assert 3 == 4\n\
             ============= 2 failed, 5 passed in 1.23s ====\n";
        let result = make_result(&plan, false, stdout, "");
        assert_eq!(count_test_failures(&result), Some(2));
    }

    #[test]
    fn count_test_failures_no_summary_returns_none() {
        let plan = cargo_plan();
        let result = make_result(&plan, false, "compile error nothing else", "");
        assert_eq!(count_test_failures(&result), None);
    }

    #[test]
    fn auto_test_disabled_returns_true_when_env_set() {
        assert!(auto_test_disabled(|k| {
            if k == "ANVIL_NO_AUTO_TEST" {
                Ok("1".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }));
    }

    #[test]
    fn auto_test_disabled_returns_false_when_env_empty() {
        assert!(!auto_test_disabled(|_| Ok(String::new())));
    }

    #[test]
    fn auto_test_disabled_returns_false_when_env_absent() {
        assert!(!auto_test_disabled(|_| Err(std::env::VarError::NotPresent)));
    }
}
