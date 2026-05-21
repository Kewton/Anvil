use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::agent::prompting::load_project_instructions;
use crate::logging::log_llm_event;
use crate::session::feedback::FeedbackKind;

use super::task_workspace_scope::TaskWorkspaceScope;

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
pub(super) const MARKER_CARGO_TEST_FAILED: &str = "test result: failed";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierCandidateSource {
    ProjectInstruction,
    RecentSuccessfulBash,
    CargoManifest,
    PackageJsonScripts,
    NativeNodeFramework,
    PythonTests,
    PythonCompileFallback,
}

impl VerifierCandidateSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProjectInstruction => "project_instruction",
            Self::RecentSuccessfulBash => "recent_successful_bash",
            Self::CargoManifest => "cargo_manifest",
            Self::PackageJsonScripts => "package_json_scripts",
            Self::NativeNodeFramework => "native_node_framework",
            Self::PythonTests => "python_tests",
            Self::PythonCompileFallback => "python_compile_fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct VerifierCandidate {
    pub plan: AutoTestPlan,
    pub source: VerifierCandidateSource,
    pub confidence: f32,
    pub evidence: Vec<String>,
}

impl VerifierCandidate {
    fn into_plan(mut self) -> AutoTestPlan {
        self.plan.reason = format!(
            "{}; source={}; confidence={:.2}; evidence={}",
            self.plan.reason,
            self.source.as_str(),
            self.confidence,
            self.evidence.join(",")
        );
        self.plan
    }
}

/// Issue #651: structured verifier command. Fields are intentionally
/// private — sibling modules MUST construct values through the allowlisted
/// `from_*` constructors below so an LLM-proposed `&&` / pipe / shell
/// substitution can never reach `Command::new(...).args(...)`.
///
/// `to_display_string` is for **display / log** only and uses std-only
/// whitespace joining (no `shlex` dependency, design 5-2). It must still
/// pass through `crate::session::feedback::redact_verifier_command_for_storage`
/// before landing in any persisted payload (DR4-004).
///
/// `bound_test_artifacts` stores the (already scope-validated) test
/// artifact paths that were appended to `args`. Phase 2.3
/// (`validate_bound_test_artifacts_for_execution`) re-checks them at
/// execution time to defend against symlink-swap / TOCTOU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierCommand {
    runner: String,
    args: Vec<String>,
    bound_test_artifacts: Vec<String>,
}

/// Runners allowlisted as structured verifier programs. The list is
/// intentionally narrow and matches the structured `detect_*` entries
/// in this module. Any new entry must come with a `from_*` constructor
/// below and (where required) a Phase 2.3 execution-time path validator.
const VERIFIER_RUNNER_ALLOWLIST: &[&str] = &[
    "cargo", "python3", "python", "pytest", "uv", "poetry", "hatch", "npm", "pnpm", "yarn",
];

impl VerifierCommand {
    /// Internal allowlist-checked constructor. Returns `None` when the
    /// runner is not on `VERIFIER_RUNNER_ALLOWLIST`, when any arg
    /// contains a shell control character (per
    /// `completion_evidence::contains_evidence_poisoning_shell_control`,
    /// DR4-002), or when `runner` itself contains such a character.
    ///
    /// Callers must dedupe `bound_test_artifacts` if they care about
    /// ordering — the constructor preserves whatever ordering they pass.
    fn new_allowlisted(
        runner: &str,
        args: Vec<String>,
        bound_test_artifacts: Vec<String>,
    ) -> Option<Self> {
        if !VERIFIER_RUNNER_ALLOWLIST.contains(&runner) {
            return None;
        }
        if super::completion_evidence::contains_evidence_poisoning_shell_control(runner) {
            return None;
        }
        for arg in &args {
            if super::completion_evidence::contains_evidence_poisoning_shell_control(arg) {
                return None;
            }
        }
        Some(Self {
            runner: runner.to_string(),
            args,
            bound_test_artifacts,
        })
    }

    /// `cargo test --test <name> ...` structured constructor.
    ///
    /// Issue #651 (CB-001 / CB-002): cargo positional args are
    /// test-name filters, **not** file paths. Passing `tests/test_a.rs`
    /// directly produces 0 tests matched and a misleading exit 0. We
    /// therefore convert each owned test artifact to a
    /// `--test <name>` flag for top-level `tests/<name>.rs` integration
    /// tests. Any path we cannot safely convert (`src/...` unit tests,
    /// nested integration paths, paths without a `.rs` stem) makes the
    /// constructor return `None` so the caller falls back to `Weak`.
    ///
    /// CB-001 defense in depth: empty `owned_test_artifacts` is also a
    /// hard reject. An unbound cargo verifier could otherwise execute
    /// the entire test suite without satisfying the design contract
    /// that the *current task's* test artifact actually ran.
    ///
    /// `args_prefix` lets the detector inject flags like `--no-fail-fast`
    /// when needed; today the cargo path passes an empty prefix.
    #[allow(dead_code)]
    pub(super) fn from_cargo_test(
        args_prefix: Vec<String>,
        owned_test_artifacts: &[String],
    ) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        let mut test_flags: Vec<String> = Vec::with_capacity(owned_test_artifacts.len() * 2);
        for path in owned_test_artifacts {
            let name = cargo_integration_test_name(path)?;
            test_flags.push("--test".to_string());
            test_flags.push(name);
        }
        let mut args = Vec::with_capacity(args_prefix.len() + 1 + test_flags.len());
        args.push("test".to_string());
        args.extend(args_prefix);
        args.extend(test_flags);
        Self::new_allowlisted("cargo", args, owned_test_artifacts.to_vec())
    }

    /// `pytest [<owned_test_artifacts>]` structured constructor. Used by
    /// the bare-pytest / generic-Python detector path. Toolchain-specific
    /// runners (`uv run pytest` etc.) get their own constructors.
    ///
    /// Issue #651 CB-001 defense in depth: empty `owned_test_artifacts`
    /// is rejected so an unbound pytest verifier can never reach
    /// `Command::new`. pytest does accept file paths as positional
    /// args, so we forward them as-is; execution-time validation in
    /// `validate_bound_test_artifacts_for_execution` re-checks the
    /// canonical path then.
    #[allow(dead_code)]
    pub(super) fn from_pytest(
        args_prefix: Vec<String>,
        owned_test_artifacts: &[String],
    ) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        let mut args = args_prefix;
        args.extend(owned_test_artifacts.iter().cloned());
        Self::new_allowlisted("pytest", args, owned_test_artifacts.to_vec())
    }

    /// `python3 -B -m pytest -p no:cacheprovider [<owned_test_artifacts>]`
    /// structured constructor for the stdlib-Python toolchain detected by
    /// `python_pytest_command`. Toolchain-specific runners (`uv run` etc.)
    /// still go through Weak in Phase 2.2 because parsing their shell
    /// shape is out of scope.
    ///
    /// Issue #651 CB-001 defense in depth: empty `owned_test_artifacts`
    /// is rejected — pytest with no positional path scans the entire
    /// rootdir, which means the verifier may pass even when zero tests
    /// from the current task ran.
    #[allow(dead_code)]
    pub(super) fn from_python3_pytest_stdlib(owned_test_artifacts: &[String]) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        let mut args = vec![
            "-B".to_string(),
            "-m".to_string(),
            "pytest".to_string(),
            "-p".to_string(),
            "no:cacheprovider".to_string(),
        ];
        args.extend(owned_test_artifacts.iter().cloned());
        Self::new_allowlisted("python3", args, owned_test_artifacts.to_vec())
    }

    /// Returns the runner program name (allowlist member).
    #[allow(dead_code)]
    pub(super) fn runner(&self) -> &str {
        &self.runner
    }

    /// Returns the structured args slice (no shell metacharacters).
    #[allow(dead_code)]
    pub(super) fn args(&self) -> &[String] {
        &self.args
    }

    /// Returns the bound test artifact paths (scope-validated at planning
    /// time; Phase 2.3 re-validates at execution time).
    #[allow(dead_code)]
    pub(super) fn bound_test_artifacts(&self) -> &[String] {
        &self.bound_test_artifacts
    }

    /// Display-only join. Uses std whitespace join (no `shlex` crate).
    /// Output must be passed through
    /// `crate::session::feedback::redact_verifier_command_for_storage`
    /// before landing in any persisted / logged payload (DR4-004).
    #[allow(dead_code)]
    pub(super) fn to_display_string(&self) -> String {
        std::iter::once(self.runner.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Issue #651 CB-002: convert an owned test artifact path to the
/// integration-test name `cargo test --test <name>` expects.
///
/// Cargo positional args after `cargo test` are test-name **filters**,
/// not file paths. The only structurally safe binding we can produce
/// from a path is the integration-test stem under top-level `tests/`:
/// `tests/<name>.rs` → `--test <name>`.
///
/// Any path that is not exactly `tests/<stem>.rs` (e.g. `src/...`
/// internal unit tests, `tests/sub/dir.rs` nested integration files,
/// non-`.rs` extensions) returns `None`. The caller — currently
/// [`VerifierCommand::from_cargo_test`] — must then drop to the `Weak`
/// branch rather than fabricating a filter the LLM did not request.
fn cargo_integration_test_name(path: &str) -> Option<String> {
    let p = Path::new(path);
    let mut components = p.components();
    let first = components.next()?;
    let std::path::Component::Normal(first_name) = first else {
        return None;
    };
    if first_name.to_string_lossy() != "tests" {
        return None;
    }
    let second = components.next()?;
    if components.next().is_some() {
        // Nested under `tests/<subdir>/...` — not a top-level
        // integration test we can convert to `--test <name>`.
        return None;
    }
    let std::path::Component::Normal(file_name) = second else {
        return None;
    };
    let file_path = Path::new(file_name);
    if file_path.extension()?.to_string_lossy() != "rs" {
        return None;
    }
    let stem = file_path.file_stem()?.to_string_lossy().into_owned();
    if stem.is_empty() {
        return None;
    }
    Some(stem)
}

/// Issue #651 Task 2.2: structured outcome of "given owned test
/// artifacts, what verifier can we actually run?". Distinct from
/// `Option<AutoTestPlan>` because Phase 4.1 needs to distinguish "found
/// a runner but it cannot bind to a structured args list"
/// (`Weak` → `SafeStopReason::VerifierWeak`) from "no runner detected
/// at all" (`Missing` → `SafeStopReason::VerifierMissing`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OwnedTestVerifierPlan {
    /// A structured `VerifierCommand` was bound to `owned_test_artifacts`
    /// and is safe to feed to `Command::new(runner).args(args)`. `plan`
    /// is the display-side metadata (reason / shell-string preview).
    Runnable {
        plan: AutoTestPlan,
        command: VerifierCommand,
    },
    /// A runner was detected, but it cannot be expressed as a structured
    /// allowlisted `VerifierCommand` (e.g. `ProjectInstruction`,
    /// `RecentSuccessfulBash`, `uv run pytest`, shell-only compound).
    /// `display_command` carries the original shell preview for log
    /// payload context (already constrained to the
    /// `redact_verifier_command_for_storage` SSOT by callers).
    Weak {
        reason: &'static str,
        detected_source: &'static str,
        display_command: Option<String>,
    },
    /// No verifier candidate detected at all.
    Missing,
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
        Self::detect_candidate(work_root, changed_files).map(VerifierCandidate::into_plan)
    }

    pub(super) fn detect_with_recent_successes(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
    ) -> Option<AutoTestPlan> {
        Self::detect_candidate_with_recent_successes(
            work_root,
            changed_files,
            recent_successful_bash_commands,
        )
        .map(VerifierCandidate::into_plan)
    }

    pub(super) fn detect_candidate(
        work_root: &Path,
        changed_files: &[String],
    ) -> Option<VerifierCandidate> {
        Self::detect_candidate_with_recent_successes(work_root, changed_files, &[])
    }

    pub(super) fn detect_candidate_with_recent_successes(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
    ) -> Option<VerifierCandidate> {
        let candidates =
            detect_verifier_candidates(work_root, changed_files, recent_successful_bash_commands);
        let selected = select_verifier_candidate(candidates.clone());
        emit_verifier_candidate_telemetry(&candidates, selected.as_ref());
        selected
    }

    #[cfg(test)]
    pub(super) fn detect_candidates(
        work_root: &Path,
        changed_files: &[String],
    ) -> Vec<VerifierCandidate> {
        detect_verifier_candidates(work_root, changed_files, &[])
    }

    #[cfg(test)]
    pub(super) fn detect_candidates_with_recent_successes(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
    ) -> Vec<VerifierCandidate> {
        detect_verifier_candidates(work_root, changed_files, recent_successful_bash_commands)
    }

    /// Issue #651 Task 2.2: `OwnedTestVerifierPlan` entrypoint.
    ///
    /// Selects the highest-priority `VerifierCandidate` (same SSOT as
    /// `detect_candidate_with_recent_successes`) and then maps it to:
    ///
    /// - `Runnable` when the source has an allowlisted structured
    ///   constructor (currently `CargoManifest` → cargo test and
    ///   `PythonTests` stdlib → `python3 -B -m pytest`).
    /// - `Weak` when a candidate exists but cannot be expressed as a
    ///   structured `VerifierCommand` (free-form shell strings from
    ///   ProjectInstruction / RecentSuccessfulBash, npm/pnpm/yarn or
    ///   uv/poetry/hatch toolchains, pip-install compound, native node
    ///   framework builds, python_compile fallback).
    /// - `Missing` when no candidate was detected at all.
    ///
    /// The function never parses `plan.command` to construct a
    /// `VerifierCommand` — only the per-source allowlisted builder may
    /// call `VerifierCommand::new_allowlisted` (design 5-2 invariant).
    #[allow(dead_code)]
    pub(super) fn detect_with_owned_test_artifacts(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
        owned_test_artifacts: &[String],
    ) -> OwnedTestVerifierPlan {
        // CB-001 (high): Without any owned test artifact, there is
        // nothing for the verifier to bind to. Issue #651 design treats
        // this as `Missing` rather than `Weak` — the semantic problem is
        // "no test artifact for the current task", not "we lack a
        // structured runner". This matches the SafeStopReason mapping
        // (`Missing` → `VerifierMissing`).
        if owned_test_artifacts.is_empty() {
            return OwnedTestVerifierPlan::Missing;
        }
        let Some(candidate) = Self::detect_candidate_with_recent_successes(
            work_root,
            changed_files,
            recent_successful_bash_commands,
        ) else {
            return OwnedTestVerifierPlan::Missing;
        };
        let display_command = candidate.plan.command.clone();
        let source = candidate.source;
        let evidence = candidate.evidence.clone();
        let plan = candidate.into_plan();
        match source {
            VerifierCandidateSource::CargoManifest => {
                if let Some(command) =
                    VerifierCommand::from_cargo_test(Vec::new(), owned_test_artifacts)
                {
                    return OwnedTestVerifierPlan::Runnable { plan, command };
                }
                // CB-002: `from_cargo_test` returns `None` when an owned
                // test artifact cannot be safely mapped to
                // `cargo test --test <name>` (src/... internal paths,
                // nested integration paths, non-`tests/<stem>.rs`
                // shapes). The allowlist may also have rejected an arg.
                // Either way we fall back to `Weak` rather than fabricate
                // a positional filter that would match 0 tests.
                OwnedTestVerifierPlan::Weak {
                    reason: "cargo runner cannot bind owned test artifacts as --test flag",
                    detected_source: source.as_str(),
                    display_command: Some(display_command),
                }
            }
            VerifierCandidateSource::PythonTests => {
                // Only the stdlib toolchain has a structured constructor
                // today. uv/poetry/hatch/pip-install paths fall through
                // to Weak so an LLM-edited shell string can never become
                // a structured execution path.
                let is_stdlib = evidence.iter().any(|e| e == "python-toolchain:stdlib");
                if is_stdlib
                    && let Some(command) =
                        VerifierCommand::from_python3_pytest_stdlib(owned_test_artifacts)
                {
                    return OwnedTestVerifierPlan::Runnable { plan, command };
                }
                OwnedTestVerifierPlan::Weak {
                    reason: "python toolchain not structurally bindable",
                    detected_source: source.as_str(),
                    display_command: Some(display_command),
                }
            }
            VerifierCandidateSource::ProjectInstruction
            | VerifierCandidateSource::RecentSuccessfulBash
            | VerifierCandidateSource::PackageJsonScripts
            | VerifierCandidateSource::NativeNodeFramework
            | VerifierCandidateSource::PythonCompileFallback => OwnedTestVerifierPlan::Weak {
                reason: "verifier source has no allowlisted structured constructor",
                detected_source: source.as_str(),
                display_command: Some(display_command),
            },
        }
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
        // Issue #608 AP-08: apply the `test_output` formatter (pytest/cargo/
        // npm summary + tail trim) to the display-side `output` only. The
        // raw `stdout` / `stderr` / `combined` strings consumed by
        // `classify_auto_test` / feedback confirmation paths are NOT
        // mutated — they remain the verbatim child output (design §4.6 /
        // T2.B.2 raw-combined invariant).
        let formatted = crate::tools::test_output::format_for_tool_result(&combined);
        Ok(AutoTestResult {
            command: plan.command.clone(),
            passed: output.status.success(),
            output: truncate(&formatted, MAX_OUTPUT_BYTES),
            exit_code: output.status.code(),
            stdout,
            stderr,
        })
    }

    /// Issue #651 Task 2.3: structured verifier execution.
    ///
    /// Re-validates `command.bound_test_artifacts` at execution time
    /// (canonicalize + scope re-check, see
    /// `validate_bound_test_artifacts_for_execution`) before spawning
    /// `Command::new(runner).args(args)`. There is no shell — invalid
    /// LLM-proposed shell text cannot reach the child process here
    /// (DR4-002).
    ///
    /// A polling timeout (`AUTO_TEST_RUN_STRUCTURED_TIMEOUT`) kills the
    /// child if it hangs (mirrors `project_verifier.rs::run_with_timeout`).
    /// `display_command` is passed through
    /// `crate::session::feedback::redact_verifier_command_for_storage`
    /// so the persisted `AutoTestResult.command` field never contains
    /// a raw LLM-supplied secret-shaped substring (DR4-004).
    #[allow(dead_code)]
    pub(super) fn run_structured(
        work_root: &Path,
        scope: &TaskWorkspaceScope,
        command: &VerifierCommand,
        display_command: &str,
    ) -> Result<AutoTestResult, String> {
        validate_bound_test_artifacts_for_execution(work_root, scope, command)?;

        let mut child_cmd = Command::new(command.runner());
        child_cmd
            .args(command.args())
            .current_dir(work_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // PR-003: put the verifier in its own process group on Unix so
        // `wait_with_auto_test_timeout` can SIGKILL the whole descendant
        // tree on timeout. On non-Unix this is a no-op. SSOT lives in
        // `crate::tools::bash::apply_unix_pgroup` (Issue #461).
        crate::tools::bash::apply_unix_pgroup(&mut child_cmd);
        let output = wait_with_auto_test_timeout(
            &mut child_cmd,
            Duration::from_secs(AUTO_TEST_RUN_STRUCTURED_TIMEOUT_SECS),
        )?;
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
        let formatted = crate::tools::test_output::format_for_tool_result(&combined);
        let redacted =
            crate::session::feedback::redact_verifier_command_for_storage(display_command);
        Ok(AutoTestResult {
            command: redacted,
            passed: output.status.success(),
            output: truncate(&formatted, MAX_OUTPUT_BYTES),
            exit_code: output.status.code(),
            stdout,
            stderr,
        })
    }
}

/// Issue #651 Task 2.3: upper bound on a structured verifier process.
/// 300s mirrors the value used by the legacy `auto_test::run` path's
/// implicit wait (kept conservative to avoid breaking long test suites).
const AUTO_TEST_RUN_STRUCTURED_TIMEOUT_SECS: u64 = 300;

/// Issue #651 Task 2.3: execution-time validator for
/// `VerifierCommand.bound_test_artifacts`.
///
/// Stricter than planning-time `classify_ownership` because the child
/// process is about to read the file. Rejects:
/// - empty `bound_test_artifacts` (CB-001 defense in depth)
/// - empty / absolute / `..` / control-character paths
/// - paths containing an ignored top-level directory component
///   (`node_modules`, `.git`, `target`, ...) — CB-004 re-applies the
///   `task_workspace_scope::is_workspace_ignored_dir` SSOT that
///   `classify_ownership` already runs at planning time
/// - paths whose `std::fs::canonicalize` fails (missing file) — at
///   execution time a missing test artifact is a hard reject
/// - canonical targets that escape canonical `work_root` (symlink swap)
/// - paths the `TaskWorkspaceScope::contains` SSOT does not admit
#[allow(dead_code)]
pub(super) fn validate_bound_test_artifacts_for_execution(
    work_root: &Path,
    scope: &TaskWorkspaceScope,
    command: &VerifierCommand,
) -> Result<(), String> {
    // CB-001 defense in depth: an empty bound list means the verifier
    // command was not bound to *any* owned test artifact. The
    // constructors already reject this, but re-check here so an
    // execution-time `VerifierCommand` mutated by a future caller can
    // never run zero-bound and pass.
    if command.bound_test_artifacts().is_empty() {
        return Err("bound test artifact list is empty at execution time".to_string());
    }
    let work_root_canon = std::fs::canonicalize(work_root)
        .map_err(|err| format!("failed to canonicalize work_root: {err}"))?;
    for path in command.bound_test_artifacts() {
        if path.is_empty() {
            return Err("bound test artifact path is empty".to_string());
        }
        if path.chars().any(|c| c.is_control()) {
            return Err(format!(
                "bound test artifact path contains control character: {path:?}"
            ));
        }
        let p = Path::new(path);
        if p.is_absolute() {
            return Err(format!(
                "bound test artifact path must be relative, got absolute: {path:?}"
            ));
        }
        if p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "bound test artifact path contains parent traversal: {path:?}"
            ));
        }
        // CB-004: re-apply the planning-time ignored-top-dir filter at
        // execution time. `classify_ownership` already rejects these
        // paths at planning, but `VerifierCommand` constructors are
        // `pub(super)` and a future sibling caller may stage a path
        // that bypassed `owned_test_artifacts` (e.g. legacy callers,
        // tests). Defense in depth keeps `node_modules/...`,
        // `.git/...`, `target/...` etc. out of the verifier child
        // process regardless of how the command was assembled.
        if let Some(ignored) = p.components().find_map(|c| match c {
            std::path::Component::Normal(name) => {
                let name_str = name.to_string_lossy().into_owned();
                if super::task_workspace_scope::is_workspace_ignored_dir(&name_str) {
                    Some(name_str)
                } else {
                    None
                }
            }
            _ => None,
        }) {
            return Err(format!(
                "bound test artifact path traverses an ignored workspace directory \
                 ({ignored}): {path:?}"
            ));
        }
        if !scope.contains(path) {
            return Err(format!(
                "bound test artifact path is not in TaskWorkspaceScope: {path:?}"
            ));
        }
        let target = work_root.join(path);
        let target_canon = std::fs::canonicalize(&target).map_err(|err| {
            format!("bound test artifact missing at execution time: {path:?} ({err})")
        })?;
        if target_canon.strip_prefix(&work_root_canon).is_err() {
            return Err(format!(
                "bound test artifact canonicalization escapes work_root: {path:?}"
            ));
        }
    }
    Ok(())
}

/// Issue #651 Task 2.3 + CB-003: std-only polling wait with
/// kill-on-timeout that **drains stdout/stderr concurrently** so a
/// chatty test process never blocks on a full pipe buffer.
///
/// CB-003 fix: the previous implementation kept `stdout`/`stderr` as
/// `Stdio::piped()` and waited via `try_wait` without reading the
/// pipes. On macOS / Linux pipe buffers are ~64 KiB, so a chatty
/// pytest / cargo test could block on `write(stdout)` while the parent
/// loop spins in `try_wait` forever — eventually surfacing as a
/// spurious timeout error instead of the real test exit code.
///
/// We now spawn one `std::thread` per output stream that drains the
/// pipe into a `Vec<u8>` and reports the result over `mpsc::channel`.
/// The main thread keeps the existing polling structure (so the
/// kill-on-timeout contract is unchanged) and joins both drain threads
/// after `wait()` returns — whether due to natural exit, timeout, or
/// poll error.
///
/// ## Issue #651 PR-003: process-group kill on Unix
///
/// `cargo test` / `pytest` can spawn descendant processes (test
/// binaries, fixtures, server-style dev tools). The previous
/// `child.kill()` only signaled the immediate child, leaving its
/// descendants alive and holding ports / file handles into the next
/// turn.
///
/// On Unix, callers MUST apply `crate::tools::bash::apply_unix_pgroup`
/// to the `Command` before passing it in. That sets a `pre_exec` hook
/// that calls `setpgid(0, 0)` in the forked child between `fork(2)`
/// and `exec(2)`, putting the child in its own process group. On
/// timeout / poll error, this helper then sends `SIGKILL` to
/// `-pgid` (the whole group) so the test process tree is reaped.
/// On non-Unix, `apply_unix_pgroup` is a no-op and `child.kill()`
/// is the fallback contract (direct-child-only).
#[allow(dead_code)]
fn wait_with_auto_test_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::sync::mpsc;

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to run auto test command: {err}"))?;

    // Move the piped handles out of `child` before any wait — once
    // wait returns, the handles are no longer reachable for read.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let stdout_join = stdout_handle.map(|mut handle| {
        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>();
        let join = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let result = handle.read_to_end(&mut buf).map(|_| buf);
            let _ = tx.send(result);
        });
        (join, rx)
    });
    let stderr_join = stderr_handle.map(|mut handle| {
        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>();
        let join = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let result = handle.read_to_end(&mut buf).map(|_| buf);
            let _ = tx.send(result);
        });
        (join, rx)
    });

    let start = Instant::now();
    let wait_outcome: Result<std::process::ExitStatus, String> = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() >= timeout => {
                // PR-003: kill the whole process group on Unix so
                // descendant test processes don't leak ports / files.
                kill_auto_test_child_tree(&mut child);
                let _ = child.wait();
                break Err(format!(
                    "auto test command timed out after {}s",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => {
                kill_auto_test_child_tree(&mut child);
                let _ = child.wait();
                break Err(format!("failed while waiting for auto test command: {err}"));
            }
        }
    };

    // Always join the drain threads. Both the success and timeout
    // paths need to consume the channel result so the OS pipe can
    // close cleanly and the thread handle is not detached.
    let stdout_bytes = drain_collect(stdout_join);
    let stderr_bytes = drain_collect(stderr_join);

    let status = wait_outcome?;
    Ok(std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

/// CB-003 helper type alias: per-stream drain handle = (join, rx).
#[allow(dead_code)]
type DrainHandle = (
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>,
);

/// Issue #651 PR-003: kill the structured auto-test child process and,
/// on Unix, its entire process group so descendants (test binaries,
/// dev servers, fixtures) cannot survive a timeout and hold ports /
/// file handles into later turns.
///
/// The Unix path uses `libc::kill(-pgid, SIGKILL)` where `pgid` is the
/// child's pid because `apply_unix_pgroup` (applied by the caller
/// before spawn) ran `setpgid(0, 0)` in the forked child, making the
/// child its own process-group leader. SIGKILL goes straight to all
/// processes in that group — including descendants the child spawned.
/// SIGTERM grace is not given: this is the timeout / fatal-error
/// branch, so the caller has already decided to terminate.
///
/// On non-Unix targets `apply_unix_pgroup` is a no-op and we fall back
/// to `child.kill()` (direct-child only). This is the documented
/// limitation for Windows / WASI.
///
/// ## `unsafe` boundary
///
/// The single `unsafe` call is `libc::kill(...)`. It does not run in
/// the forked child (unlike `pre_exec`), so the Rust async-signal
/// safety rules do not apply. We deliberately avoid pulling in the
/// `nix` crate — `Cargo.toml` is unchanged (CLAUDE.md "no new
/// dependency" expectation).
#[allow(dead_code)]
fn kill_auto_test_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Negative pid → process group. SAFETY: the libc call is FFI;
        // no Rust state is mutated and the call is async-signal-safe.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
        // Belt-and-suspenders: also kill the direct child in case
        // `apply_unix_pgroup` was not applied (e.g. legacy callers).
        // `child.kill()` is idempotent w.r.t. an already-killed child.
        let _ = child.kill();
    }

    #[cfg(not(unix))]
    {
        // Non-Unix: direct child only (documented limitation).
        let _ = child.kill();
    }
}

/// CB-003 helper: join a drain thread's channel and unwrap to bytes.
/// Any IO / panic failure degrades to an empty buffer — we never let a
/// drain glitch mask the child exit status that the caller cares about.
#[allow(dead_code)]
fn drain_collect(handle: Option<DrainHandle>) -> Vec<u8> {
    let Some((join, rx)) = handle else {
        return Vec::new();
    };
    let bytes = rx.recv().ok().and_then(Result::ok).unwrap_or_default();
    let _ = join.join();
    bytes
}

fn detect_verifier_candidates(
    work_root: &Path,
    changed_files: &[String],
    recent_successful_bash_commands: &[String],
) -> Vec<VerifierCandidate> {
    let mut candidates = Vec::new();
    if let Some(candidate) = detect_project_instruction_test(work_root, changed_files) {
        candidates.push(candidate);
    }
    if let Some(candidate) =
        detect_recent_successful_bash(changed_files, recent_successful_bash_commands)
    {
        candidates.push(candidate);
    }
    if let Some(candidate) = detect_cargo_test(work_root) {
        candidates.push(candidate);
    }
    if let Some(candidate) = detect_node_scripts(work_root) {
        candidates.push(candidate);
    }
    if let Some(candidate) = detect_native_node_framework(work_root, changed_files) {
        candidates.push(candidate);
    }
    if let Some(candidate) = detect_python_verifier(work_root, changed_files) {
        candidates.push(candidate);
    }
    candidates
}

fn select_verifier_candidate(candidates: Vec<VerifierCandidate>) -> Option<VerifierCandidate> {
    candidates.into_iter().max_by(|a, b| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| source_priority(a.source).cmp(&source_priority(b.source)))
    })
}

fn emit_verifier_candidate_telemetry(
    candidates: &[VerifierCandidate],
    selected: Option<&VerifierCandidate>,
) {
    let mut source_counts = std::collections::BTreeMap::new();
    for candidate in candidates {
        *source_counts
            .entry(candidate.source.as_str())
            .or_insert(0usize) += 1;
    }
    log_llm_event(
        "agent.autotest.candidates",
        serde_json::json!({
            "candidate_count": candidates.len(),
            "selected_source": selected.map(|candidate| candidate.source.as_str()),
            "source_counts": source_counts,
        }),
    );
}

fn source_priority(source: VerifierCandidateSource) -> u8 {
    match source {
        VerifierCandidateSource::ProjectInstruction => 6,
        VerifierCandidateSource::RecentSuccessfulBash => 5,
        VerifierCandidateSource::CargoManifest => 4,
        VerifierCandidateSource::PackageJsonScripts => 3,
        VerifierCandidateSource::NativeNodeFramework => 2,
        VerifierCandidateSource::PythonTests => 2,
        VerifierCandidateSource::PythonCompileFallback => 1,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PackageJsonEvidence {
    scripts: BTreeMap<String, String>,
    packages: BTreeSet<String>,
}

impl PackageJsonEvidence {
    fn from_file(work_root: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(work_root.join("package.json")).ok()?;
        Self::from_str(&raw)
    }

    fn from_str(raw: &str) -> Option<Self> {
        let json = serde_json::from_str::<serde_json::Value>(raw).ok()?;
        let scripts = json
            .get("scripts")
            .and_then(serde_json::Value::as_object)
            .map(|scripts| {
                scripts
                    .iter()
                    .filter_map(|(name, value)| {
                        value
                            .as_str()
                            .map(str::trim)
                            .filter(|script| !script.is_empty())
                            .map(|script| (name.to_ascii_lowercase(), script.to_string()))
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let mut packages = BTreeSet::new();
        for section in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(deps) = json.get(section).and_then(serde_json::Value::as_object) {
                packages.extend(deps.keys().map(|name| name.to_ascii_lowercase()));
            }
        }
        Some(Self { scripts, packages })
    }

    fn has_script(&self, name: &str) -> bool {
        self.scripts
            .get(&name.to_ascii_lowercase())
            .is_some_and(|script| !script.trim().is_empty())
    }

    fn has_package(&self, name: &str) -> bool {
        self.packages.contains(&name.to_ascii_lowercase())
    }

    fn has_any_package(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.has_package(name))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PythonProjectEvidence {
    sections: BTreeSet<String>,
    dependencies: BTreeSet<String>,
    hatch_test_script: bool,
}

impl PythonProjectEvidence {
    fn from_file(work_root: &Path) -> Self {
        let raw = std::fs::read_to_string(work_root.join("pyproject.toml")).unwrap_or_default();
        Self::from_str(&raw)
    }

    fn from_str(raw: &str) -> Self {
        let mut evidence = Self::default();
        let mut active_section = String::new();
        let mut in_project_dependency_array = false;
        let mut in_project_optional_dependency_array = false;
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(section) = parse_toml_section(trimmed) {
                active_section = section;
                evidence.sections.insert(active_section.clone());
                continue;
            }
            let line_without_comment = trimmed.split('#').next().unwrap_or(trimmed).trim();
            if in_project_dependency_array || in_project_optional_dependency_array {
                evidence
                    .dependencies
                    .extend(extract_dependency_names(line_without_comment));
                if line_without_comment.contains(']') {
                    in_project_dependency_array = false;
                    in_project_optional_dependency_array = false;
                }
                continue;
            }
            if active_section.starts_with("tool.hatch")
                && (line_without_comment.starts_with("test =")
                    || line_without_comment.starts_with("test="))
            {
                evidence.hatch_test_script = true;
            }
            if active_section == "project"
                && (line_without_comment.starts_with("dependencies =")
                    || line_without_comment.starts_with("dependencies="))
            {
                evidence
                    .dependencies
                    .extend(extract_dependency_names(line_without_comment));
                in_project_dependency_array =
                    line_without_comment.contains('[') && !line_without_comment.contains(']');
                continue;
            }
            if active_section.starts_with("project.optional-dependencies") {
                evidence
                    .dependencies
                    .extend(extract_quoted_dependency_names(line_without_comment));
                in_project_optional_dependency_array =
                    line_without_comment.contains('[') && !line_without_comment.contains(']');
                continue;
            }
            if line_without_comment.starts_with("optional-dependencies")
                || active_section.starts_with("tool.poetry.dependencies")
                || active_section.starts_with("tool.poetry.group.")
            {
                evidence
                    .dependencies
                    .extend(extract_dependency_names(line_without_comment));
            }
        }
        evidence
    }

    fn has_section_prefix(&self, prefix: &str) -> bool {
        self.sections
            .iter()
            .any(|section| section.starts_with(prefix))
    }

    fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains(&name.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CargoManifestEvidence {
    has_explicit_test_target: bool,
}

impl CargoManifestEvidence {
    fn from_str(raw: &str) -> Self {
        Self {
            has_explicit_test_target: raw
                .lines()
                .map(str::trim_start)
                .filter(|line| !line.starts_with('#'))
                .any(|line| {
                    parse_toml_array_section(line).is_some_and(|section| section == "test")
                }),
        }
    }
}

fn parse_toml_section(trimmed: &str) -> Option<String> {
    if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.starts_with("[[") {
        return Some(trimmed.trim_matches(['[', ']']).trim().to_ascii_lowercase());
    }
    None
}

fn parse_toml_array_section(trimmed: &str) -> Option<String> {
    if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
        return Some(
            trimmed
                .trim_start_matches("[[")
                .trim_end_matches("]]")
                .trim()
                .to_ascii_lowercase(),
        );
    }
    None
}

fn extract_dependency_names(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some((name, _)) = line.split_once('=')
        && !name.trim().eq_ignore_ascii_case("dependencies")
        && !name.trim().eq_ignore_ascii_case("optional-dependencies")
    {
        names.push(normalize_dependency_name(name.trim()));
    }
    names.extend(extract_quoted_dependency_names(line));
    names
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn extract_quoted_dependency_names(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = line;
    while let Some((_, after_quote)) = rest.split_once(['"', '\'']) {
        let Some((candidate, after_close)) = after_quote.split_once(['"', '\'']) else {
            break;
        };
        if let Some(name) = dependency_name_from_requirement(candidate) {
            names.push(name);
        }
        rest = after_close;
    }
    names
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn dependency_name_from_requirement(requirement: &str) -> Option<String> {
    let trimmed = requirement.trim();
    if trimmed.is_empty() {
        return None;
    }
    let name = trimmed
        .split(['<', '>', '=', '!', '~', '[', ';', ' '])
        .next()
        .unwrap_or("")
        .trim();
    (!name.is_empty()).then(|| normalize_dependency_name(name))
}

fn normalize_dependency_name(name: &str) -> String {
    name.trim()
        .trim_matches(['"', '\'', '`'])
        .to_ascii_lowercase()
        .replace('_', "-")
}

fn detect_recent_successful_bash(
    changed_files: &[String],
    recent_successful_bash_commands: &[String],
) -> Option<VerifierCandidate> {
    let command = recent_successful_bash_commands
        .iter()
        .rev()
        .find(|command| recent_successful_command_is_reusable_verifier(command, changed_files))?
        .trim()
        .to_string();
    Some(VerifierCandidate {
        plan: AutoTestPlan {
            command,
            reason: "recent successful Bash verifier detected".to_string(),
        },
        source: VerifierCandidateSource::RecentSuccessfulBash,
        confidence: 0.9,
        evidence: vec![
            "bash-exit-code-0".to_string(),
            "safe-verifier-command".to_string(),
        ],
    })
}

fn recent_successful_command_is_reusable_verifier(command: &str, changed_files: &[String]) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    if lower.is_empty() || lower.len() > 300 {
        return false;
    }
    if contains_blocked_shell_fragment(&lower) {
        return false;
    }
    if is_project_level_verifier_command(&lower) {
        return true;
    }
    command_references_changed_file(command, changed_files)
        && is_local_script_verifier_command(&lower)
}

fn contains_blocked_shell_fragment(lower: &str) -> bool {
    [
        "rm -rf",
        "sudo ",
        "curl ",
        "wget ",
        "git push",
        "git reset",
        "chmod ",
        "chown ",
        "mkfs",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_project_level_verifier_command(lower: &str) -> bool {
    [
        "cargo test",
        "cargo build",
        "cargo check",
        "cargo clippy",
        "npm test",
        "npm run test",
        "npm run build",
        "npm exec -- next build",
        "npm exec -- vite build",
        "npm exec -- astro build",
        "pnpm test",
        "pnpm run test",
        "pnpm build",
        "pnpm run build",
        "yarn test",
        "yarn build",
        "python -m pytest",
        "python3 -m pytest",
        "python3 -b -m pytest",
        "pytest",
        "uv run pytest",
        "uv run python -m pytest",
        "poetry run pytest",
        "hatch run test",
        "hatch run pytest",
        "ruff check",
        "mypy",
        "pyright",
        "tsc",
        "go test",
    ]
    .iter()
    .any(|needle| lower.starts_with(needle) || lower.contains(&format!("&& {needle}")))
}

fn is_local_script_verifier_command(lower: &str) -> bool {
    lower.starts_with("python ")
        || lower.starts_with("python3 ")
        || lower.starts_with("node ")
        || lower.starts_with("deno ")
        || lower.starts_with("bash ")
        || lower.starts_with("sh ")
}

fn command_references_changed_file(command: &str, changed_files: &[String]) -> bool {
    changed_files.iter().any(|path| {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        if command.contains(path) {
            return true;
        }
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.is_empty() && command.contains(name))
    })
}

fn detect_cargo_test(work_root: &Path) -> Option<VerifierCandidate> {
    if work_root.join("Cargo.toml").is_file() {
        return Some(VerifierCandidate {
            plan: AutoTestPlan {
                command: "cargo test".to_string(),
                reason: "Cargo.toml detected".to_string(),
            },
            source: VerifierCandidateSource::CargoManifest,
            confidence: 0.86,
            evidence: vec!["Cargo.toml".to_string()],
        });
    }
    None
}

fn detect_node_scripts(work_root: &Path) -> Option<VerifierCandidate> {
    let package = PackageJsonEvidence::from_file(work_root)?;
    let has_test = package.has_script("test");
    let has_build = package.has_script("build");
    let (command, reason, confidence, evidence) = if has_test && has_build {
        (
            node_verifier_command(work_root, "npm test && npm run build"),
            "package.json test and build scripts detected",
            0.84,
            vec!["scripts.test".to_string(), "scripts.build".to_string()],
        )
    } else if has_test {
        (
            node_verifier_command(work_root, "npm test"),
            "package.json test script detected",
            0.8,
            vec!["scripts.test".to_string()],
        )
    } else if has_build {
        (
            node_verifier_command(work_root, "npm run build"),
            "package.json build script detected",
            0.62,
            vec!["scripts.build".to_string()],
        )
    } else {
        return None;
    };
    Some(VerifierCandidate {
        plan: AutoTestPlan {
            command,
            reason: reason.to_string(),
        },
        source: VerifierCandidateSource::PackageJsonScripts,
        confidence,
        evidence,
    })
}

fn detect_native_node_framework(
    work_root: &Path,
    changed_files: &[String],
) -> Option<VerifierCandidate> {
    let package = PackageJsonEvidence::from_file(work_root)?;
    if package.has_script("build") || package.has_script("test") {
        return None;
    }
    let framework = native_node_framework_command(&package, work_root, changed_files)?;
    let command = node_verifier_command(work_root, framework.command);
    Some(VerifierCandidate {
        plan: AutoTestPlan {
            command,
            reason: format!(
                "{} project detected without package scripts",
                framework.label
            ),
        },
        source: VerifierCandidateSource::NativeNodeFramework,
        confidence: 0.56,
        evidence: framework.evidence,
    })
}

struct NativeNodeFrameworkCommand {
    label: &'static str,
    command: &'static str,
    evidence: Vec<String>,
}

fn native_node_framework_command(
    package: &PackageJsonEvidence,
    work_root: &Path,
    changed_files: &[String],
) -> Option<NativeNodeFrameworkCommand> {
    let changed_ui = changed_files.iter().any(|path| {
        matches!(
            Path::new(path).extension().and_then(|ext| ext.to_str()),
            Some("svelte" | "vue" | "tsx" | "jsx" | "astro" | "css")
        )
    });
    let mut evidence = Vec::new();
    if changed_ui {
        evidence.push("changed-ui-file".to_string());
    }
    if package.has_package("next") {
        evidence.push("package.next".to_string());
        return Some(NativeNodeFrameworkCommand {
            label: "Next.js",
            command: "npm exec -- next build",
            evidence,
        });
    }
    if package.has_package("astro") || changed_files.iter().any(|path| path.ends_with(".astro")) {
        evidence.push("package.astro-or-astro-file".to_string());
        return Some(NativeNodeFrameworkCommand {
            label: "Astro",
            command: "npm exec -- astro build",
            evidence,
        });
    }
    let has_vite_family = package.has_any_package(&[
        "vite",
        "@sveltejs/kit",
        "svelte",
        "@vitejs/plugin-vue",
        "vue",
        "solid-js",
    ]) || work_root.join("vite.config.js").is_file()
        || work_root.join("vite.config.ts").is_file()
        || work_root.join("svelte.config.js").is_file()
        || work_root.join("svelte.config.ts").is_file();
    if has_vite_family && changed_ui {
        evidence.push("vite-family-framework".to_string());
        return Some(NativeNodeFrameworkCommand {
            label: "Vite-family UI",
            command: "npm exec -- vite build",
            evidence,
        });
    }
    None
}

fn detect_python_verifier(work_root: &Path, changed_files: &[String]) -> Option<VerifierCandidate> {
    if !has_python_surface(work_root, changed_files) {
        return None;
    }
    let has_test_path = work_root.join("tests").is_dir()
        || changed_files.iter().any(|path| {
            path.starts_with("tests/") || path.ends_with("_test.py") || path.starts_with("test_")
        });
    let has_pytest_config = work_root.join("pytest.ini").is_file();
    let has_pytest_dependency = python_project_mentions_pytest(work_root);
    if has_test_path || has_pytest_config || has_pytest_dependency {
        let mut evidence = Vec::new();
        if has_test_path {
            evidence.push("python-test-path".to_string());
        }
        if has_pytest_config {
            evidence.push("pytest.ini".to_string());
        }
        if has_pytest_dependency {
            evidence.push("pytest-dependency".to_string());
        }
        let pytest = python_pytest_command(work_root);
        evidence.extend(pytest.evidence);
        return Some(VerifierCandidate {
            plan: AutoTestPlan {
                command: pytest.command,
                reason: pytest.reason,
            },
            source: VerifierCandidateSource::PythonTests,
            confidence: pytest.confidence,
            evidence,
        });
    }
    if let Some(script) = first_python_script(changed_files) {
        return Some(VerifierCandidate {
            plan: AutoTestPlan {
                command: format!("python3 -m py_compile {}", shell_quote(&script)),
                reason: "Python implementation detected without pytest".to_string(),
            },
            source: VerifierCandidateSource::PythonCompileFallback,
            confidence: 0.48,
            evidence: vec![
                "changed-python-file".to_string(),
                "no-pytest-signal".to_string(),
            ],
        });
    }
    None
}

struct PythonPytestCommand {
    command: String,
    reason: String,
    confidence: f32,
    evidence: Vec<String>,
}

fn python_pytest_command(work_root: &Path) -> PythonPytestCommand {
    let pyproject = PythonProjectEvidence::from_file(work_root);
    if work_root.join("uv.lock").is_file() || pyproject.has_section_prefix("tool.uv") {
        return PythonPytestCommand {
            command: "uv run pytest -p no:cacheprovider".to_string(),
            reason: "Python pytest suite detected with uv project evidence".to_string(),
            confidence: 0.84,
            evidence: vec!["python-toolchain:uv".to_string()],
        };
    }
    if work_root.join("poetry.lock").is_file() || pyproject.has_section_prefix("tool.poetry") {
        return PythonPytestCommand {
            command: "poetry run pytest -p no:cacheprovider".to_string(),
            reason: "Python pytest suite detected with Poetry project evidence".to_string(),
            confidence: 0.83,
            evidence: vec!["python-toolchain:poetry".to_string()],
        };
    }
    if pyproject.has_section_prefix("tool.hatch") {
        let has_test_script = pyproject.hatch_test_script;
        return PythonPytestCommand {
            command: if has_test_script {
                "hatch run test".to_string()
            } else {
                "hatch run pytest -p no:cacheprovider".to_string()
            },
            reason: "Python pytest suite detected with Hatch project evidence".to_string(),
            confidence: 0.82,
            evidence: vec![if has_test_script {
                "python-toolchain:hatch-test-script".to_string()
            } else {
                "python-toolchain:hatch".to_string()
            }],
        };
    }
    if work_root.join("pyproject.toml").is_file() {
        let packages = python_pyproject_test_packages(&pyproject);
        return PythonPytestCommand {
            command: format!(
                "python3 -m pip install {} && PYTHONPATH=src:. python3 -B -m pytest -p no:cacheprovider",
                packages.join(" ")
            ),
            reason: "Python tests detected with pyproject.toml; installing declared test dependencies before pytest".to_string(),
            confidence: 0.8,
            evidence: vec!["python-toolchain:pip-direct-deps".to_string()],
        };
    }
    PythonPytestCommand {
        command: "python3 -B -m pytest -p no:cacheprovider".to_string(),
        reason: "Python tests detected".to_string(),
        confidence: 0.78,
        evidence: vec!["python-toolchain:stdlib".to_string()],
    }
}

fn python_pyproject_test_packages(pyproject: &PythonProjectEvidence) -> Vec<String> {
    let mut packages: BTreeSet<String> = pyproject
        .dependencies
        .iter()
        .filter(|name| is_safe_python_package_name(name))
        .cloned()
        .collect();
    packages.insert("pytest".to_string());
    packages.into_iter().collect()
}

fn is_safe_python_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}

fn python_project_mentions_pytest(work_root: &Path) -> bool {
    if PythonProjectEvidence::from_file(work_root).has_dependency("pytest") {
        return true;
    }
    ["requirements.txt", "requirements-dev.txt"]
        .iter()
        .filter_map(|name| std::fs::read_to_string(work_root.join(name)).ok())
        .any(|contents| requirements_mentions_package(&contents, "pytest"))
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

pub(super) fn combined_output_for_classify(result: &AutoTestResult) -> String {
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
pub(super) fn parse_pytest_failed(lower: &str) -> Option<usize> {
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
) -> Option<VerifierCandidate> {
    let instructions = load_project_instructions(work_root, work_root)?;
    let command =
        extract_safe_preferred_command(&instructions.global_content, work_root, changed_files)?;
    Some(VerifierCandidate {
        plan: AutoTestPlan {
            command,
            reason: "ANVIL.md preferred command".to_string(),
        },
        source: VerifierCandidateSource::ProjectInstruction,
        confidence: 0.95,
        evidence: vec!["ANVIL.md".to_string(), "safe-preferred-command".to_string()],
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
    CargoManifestEvidence::from_str(raw).has_explicit_test_target
}

/// Returns true when `work_root/package.json` defines a real `scripts.test`
/// string. Tester takes the *false* branch (no test script defined) as a
/// candidate for smoke-test generation, so top-level `"test"` metadata must
/// not be treated as a runnable verifier.
pub(super) fn package_json_has_test_script(work_root: &Path) -> bool {
    let path = work_root.join("package.json");
    let Ok(package) = std::fs::read_to_string(&path) else {
        return false;
    };
    package_json_has_script(&package, "test")
}

fn package_json_has_script(package: &str, name: &str) -> bool {
    PackageJsonEvidence::from_str(package).is_some_and(|package| package.has_script(name))
}

fn requirements_mentions_package(contents: &str, package_name: &str) -> bool {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(dependency_name_from_requirement)
        .any(|name| name == package_name)
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
        assert_eq!(plan.command, "python3 -B -m pytest -p no:cacheprovider");
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
        assert!(
            plan.reason
                .starts_with("package.json test and build scripts detected")
        );
        assert!(plan.reason.contains("source=package_json_scripts"));
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
    fn detects_sveltekit_build_without_package_scripts() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"@sveltejs/kit":"1.0.0","svelte":"4.0.0","vite":"5.0.0"}}"#,
        )
        .expect("package");
        std::fs::write(dir.path().join("svelte.config.js"), "export default {};\n")
            .expect("svelte config");

        let plan = AutoTestRunner::detect(dir.path(), &["src/routes/+page.svelte".to_string()])
            .expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm exec -- vite build"
        );
        assert!(plan.reason.contains("source=native_node_framework"));
        assert!(plan.reason.contains("vite-family-framework"));
    }

    #[test]
    fn detects_next_build_without_package_scripts() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"}}"#,
        )
        .expect("package");

        let plan =
            AutoTestRunner::detect(dir.path(), &["src/app/page.tsx".to_string()]).expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm exec -- next build"
        );
        assert!(plan.reason.contains("Next.js project detected"));
        assert!(plan.reason.contains("package.next"));
    }

    #[test]
    fn detects_astro_build_without_package_scripts() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"astro":"4.0.0"}}"#,
        )
        .expect("package");

        let plan = AutoTestRunner::detect(dir.path(), &["src/pages/index.astro".to_string()])
            .expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm exec -- astro build"
        );
        assert!(plan.reason.contains("Astro project detected"));
        assert!(plan.reason.contains("package.astro-or-astro-file"));
    }

    #[test]
    fn detects_solid_vite_build_without_package_scripts() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"solid-js":"1.8.0","vite":"5.0.0"}}"#,
        )
        .expect("package");

        let plan = AutoTestRunner::detect(dir.path(), &["src/App.tsx".to_string()]).expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm exec -- vite build"
        );
        assert!(plan.reason.contains("Vite-family UI project detected"));
        assert!(plan.reason.contains("vite-family-framework"));
    }

    #[test]
    fn package_build_script_beats_native_framework_fallback() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"svelte-kit build"},"dependencies":{"@sveltejs/kit":"1.0.0","svelte":"4.0.0","vite":"5.0.0"}}"#,
        )
        .expect("package");

        let plan = AutoTestRunner::detect(dir.path(), &["src/routes/+page.svelte".to_string()])
            .expect("plan");

        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm run build"
        );
        assert!(plan.reason.contains("source=package_json_scripts"));
    }

    #[test]
    fn detector_exposes_ranked_verifier_candidates() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `python3 tool.py sample.csv`\n",
        )
        .expect("anvil");
        std::fs::write(dir.path().join("tool.py"), "print('ok')\n").expect("py");
        std::fs::write(dir.path().join("sample.csv"), "x\n").expect("csv");
        std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname='x'\n")
            .expect("pyproject");

        let candidates = AutoTestRunner::detect_candidates(dir.path(), &["tool.py".to_string()]);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.source == VerifierCandidateSource::ProjectInstruction)
        );
        assert!(candidates
            .iter()
            .any(|candidate| candidate.source == VerifierCandidateSource::PythonCompileFallback));

        let selected = AutoTestRunner::detect_candidate(dir.path(), &["tool.py".to_string()])
            .expect("selected");
        assert_eq!(selected.source, VerifierCandidateSource::ProjectInstruction);
        assert_eq!(selected.plan.command, "python3 tool.py sample.csv");
    }

    #[test]
    fn recent_successful_bash_beats_weak_python_fallback() {
        let dir = tempdir().expect("tempdir");
        let changed_files = vec!["scripts/report.py".to_string()];
        let recent = vec!["python3 -m pytest".to_string()];

        let candidates = AutoTestRunner::detect_candidates_with_recent_successes(
            dir.path(),
            &changed_files,
            &recent,
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.source == VerifierCandidateSource::RecentSuccessfulBash
        }));

        let selected = AutoTestRunner::detect_candidate_with_recent_successes(
            dir.path(),
            &changed_files,
            &recent,
        )
        .expect("selected");
        assert_eq!(
            selected.source,
            VerifierCandidateSource::RecentSuccessfulBash
        );
        assert_eq!(selected.plan.command, "python3 -m pytest");
    }

    #[test]
    fn recent_successful_bash_ignores_non_verifier_command() {
        let dir = tempdir().expect("tempdir");
        let recent = vec!["echo done".to_string()];

        let selected = AutoTestRunner::detect_candidate_with_recent_successes(
            dir.path(),
            &["scripts/report.py".to_string()],
            &recent,
        )
        .expect("python fallback");

        assert_eq!(
            selected.source,
            VerifierCandidateSource::PythonCompileFallback
        );
        assert_ne!(selected.plan.command, "echo done");
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
            command: "python3 -B -m pytest -p no:cacheprovider".to_string(),
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

    /// `package_json_has_test_script` is true iff `scripts.test` is a real
    /// non-empty string.
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
    fn package_json_has_test_script_false_when_test_is_not_script() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","test":"not a script","scripts":{"build":"tsc"}}"#,
        )
        .expect("write");
        assert!(!package_json_has_test_script(dir.path()));
        let plan = AutoTestRunner::detect(dir.path(), &[]).expect("build plan");
        assert_eq!(
            plan.command,
            "export CI=1 NUXT_IGNORE_LOCK=1; npm install && npm run build"
        );
    }

    #[test]
    fn pyproject_without_pytest_uses_py_compile_fallback() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname='x'\n")
            .expect("pyproject");
        let plan = AutoTestRunner::detect(dir.path(), &["src/app.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m py_compile 'src/app.py'");
        assert!(plan.reason.contains("source=python_compile_fallback"));
    }

    #[test]
    fn pyproject_with_pytest_dependency_uses_pytest() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\ndependencies = ['pytest']\n",
        )
        .expect("pyproject");
        let plan = AutoTestRunner::detect(dir.path(), &["src/app.py".to_string()]).expect("plan");
        assert_eq!(
            plan.command,
            "python3 -m pip install pytest && PYTHONPATH=src:. python3 -B -m pytest -p no:cacheprovider"
        );
        assert!(plan.reason.contains("pytest-dependency"));
        assert!(plan.reason.contains("python-toolchain:pip-direct-deps"));
    }

    #[test]
    fn pyproject_multiline_dependencies_feed_direct_pip_pytest_command() {
        let pyproject = PythonProjectEvidence::from_str(
            r#"[project]
dependencies = [
    "fastapi>=0.104",
    "uvicorn[standard]>=0.24",
]

[project.optional-dependencies]
dev = [
    "httpx>=0.25",
    "pytest>=7",
]
"#,
        );

        assert!(pyproject.has_dependency("fastapi"));
        assert!(pyproject.has_dependency("uvicorn"));
        assert!(pyproject.has_dependency("httpx"));
        assert!(pyproject.has_dependency("pytest"));
        assert!(!pyproject.has_dependency("dev"));
        assert_eq!(
            python_pyproject_test_packages(&pyproject),
            vec!["fastapi", "httpx", "pytest", "uvicorn"]
        );
    }

    #[test]
    fn pyproject_description_keyword_does_not_count_as_pytest_dependency() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname='x'\ndescription='mentions pytest in prose only'\n",
        )
        .expect("pyproject");
        let plan = AutoTestRunner::detect(dir.path(), &["app.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m py_compile 'app.py'");
    }

    #[test]
    fn requirements_comment_keyword_does_not_count_as_pytest_dependency() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("requirements.txt"),
            "# pytest is optional later\n",
        )
        .expect("requirements");
        let plan = AutoTestRunner::detect(dir.path(), &["app.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m py_compile 'app.py'");
    }

    #[test]
    fn python_pytest_uses_uv_when_lockfile_exists() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\ndependencies=['pytest']\n",
        )
        .expect("pyproject");
        std::fs::write(dir.path().join("uv.lock"), "").expect("uv lock");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");

        let plan = AutoTestRunner::detect(dir.path(), &["src/app.py".to_string()]).expect("plan");

        assert_eq!(plan.command, "uv run pytest -p no:cacheprovider");
        assert!(plan.reason.contains("python-toolchain:uv"));
    }

    #[test]
    fn python_pytest_uses_poetry_when_poetry_project_detected() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.poetry]\nname='x'\n[tool.poetry.dependencies]\npytest='*'\n",
        )
        .expect("pyproject");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");

        let plan = AutoTestRunner::detect(dir.path(), &["src/app.py".to_string()]).expect("plan");

        assert_eq!(plan.command, "poetry run pytest -p no:cacheprovider");
        assert!(plan.reason.contains("python-toolchain:poetry"));
    }

    #[test]
    fn python_pytest_uses_hatch_test_script_when_declared() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\ndependencies=['pytest']\n[tool.hatch.envs.default.scripts]\ntest = 'pytest'\n",
        )
        .expect("pyproject");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");

        let plan = AutoTestRunner::detect(dir.path(), &["src/app.py".to_string()]).expect("plan");

        assert_eq!(plan.command, "hatch run test");
        assert!(plan.reason.contains("python-toolchain:hatch-test-script"));
    }

    #[test]
    fn package_json_has_test_script_false_when_file_missing() {
        let dir = tempdir().expect("tempdir");
        assert!(!package_json_has_test_script(dir.path()));
    }

    #[test]
    fn native_node_framework_ignores_keywords_outside_dependency_names() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"description":"next astro svelte vite vue solid keywords only","dependencies":{}}"#,
        )
        .expect("package");
        let candidates =
            AutoTestRunner::detect_candidates(dir.path(), &["src/App.tsx".to_string()]);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.source != VerifierCandidateSource::NativeNodeFramework),
            "unexpected candidates: {candidates:?}"
        );
    }

    #[test]
    fn native_node_framework_uses_dependency_names_as_evidence() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"vite":"5.0.0","solid-js":"1.0.0"}}"#,
        )
        .expect("package");
        let plan = AutoTestRunner::detect(dir.path(), &["src/App.tsx".to_string()]).expect("plan");
        assert!(plan.command.contains("vite build"));
        assert!(plan.reason.contains("source=native_node_framework"));
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

    // -----------------------------------------------------------------
    // Issue #651 Task 2.1: VerifierCommand allowlist + display safety.
    // -----------------------------------------------------------------

    #[test]
    fn verifier_command_from_cargo_test_binds_artifact_paths() {
        // CB-002: `tests/<name>.rs` paths convert to `--test <name>`
        // flags rather than passing the file path verbatim (which cargo
        // would interpret as a test-name filter, not a file path).
        let owned = vec!["tests/test_a.rs".to_string()];
        let command =
            VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo is allowlisted");
        assert_eq!(command.runner(), "cargo");
        assert_eq!(
            command.args(),
            vec![
                "test".to_string(),
                "--test".to_string(),
                "test_a".to_string()
            ]
            .as_slice()
        );
        assert_eq!(command.bound_test_artifacts(), owned.as_slice());
        // Display string joins with single spaces — no `shlex`.
        assert_eq!(command.to_display_string(), "cargo test --test test_a");
    }

    #[test]
    fn verifier_command_from_pytest_binds_artifact_paths() {
        let owned = vec!["app/tests/test_foo.py".to_string()];
        let command = VerifierCommand::from_pytest(vec!["-q".to_string()], &owned)
            .expect("pytest allowlisted");
        assert_eq!(command.runner(), "pytest");
        assert_eq!(
            command.args(),
            vec!["-q".to_string(), "app/tests/test_foo.py".to_string()].as_slice()
        );
        assert_eq!(
            command.to_display_string(),
            "pytest -q app/tests/test_foo.py"
        );
    }

    #[test]
    fn verifier_command_rejects_shell_compound_in_runner_or_args() {
        // Runner outside the allowlist (e.g. `sh`, or pre-joined shell
        // string) must fail to construct via the internal allowlist gate.
        assert!(
            VerifierCommand::new_allowlisted("sh", vec!["-c".into(), "cargo test".into()], vec![])
                .is_none(),
            "sh must not be allowlisted as a verifier runner"
        );
        // Shell-compound `&&` injected in an arg must trip the DR4-002
        // detector and reject the construction.
        let bad_args = vec!["test".to_string(), "&&".to_string(), "rm".to_string()];
        assert!(
            VerifierCommand::new_allowlisted("cargo", bad_args, vec![]).is_none(),
            "arg containing `&&` must trip contains_evidence_poisoning_shell_control"
        );
    }

    #[test]
    fn verifier_command_display_string_for_shell_safe_path_does_not_trip_detector() {
        // Shell-safe (no `;`, `&`, `|`, `<`, `>`, backtick, `$(`, etc.)
        // owned test artifacts produce a display string that itself
        // passes the DR4-002 detector. This pins the invariant that
        // VerifierCommand never round-trips into the
        // `contains_evidence_poisoning_shell_control` reject path.
        let owned = vec!["tests/test_a.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        let display = command.to_display_string();
        assert!(
            !super::super::completion_evidence::contains_evidence_poisoning_shell_control(&display),
            "shell-safe owned test artifact path should produce a shell-safe display string, got {display:?}"
        );
    }

    // -----------------------------------------------------------------
    // Issue #651 Task 2.2: detect_with_owned_test_artifacts dispatch.
    // -----------------------------------------------------------------

    #[test]
    fn detect_owned_for_cargo_project_returns_runnable_with_bound_paths() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let owned = vec!["tests/test_a.rs".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &owned);
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "cargo");
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
                // CB-002: positional `tests/test_a.rs` becomes
                // `--test test_a` so cargo runs the integration test.
                assert_eq!(
                    command.args(),
                    vec![
                        "test".to_string(),
                        "--test".to_string(),
                        "test_a".to_string()
                    ]
                    .as_slice()
                );
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    #[test]
    fn detect_owned_for_stdlib_python_project_returns_runnable_python3() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        let owned = vec!["tests/test_x.py".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["app.py".to_string()],
            &[],
            &owned,
        );
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "python3");
                assert!(command.args().contains(&"pytest".to_string()));
                assert!(
                    command.args().contains(&"tests/test_x.py".to_string()),
                    "owned test artifact must be appended to args, got {:?}",
                    command.args()
                );
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    #[test]
    fn detect_owned_for_project_instruction_returns_weak() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `python3 project_csv_tool.py example.csv`\n",
        )
        .expect("anvil");
        std::fs::write(dir.path().join("project_csv_tool.py"), "print('ok')\n").expect("py");
        std::fs::write(dir.path().join("example.csv"), "Category,Amount\nA,1\n").expect("csv");
        // CB-001 entry-point check requires a non-empty bound list.
        // The owned test artifact (a Python test file) keeps us out of
        // the Missing branch; the candidate detector still selects
        // `project_instruction` and the dispatch maps it to Weak.
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["project_csv_tool.py".to_string()],
            &[],
            &["tests/test_x.py".to_string()],
        );
        match plan {
            OwnedTestVerifierPlan::Weak {
                detected_source, ..
            } => {
                assert_eq!(detected_source, "project_instruction");
            }
            other => panic!("expected Weak, got {other:?}"),
        }
    }

    #[test]
    fn detect_owned_for_recent_successful_bash_only_returns_weak() {
        // No detectable structured project; only a recent successful
        // bash command. The detector must return Weak so the caller
        // does not feed the free-form shell text to `Command::new`.
        // CB-001 requires a non-empty owned list to even reach the
        // candidate-source dispatch.
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.py"), "print('ok')\n").expect("py");
        let recent = vec!["cargo test".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["app.py".to_string()],
            &recent,
            &["tests/test_x.py".to_string()],
        );
        match plan {
            OwnedTestVerifierPlan::Weak {
                detected_source, ..
            } => {
                assert_eq!(detected_source, "recent_successful_bash");
            }
            other => panic!("expected Weak, got {other:?}"),
        }
    }

    #[test]
    fn detect_owned_with_nothing_detected_returns_missing() {
        let dir = tempdir().expect("tempdir");
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &[]);
        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    // -----------------------------------------------------------------
    // Issue #651 Task 2.3: validate_bound_test_artifacts_for_execution
    // + run_structured (spawn + polling timeout).
    // -----------------------------------------------------------------

    fn single_root_scope_for_validation() -> super::super::task_workspace_scope::TaskWorkspaceScope
    {
        super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        }
    }

    #[test]
    fn validate_bound_artifacts_accepts_existing_in_scope_paths() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_x.rs"), "").unwrap();
        let scope = single_root_scope_for_validation();
        let owned = vec!["tests/test_x.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn validate_bound_artifacts_rejects_missing_file_at_execution_time() {
        let dir = tempdir().expect("tempdir");
        let scope = single_root_scope_for_validation();
        // Use `from_pytest` here: it preserves the path verbatim in
        // `bound_test_artifacts`, while `from_cargo_test` rejects any
        // path that does not match the `tests/<stem>.rs` shape
        // (CB-002).
        let owned = vec!["tests/does_not_exist.rs".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("missing file must reject");
        assert!(
            err.contains("missing at execution time"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_bound_artifacts_rejects_absolute_path() {
        // The pytest constructor stores `bound_test_artifacts` verbatim,
        // so we can stage a bad path that bypasses planning-time checks.
        // (CB-002 prevents `from_cargo_test` from being used for this.)
        let dir = tempdir().expect("tempdir");
        let scope = single_root_scope_for_validation();
        let owned = vec!["/etc/passwd".to_string()];
        // /etc/passwd contains shell-safe characters only — the
        // allowlist constructor accepts it; the execution-time
        // validator must still reject.
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("absolute path must reject");
        assert!(err.contains("must be relative"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn validate_bound_artifacts_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().expect("outside");
        std::fs::write(outside.path().join("secret.rs"), "").unwrap();
        let work = tempdir().expect("work");
        symlink(
            outside.path().join("secret.rs"),
            work.path().join("alias.rs"),
        )
        .expect("symlink");
        let scope = single_root_scope_for_validation();
        let owned = vec!["alias.rs".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(work.path(), &scope, &command);
        let err = result.expect_err("symlink escape must reject");
        assert!(
            err.contains("canonicalization escapes work_root"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_structured_kills_child_on_timeout() {
        // Spawn /bin/sleep 60 and force a 50ms timeout to exercise the
        // kill path. We cannot easily build a VerifierCommand for sleep
        // (not on the allowlist), so we go through the lower-level
        // helper directly. This pins the polling-timeout contract.
        let mut command = Command::new("/bin/sleep");
        command
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let result = wait_with_auto_test_timeout(&mut command, Duration::from_millis(50));
        let err = result.expect_err("timed-out sleep must Err");
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}"
        );
    }

    // -----------------------------------------------------------------
    // Issue #651 PR-003 (Medium): timeout must kill the entire process
    // group on Unix, not just the immediate child. We spawn a shell
    // that prints its own pid and the pid of a backgrounded sleep,
    // forces a short timeout, then verifies both processes are gone.
    // The shell is the direct child; its `sleep` descendant is what
    // the old `child.kill()` would have leaked.
    // -----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn run_structured_kills_process_group_on_unix_timeout() {
        use std::io::{BufRead, BufReader};
        use std::time::Instant;

        // Spawn `sh -c 'sleep 120 & echo $!; wait'` so the shell prints
        // the descendant's pid on stdout, then waits indefinitely. Old
        // `child.kill()` would kill `sh` only, leaving `sleep` alive.
        // The PR-003 fix sends SIGKILL to the negative pgid, so both
        // the shell and the sleep go away.
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            // `exec sleep ... & echo $!; wait $!` — `exec` is not used
            // because we need the parent shell to print the pid AND
            // remain alive long enough for the parent to read it.
            .arg("sleep 120 & echo $! ; wait $!")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Apply the same pgroup setup `run_structured` uses.
        crate::tools::bash::apply_unix_pgroup(&mut command);

        let mut child = command.spawn().expect("spawn sh");
        let stdout = child.stdout.take().expect("stdout piped");
        let descendant_pid: i32 = {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            // Bounded read so a misbehaving shell does not hang the test.
            let read_deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if Instant::now() > read_deadline {
                    panic!("timed out reading descendant pid from shell stdout");
                }
                line.clear();
                let n = reader.read_line(&mut line).expect("read line");
                if n == 0 {
                    panic!("shell closed stdout before printing descendant pid");
                }
                let trimmed = line.trim();
                if let Ok(pid) = trimmed.parse::<i32>() {
                    break pid;
                }
            }
        };
        assert!(descendant_pid > 0, "descendant pid must be positive");

        // Now drive the kill path. Re-attach the (already-consumed)
        // stdout role is not required for `kill_auto_test_child_tree`
        // since it only signals; it does not drain output.
        kill_auto_test_child_tree(&mut child);
        let _ = child.wait();

        // The shell PID and the sleep PID should both be gone now.
        // `kill(0, SIG=0)` is the canonical existence check (signal 0
        // is a no-op delivery, error 3 = ESRCH = no such process).
        let descendant_alive_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            // SAFETY: libc FFI; no Rust state mutated.
            let res = unsafe { libc::kill(descendant_pid, 0) };
            if res != 0 {
                // ESRCH expected — descendant is gone.
                break;
            }
            if Instant::now() > descendant_alive_deadline {
                // Try one more SIGKILL on the descendant directly so the
                // test does not leave a stray sleep behind in the
                // unlikely case the OS has not reaped yet, then fail.
                unsafe {
                    let _ = libc::kill(descendant_pid, libc::SIGKILL);
                }
                panic!(
                    "PR-003 regression: descendant sleep pid {descendant_pid} survived process-group SIGKILL"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // -----------------------------------------------------------------
    // Issue #651 CB-001: empty owned_test_artifacts must produce
    // OwnedTestVerifierPlan::Missing (not Runnable, not Weak).
    // -----------------------------------------------------------------

    #[test]
    fn detect_owned_with_cargo_project_and_empty_artifacts_returns_missing() {
        // Cargo.toml is present, so the candidate detector finds
        // `CargoManifest`. Without any bound test artifact path,
        // however, `detect_with_owned_test_artifacts` must drop to
        // `Missing` per CB-001 — running `cargo test` unbound would
        // produce a false-positive completion when the suite happens
        // to be empty / pre-existing.
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &[]);
        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    #[test]
    fn detect_owned_with_python_project_and_empty_artifacts_returns_missing() {
        // Python stdlib pytest project (tests/ dir present) with no
        // owned test artifact must also Missing rather than running
        // `python3 -m pytest` with zero positional paths (which would
        // scan the rootdir indiscriminately).
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["app.py".to_string()],
            &[],
            &[],
        );
        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    #[test]
    fn validate_bound_artifacts_rejects_empty_bound_paths() {
        // `bound_test_artifacts` is normally guaranteed non-empty by
        // the constructors, but the execution-time validator must
        // independently reject an empty list (CB-001 defense in depth).
        // We build the command via the from_pytest path with a single
        // path, then exercise the empty-list branch through a hand-
        // assembled `VerifierCommand` via `new_allowlisted` with no
        // bound artifacts.
        let dir = tempdir().expect("tempdir");
        let scope = single_root_scope_for_validation();
        let command =
            VerifierCommand::new_allowlisted("pytest", vec!["-q".to_string()], vec![]).expect("ok");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("empty bound list must reject");
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    // -----------------------------------------------------------------
    // Issue #651 CB-002: cargo positional arg is a name filter, not
    // a path. `from_cargo_test` must convert `tests/<stem>.rs` →
    // `--test <stem>` and reject paths it cannot safely convert.
    // -----------------------------------------------------------------

    #[test]
    fn verifier_command_from_cargo_test_converts_tests_dir_to_test_flag() {
        let owned = vec!["tests/integration_one.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        assert_eq!(
            command.args(),
            vec![
                "test".to_string(),
                "--test".to_string(),
                "integration_one".to_string()
            ]
            .as_slice(),
            "tests/<stem>.rs must map to `--test <stem>` (CB-002)"
        );
    }

    #[test]
    fn verifier_command_from_cargo_test_rejects_src_internal_path_returns_none() {
        // src/... is a unit-test module path; cargo cannot run an
        // arbitrary file under `src/` as an integration test, so the
        // constructor must return None (and the caller falls back to
        // Weak).
        let owned = vec!["src/lib/foo.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned);
        assert!(command.is_none(), "src/... path must not be convertible");
    }

    #[test]
    fn verifier_command_from_cargo_test_rejects_non_rs_extension_returns_none() {
        // `.py`, `.toml`, etc. cannot be an integration test file —
        // the helper must refuse to fabricate a `--test <stem>` flag.
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned);
        assert!(command.is_none(), "non-.rs path must not be convertible");
    }

    #[test]
    fn verifier_command_from_cargo_test_rejects_nested_tests_path_returns_none() {
        // tests/sub/dir.rs is a sub-directory integration file. cargo's
        // `--test <name>` flag does not address those; reject so the
        // caller drops to Weak rather than fabricate a misleading flag.
        let owned = vec!["tests/sub/dir.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned);
        assert!(
            command.is_none(),
            "nested tests/<sub>/<file>.rs must not be convertible"
        );
    }

    #[test]
    fn detect_owned_for_cargo_project_with_unconvertible_path_returns_weak() {
        // A Cargo project with an owned test artifact under src/...
        // means `from_cargo_test` returns None. The detector must
        // surface this as `Weak` so the caller never executes an
        // unbound `cargo test` (CB-002).
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let owned = vec!["src/lib/foo.rs".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &owned);
        match plan {
            OwnedTestVerifierPlan::Weak {
                detected_source, ..
            } => {
                assert_eq!(detected_source, "cargo_manifest");
            }
            other => panic!("expected Weak, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Issue #651 CB-003: timeout polling must concurrently drain
    // stdout/stderr so a chatty child cannot block on pipe buffer.
    // -----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn run_structured_drains_large_output_without_pipe_block() {
        // The previous implementation kept stdout piped without
        // reading it, so a child that writes more than the OS pipe
        // buffer (~64 KiB on Linux/macOS) blocks on `write()` and
        // never exits, eventually surfacing as a spurious timeout.
        //
        // We emit ~512 KiB to stdout from a tiny shell command, then
        // exit 0. With concurrent drain in place the helper must
        // collect the full output and return Ok within the timeout.
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            // 1024 lines of ~512 bytes each → ~512 KiB on stdout.
            .arg("i=0; while [ $i -lt 1024 ]; do printf '%s\\n' \"$(printf '%.0sa' $(seq 1 500))\"; i=$((i+1)); done")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let result = wait_with_auto_test_timeout(&mut command, Duration::from_secs(10))
            .expect("large stdout must not deadlock with concurrent drain (CB-003)");
        assert!(result.status.success(), "shell must exit 0");
        assert!(
            result.stdout.len() >= 500 * 1024,
            "expected ~512 KiB of stdout, got {} bytes",
            result.stdout.len()
        );
    }

    // -----------------------------------------------------------------
    // Issue #651 CB-004: execution-time validator must re-apply the
    // ignored-top-dir rule (node_modules / .git / target / ...).
    // -----------------------------------------------------------------

    #[test]
    fn validate_bound_artifacts_rejects_node_modules_path() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/test.js"), "").unwrap();
        let scope = single_root_scope_for_validation();
        let owned = vec!["node_modules/pkg/test.js".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("node_modules path must reject");
        assert!(
            err.contains("ignored workspace directory") && err.contains("node_modules"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_bound_artifacts_rejects_dot_git_path() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
        std::fs::write(dir.path().join(".git/hooks/test.py"), "").unwrap();
        let scope = single_root_scope_for_validation();
        let owned = vec![".git/hooks/test.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err(".git path must reject");
        assert!(
            err.contains("ignored workspace directory") && err.contains(".git"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_bound_artifacts_rejects_target_dir_path() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/test_x.rs"), "").unwrap();
        let scope = single_root_scope_for_validation();
        let owned = vec!["target/debug/test_x.rs".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("target/ path must reject");
        assert!(
            err.contains("ignored workspace directory") && err.contains("target"),
            "unexpected error: {err}"
        );
    }

    // -----------------------------------------------------------------
    // Issue #651 Phase 7: E2E observation tests (5 acceptance criteria).
    //
    // These pin the end-to-end Issue #651 receipt conditions at the
    // structured-verifier seam. Ollama is not required; every scenario
    // exercises pure dispatch / construction logic of the
    // `OwnedTestVerifierPlan` family.
    // -----------------------------------------------------------------

    use super::super::task_contract::{CompletionDecision, SafeStopReason, TaskContract};

    /// 受入条件 1: a request that literally asks for tests AND only has
    /// a `py_compile`-style fallback (no allowlisted runner) MUST NOT
    /// reach `CompletionDecision::Done` — `evaluate_with_owned_test_artifacts`
    /// returns `SafeStop { reason: VerifierMissing }` when no owned
    /// test artifact bound to a structured runner.
    #[test]
    fn e2e_651_001_py_compile_only_with_required_tests_does_not_reach_done() {
        let contract = TaskContract::from_request(
            "Implement a small feature and add a test for it (tests required).",
        );
        assert!(
            contract.required_behavior.test_execution_required,
            "request must mark test_execution_required"
        );
        // Simulate "all required artifacts observed + verifier passed"
        // — the only failure mode left is "no owned test artifact bound".
        use super::super::completion_evidence::{
            CompletionEvidence, EvidenceSet, RepoEditCategory,
        };
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Impl,
            count: 1,
        });
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Test,
            count: 1,
        });
        evidence.push(CompletionEvidence::VerifierExitZero {
            class: crate::tools::bash::BashCommandClass::BuildTest,
            command: "python3 -m py_compile app.py".to_string(),
            bound_test_artifacts_count: None,
        });
        let decision = contract.evaluate_with_owned_test_artifacts(&evidence, &[]);
        assert_eq!(
            decision,
            CompletionDecision::SafeStop {
                reason: SafeStopReason::VerifierMissing,
            },
            "py_compile + required-tests + empty owned must SafeStop, got {decision:?}"
        );
    }

    /// 受入条件 2: when an owned Python test artifact is staged,
    /// `OwnedTestVerifierPlan::Runnable.command.bound_test_artifacts`
    /// MUST contain that path verbatim.
    #[test]
    fn e2e_651_002_owned_test_artifact_appears_in_bound_test_artifacts() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        let owned = vec!["tests/test_x.py".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["app.py".to_string()],
            &[],
            &owned,
        );
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    /// 受入条件 3: symlink / absolute / `..` paths are filtered upstream
    /// at the `artifact_ownership::owned_test_artifacts` SSOT boundary
    /// (see the dedicated unit tests in `artifact_ownership.rs`). At the
    /// `VerifierCommand` constructor level we additionally require that
    /// any unsafe path which somehow reached `bound_test_artifacts` is
    /// rejected at execution time. This test pins the absolute-path
    /// reject because that is the most likely path an LLM could
    /// fabricate.
    #[test]
    fn e2e_651_003_unsafe_path_does_not_reach_execution() {
        // VerifierCommand constructors take the path as-is from the
        // upstream ownership filter. We simulate a leaked absolute
        // path passing the constructor (e.g. a future regression in
        // owned_test_artifacts) and assert that the execution-time
        // validator rejects it.
        let dir = tempdir().expect("tempdir");
        let scope = single_root_scope_for_validation();
        let owned = vec!["/etc/passwd".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest accepts");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        let err = result.expect_err("absolute path must reject");
        assert!(
            err.contains("absolute"),
            "expected absolute-path rejection, got {err}"
        );
    }

    /// 受入条件 4: an auto-detected `cargo test` runner does NOT
    /// silently satisfy the Issue #651 invariant when the owned test
    /// artifact slice is empty — the dispatch returns `Missing`, not
    /// `Runnable`, so the caller maps it to `verifier_missing`.
    #[test]
    fn e2e_651_004_auto_detected_cargo_test_with_no_owned_artifact_is_missing() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        // Cargo would otherwise be Runnable, but with an empty owned
        // slice the entry-point guard (CB-001 defense-in-depth) flips
        // it to Missing.
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &[]);
        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    /// 受入条件 5: an LLM-fabricated shell-style command string never
    /// reaches `Command::new(...).args(...)`. `VerifierCommand::new_allowlisted`
    /// is the only allowlisted constructor, and it rejects:
    ///   - runners outside the allowlist (e.g. `sh -lc ...`)
    ///   - any arg containing shell metacharacters (`&&`, `|`, `;`, ...)
    #[test]
    fn e2e_651_005_llm_generated_verifier_command_string_never_reaches_shell() {
        // sh outside the allowlist: caller cannot fabricate
        // `sh -lc "rm -rf /"` even if it tries.
        assert!(
            VerifierCommand::new_allowlisted(
                "sh",
                vec!["-lc".to_string(), "rm -rf /".to_string()],
                vec![],
            )
            .is_none(),
            "sh must not be allowlisted"
        );
        // bash same — outside the allowlist.
        assert!(
            VerifierCommand::new_allowlisted(
                "bash",
                vec!["-c".to_string(), "cargo test".to_string()],
                vec![],
            )
            .is_none(),
            "bash must not be allowlisted"
        );
        // Allowlisted runner + shell-control args still reject.
        let cases = vec![
            vec!["test".to_string(), "&&".to_string(), "rm".to_string()],
            vec!["test".to_string(), "|".to_string(), "cat".to_string()],
            vec!["test".to_string(), ";".to_string(), "echo".to_string()],
            vec!["test".to_string(), "`whoami`".to_string()],
            vec!["test".to_string(), "$(id)".to_string()],
        ];
        for args in cases {
            assert!(
                VerifierCommand::new_allowlisted("cargo", args.clone(), vec![]).is_none(),
                "args with shell control must be rejected: {args:?}"
            );
        }
    }
}
