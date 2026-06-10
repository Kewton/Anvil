use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::agent::prompting::load_project_instructions;
use crate::logging::log_llm_event;
use crate::session::feedback::FeedbackKind;
use crate::util::workspace_paths::is_ignored_workspace_display_path;

use super::completion_evidence::is_completion_verifier_command;
use super::project_probe::ProjectUnit;
use super::task_workspace_scope::TaskWorkspaceScope;
use super::verifier_command_policy::PythonProjectUnitVerifierFlavor;

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
    pub(super) fn as_str(self) -> &'static str {
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

    fn from_project_unit_source(source: &str) -> Option<Self> {
        match source {
            "cargo_manifest" => Some(Self::CargoManifest),
            "package_json_scripts" => Some(Self::PackageJsonScripts),
            "python_tests" => Some(Self::PythonTests),
            _ => None,
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
/// Issue #661 Task 2.2 (DR1-008 OCP 拡張口 / DR2-004 serialize 戦略):
/// `OwnedTestVerifierPlan::Runnable` 内の runner kind 表現。`as_str()` is
/// the **only** serialization path for the `agent.verifier.invoked` event
/// payload `runner` field. `#[non_exhaustive]` blocks external `match`
/// wildcard arms — new variants (Mocha / Vitest / ...) require an explicit
/// `as_str()` arm so payload schema drift is caught at compile time.
///
/// Security gate (DR4-004): runnable authorization is also discriminated on
/// `RunnerKind`. A new variant MUST land together with (1) the matching
/// `VerifierCommand::from_<runner>` constructor, (2) the
/// `append_bound_path_args_for_runner` / execution-time validator match
/// arm, and (3) the runner-hijack validation test. Until those land the
/// variant must not appear in production-emitting paths.
///
/// Phase A scope (#661 iteration-2): the enum is defined and the
/// `as_str()` mapping is pinned. Wiring into `OwnedTestVerifierPlan` and
/// into the pre-spawn snapshot follows in iteration-3.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Iteration-3 instantiates these variants when wiring `OwnedTestVerifierPlan::Runnable` and the `agent.verifier.invoked` payload. Until then only unit tests construct them.
pub(super) enum RunnerKind {
    Cargo,
    Python3,
    Npm,
}

impl RunnerKind {
    /// Payload schema strings. Must match Section 8-1 of the design doc and
    /// the `runner` field documented for the `agent.verifier.invoked` event.
    #[allow(dead_code)] // Iteration-3 wires the production caller (`agent.verifier.invoked` payload emit); until then only unit tests consume this.
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            RunnerKind::Cargo => "cargo",
            RunnerKind::Python3 => "python3",
            RunnerKind::Npm => "npm",
        }
    }

    /// Issue #661 iteration-4 (Task 2.5 + 5.2): map a `VerifierCommand.runner()`
    /// string back to its `RunnerKind` discriminant. Returns `None` for runners
    /// that have no `RunnerKind` variant yet (e.g. `pytest` bare invocations
    /// constructed in unit tests only, or future runners landing through a
    /// `from_*` constructor before their `RunnerKind` arm and security gate
    /// are added together, DR4-004). Production `OwnedTestVerifierPlan::Runnable`
    /// only yields `cargo` / `python3` today, so this stays a total function over
    /// the production runner string set while the security gate keeps unknown
    /// runners out of `agent.verifier.invoked` payloads.
    #[allow(dead_code)] // wired by `VerifierInvokedSnapshot::from_command_and_env` (iteration-4 Task 5.2 producer site)
    pub(super) fn from_runner_str(runner: &str) -> Option<Self> {
        match runner {
            "cargo" => Some(RunnerKind::Cargo),
            "python3" => Some(RunnerKind::Python3),
            "npm" => Some(RunnerKind::Npm),
            _ => None,
        }
    }
}

/// Issue #661 Task 2.3 (DR1-012 / DR2-007): 16-hex `stable_path_hash`
/// 出力の newtype wrapper。`from_relative_str` 経由でしか構築できないため、
/// raw path がそのまま `agent.verifier.invoked` の `bound_artifacts[].path_hash`
/// に流れ込むことをコンパイル時にブロックする。SSOT は
/// `crate::logging::stable_path_hash`（`mask_secrets` 前置で 16-hex
/// `DefaultHasher` 出力に統一）。#659 / #660 / #661 はすべて同じ representation
/// を共有する (iteration-1 で SSOT 昇格済)。
#[derive(Debug, Clone)]
pub(super) struct PathHashHex(String);

impl PathHashHex {
    /// workspace-relative path 文字列を `mask_secrets` で前処理した上で
    /// `crate::logging::stable_path_hash` に流す。caller の raw path
    /// (絶対 path / .. / secret-bearing) は mask 後でも 16-hex 出力に均一化
    /// される。
    #[allow(dead_code)] // Iteration-3 wires the production caller (`VerifierInvokedSnapshot.bound_artifacts`); until then only unit tests consume this.
    pub(super) fn from_relative_str(s: &str) -> Self {
        Self(crate::logging::stable_path_hash(
            &crate::session::feedback::mask_secrets(s),
        ))
    }

    /// 16-hex の borrowed view。caller は payload に直接埋めるだけで、独自
    /// に format したり raw path を交ぜたりしない。
    #[allow(dead_code)] // Iteration-3 wires the production caller (`agent.verifier.invoked` payload emit); until then only unit tests consume this.
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Issue #661 iteration-4 Task 2.4 (DR1-001 SRP) /
/// iteration-5 Task 6.3 (Phase B 実値 populate):
/// `HermeticEnvPlan` の `summary()` が返す観測値。`agent.verifier.invoked`
/// event の `env_summary` field と root-level `cwd_inside_work_root` を
/// 構築するための純粋な観測 view。
///
/// Phase B 観測実値:
/// - `allowlist_keys`: `filter_env_for_tester(std::env::vars())` で実際に
///   注入された key の `'static str` 列 (DR1-009: `TESTER_ENV_ALLOWLIST_EXACT`
///   の slice は直接参照しない — `filter_env_for_tester` の戻り値 key 名を
///   `TESTER_ENV_ALLOWLIST_EXACT` の対応する `'static str` に解決する経路でのみ
///   `&'static str` に昇格する)
/// - `pythonpath_root`: `pythonpath_root: Option<PathBuf>` が `Some` か
/// - `cwd_inside_work_root`: `self.cwd == self.work_root_for_summary`
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired by `VerifierInvokedSnapshot::from_command_and_env`
pub(super) struct HermeticEnvSummary {
    pub allowlist_keys: Vec<&'static str>,
    pub pythonpath_root: bool,
    pub cwd_inside_work_root: bool,
}

/// Issue #661 iteration-5 Task 7.1 (pre-execution PYTHONPATH 検査):
/// `build_hermetic_env_plan` が PYTHONPATH に work_root 外の path を発見した
/// 場合、`HermeticEnvPlan` 内に rejection signal を保持する。raw path は
/// payload には出さず、emit site で `external_pythonpath_rejected` reason
/// として再構成する。
///
/// Phase B 構築不変条件 (DR1-001 SRP):
/// - `allow_entries`: `filter_env_for_tester(std::env::vars())` を `apply_to`
///   時に毎回 fresh に評価する (caller process env を snapshot として hold する
///   設計だが、`build_hermetic_env_plan` 時点で評価して `Vec<(String, String)>`
///   に固定する。caller env の途中変更は反映しない、これは hermetic 観点で
///   むしろ望ましい invariant)
/// - `extras`: caller (Python adapter / Cargo adapter) が決める固定 closed slice
/// - `pythonpath_root`: `apply_to` が `cmd.env("PYTHONPATH", root)` で挿入。
///   `None` のときは `apply_to` が `cmd.env_remove("PYTHONPATH")` を呼ぶ
/// - `cwd`: `work_root` 自身
/// - `rejected_pythonpath`: PYTHONPATH に外部 path を発見した場合の signal。
///   `apply_to` 自体は依然として work_root cwd / allowlist 再注入の副作用を
///   適用するが、caller (`run_structured` / event emit) が rejection を観測する
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired by `run_structured` and `VerifierInvokedSnapshot::from_command_and_env`
pub(super) struct HermeticEnvPlan {
    allow_entries: Vec<(String, String)>,
    extras: Vec<(&'static str, &'static str)>,
    pythonpath_root: Option<PathBuf>,
    cwd: PathBuf,
    /// PYTHONPATH に work_root 外の path を発見した場合に raw substring
    /// (mask 前) を保持。raw value は `apply_to` では使われず、emit site で
    /// `external_pythonpath_rejected` event の detected_modules に hash 化
    /// される。Phase B Task 7.1 で導入。
    rejected_pythonpath: Option<String>,
}

impl HermeticEnvPlan {
    /// `Command` への副作用適用 (SRP)。Phase B 本実装:
    /// 1. `cmd.env_clear()` で parent env を baseline reset
    /// 2. `allow_entries` で allowlist key 群を再注入 (PATH / HOME / LANG / ...)
    /// 3. `extras` で runner-specific extra key (PYTHONDONTWRITEBYTECODE 等)
    /// 4. PYTHONPATH 制御: `pythonpath_root` が `Some` なら `cmd.env("PYTHONPATH", root)`、
    ///    `None` なら `cmd.env_remove("PYTHONPATH")` で明示的に drop
    /// 5. `cmd.current_dir(&self.cwd)` で work_root に固定
    ///
    /// DR1-009 invariant: `allow_entries` の中身は `build_hermetic_env_plan`
    /// で `filter_env_for_tester` 経由のみで構築。`apply_to` 自身は slice の
    /// key 名を直接参照しない。
    #[allow(dead_code)] // wired by `run_structured` and Phase B verifier_skill execution
    pub(super) fn apply_to(&self, cmd: &mut std::process::Command) {
        cmd.env_clear();
        for (k, v) in &self.allow_entries {
            cmd.env(k, v);
        }
        for (k, v) in &self.extras {
            cmd.env(k, v);
        }
        match &self.pythonpath_root {
            Some(root) => {
                cmd.env("PYTHONPATH", root);
            }
            None => {
                cmd.env_remove("PYTHONPATH");
            }
        }
        cmd.current_dir(&self.cwd);
    }

    /// event payload 用 summary を純関数で返す (SRP)。Phase B 観測実値:
    /// - `allowlist_keys`: `allow_entries` の key 名を `&'static str` に昇格
    ///   (DR1-009: `crate::tools::bash::tester_allowlist_static_key` 経由で
    ///   SSOT を bash.rs に閉じる — `TESTER_ENV_ALLOWLIST_EXACT` の slice を
    ///   直接参照しない)
    /// - `pythonpath_root`: `self.pythonpath_root` が Some か
    /// - `cwd_inside_work_root`: build 時に `cwd = work_root` で常に true
    #[allow(dead_code)] // wired by `VerifierInvokedSnapshot::from_command_and_env`
    pub(super) fn summary(&self) -> HermeticEnvSummary {
        // DR1-009: `allow_entries` の `String` key 名から `&'static str`
        // への昇格は `crate::tools::bash::tester_allowlist_static_key` 経由
        // のみで実行する (SSOT は bash.rs 側に閉じる)。`filter_env_for_tester`
        // の戻り値 key は allowlist 通過済みなので常に `Some(...)` を返す
        // 経路だが、defense-in-depth で `None` 時は drop する。
        let allowlist_keys: Vec<&'static str> = self
            .allow_entries
            .iter()
            .filter_map(|(k, _)| crate::tools::bash::tester_allowlist_static_key(k.as_str()))
            .collect();
        let pythonpath_root = self.pythonpath_root.is_some();
        // build path always sets `cwd = work_root` (see `build_hermetic_env_plan`).
        let cwd_inside_work_root = true;
        HermeticEnvSummary {
            allowlist_keys,
            pythonpath_root,
            cwd_inside_work_root,
        }
    }

    /// Phase B Task 7.1: PYTHONPATH に external path が発見された場合の
    /// rejection summary。raw substring は出さず、`source_kind=pythonpath`
    /// の検出が発生したか否か (bool) と detected_count を返す。
    #[allow(dead_code)] // wired by `run_structured` / event emit site
    pub(super) fn rejected_pythonpath_hash(&self) -> Option<String> {
        self.rejected_pythonpath.as_ref().map(|raw| {
            crate::logging::stable_path_hash(&crate::session::feedback::mask_secrets(raw))
        })
    }

    /// Phase B Task 6.4: hermetic env の planned PATH を borrow して返す。
    /// `VerifierCommand::resolved_runner_program` で runner 絶対 path 解決に
    /// 使う。`allow_entries` 経由 (DR1-009: `TESTER_ENV_ALLOWLIST_EXACT` を
    /// 直接参照しない) で PATH を抽出する。
    #[allow(dead_code)] // wired by `run_structured` / verifier_skill spawn site
    pub(super) fn planned_path(&self) -> Option<&str> {
        self.allow_entries
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str())
    }
}

/// Issue #661 iteration-5 Task 6.2 (DR1-006 Phase B): Python verifier path
/// extras. `PYTHONDONTWRITEBYTECODE=1` disables `.pyc` file generation
/// (avoid `__pycache__` pollution under work_root), `PYTHONNOUSERSITE=1`
/// blocks `~/.local/lib/python*/site-packages` from polluting `sys.path`,
/// and `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1` prevents unrelated third-party
/// pytest plugins from mutating collection/import behavior.
///
/// Cargo path uses `&[]` (empty extras). The slice is a private const so
/// no LLM/recent-shell input can flow into the extras list (DR4-002).
#[allow(dead_code)] // wired by `from_python3_pytest_stdlib` adapter (iteration-5 Task 6.2)
pub(super) const VERIFIER_ENV_PYTHON_EXTRA: &[(&str, &str)] = &[
    ("PYTHONDONTWRITEBYTECODE", "1"),
    ("PYTHONNOUSERSITE", "1"),
    ("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1"),
];

/// Issue #661 iteration-5 Task 6.1 (Phase B hermetic env 本実装):
/// 純粋に hermetic env plan を構築する。
///
/// Phase B 動作:
/// - `allow_entries` を `filter_env_for_tester(std::env::vars())` で構築
/// - `extras` を caller-provided closed slice からコピー (Phase B では Python
///   adapter が `VERIFIER_ENV_PYTHON_EXTRA` を渡し、Cargo adapter は `&[]`)
/// - `pythonpath_root` は parent PYTHONPATH と runner extras から Some/None。
///   - PYTHONPATH 未設定 or 空かつ non-Python runner → `None` (child から remove)
///   - Python verifier extras がある → `Some(work_root)` に固定
///   - PYTHONPATH に external (work_root 外) component を発見 → `Some(work_root)`
///     に上書きしつつ、`rejected_pythonpath` に raw substring を保持
///   - PYTHONPATH 全要素が work_root 配下 → `Some(work_root)`
/// - `cwd` は `work_root` 自身
#[allow(dead_code)] // wired by `VerifierInvokedSnapshot::from_command_and_env`
pub(super) fn build_hermetic_env_plan(
    work_root: &Path,
    extras: &[(&'static str, &'static str)],
) -> HermeticEnvPlan {
    let allow_entries = crate::tools::bash::filter_env_for_tester(std::env::vars());
    let extras_vec: Vec<(&'static str, &'static str)> = extras.to_vec();
    // Phase B Task 7.1: pre-execution PYTHONPATH 検査。parent process env から
    // PYTHONPATH を読み取り、各要素が work_root 配下かを判定。
    let (mut pythonpath_root, rejected_pythonpath) = evaluate_pythonpath_for_hermetic_env(
        work_root,
        std::env::var("PYTHONPATH").ok().as_deref(),
    );
    if pythonpath_root.is_none()
        && extras
            .iter()
            .any(|(key, _)| *key == "PYTHONNOUSERSITE" || *key == "PYTEST_DISABLE_PLUGIN_AUTOLOAD")
    {
        pythonpath_root = Some(work_root.to_path_buf());
    }
    HermeticEnvPlan {
        allow_entries,
        extras: extras_vec,
        pythonpath_root,
        cwd: work_root.to_path_buf(),
        rejected_pythonpath,
    }
}

/// Issue #661 iteration-5 Task 7.1: pure function over a PYTHONPATH string.
/// Returns `(pythonpath_root, rejected_pythonpath)`:
/// - `pythonpath_root = Some(work_root)` if any PYTHONPATH component is set
///   (including external ones — we still pin PYTHONPATH to work_root so the
///   child cannot import from the external paths)
/// - `pythonpath_root = None` if PYTHONPATH was unset/empty
/// - `rejected_pythonpath = Some(raw_substring)` if any component resolves
///   outside `work_root` (excluding well-known safe roots like `~/.pyenv`,
///   `~/.local/share/uv`, target/cache dirs which never appear in PYTHONPATH
///   in normal projects but are kept here defensively)
///
/// The raw substring is mask_secrets'd at emit time; this function returns
/// the unmasked form so the caller can apply mask_secrets in the same pass
/// as stable_path_hash.
#[allow(dead_code)] // wired by `build_hermetic_env_plan`
fn evaluate_pythonpath_for_hermetic_env(
    work_root: &Path,
    raw_pythonpath: Option<&str>,
) -> (Option<PathBuf>, Option<String>) {
    // CB-010 (Codex iteration-5 medium): use the platform separator instead
    // of a hard-coded `:` so Windows PYTHONPATH (`;`-separated) is parsed
    // correctly and drive letters (`C:`) are not split. On Unix the
    // separator is still `:`.
    evaluate_pythonpath_with_separator(work_root, raw_pythonpath, pythonpath_separator())
}

/// Returns the platform PYTHONPATH separator character. On Unix this is
/// `:`; on Windows it is `;`. Mirrors `std::env::join_paths` semantics.
fn pythonpath_separator() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

/// CB-010 (Codex iteration-5 medium): pure parser variant of
/// `evaluate_pythonpath_for_hermetic_env` that takes the separator as a
/// parameter so unit tests can drive both POSIX (`:`) and Windows (`;`)
/// shapes deterministically without touching the host platform.
///
/// Behavioral invariant identical to the production wrapper:
/// - Empty / unset PYTHONPATH → `(None, None)`
/// - Any component present → `pythonpath_root = Some(work_root)` so the
///   child cannot inherit external paths
/// - First external (non-work_root) component → `rejected = Some(raw)`
/// - Empty components (e.g. trailing `:`) are skipped, `.` is treated as
///   work_root
#[allow(dead_code)] // wired by `evaluate_pythonpath_for_hermetic_env` + unit tests
fn evaluate_pythonpath_with_separator(
    work_root: &Path,
    raw_pythonpath: Option<&str>,
    separator: char,
) -> (Option<PathBuf>, Option<String>) {
    let raw = match raw_pythonpath {
        Some(s) if !s.is_empty() => s,
        _ => return (None, None),
    };
    // Canonicalize work_root once for prefix comparison. Falls back to the
    // raw `to_path_buf` form if canonicalize fails (e.g. tempdir cleanup
    // race) — the comparison stays strict-equality / starts_with-based.
    let work_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let mut rejected: Option<String> = None;
    for component in raw.split(separator) {
        let trimmed = component.trim();
        if trimmed.is_empty() || trimmed == "." {
            continue;
        }
        // Treat relative paths as relative to work_root.
        let path = if Path::new(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            work_canon.join(trimmed)
        };
        let canon = std::fs::canonicalize(&path).unwrap_or(path);
        if !canon.starts_with(&work_canon) {
            // First external component wins; raw substring retained for hash.
            rejected = Some(trimmed.to_string());
            break;
        }
    }
    // Always pin PYTHONPATH to work_root once any component is present —
    // child cannot reach external imports through PYTHONPATH even if the
    // hermetic env rejection signal is observed asynchronously.
    (Some(work_root.to_path_buf()), rejected)
}

/// Issue #661 iteration-5 Task 6.4 (DR4-001 PATH hijack 防止) /
/// CB-008 (Codex iteration-5 high): resolve a runner binary against a
/// **sanitized** `PATH` string and return the absolute path of the first
/// executable hit. CB-008 hardens this to fail-closed: callers SHOULD
/// route through `resolve_runner_in_sanitized_path` (which applies
/// `sanitize_path_for_runner_resolution` before scanning) so a hostile
/// component in the parent PATH cannot hijack the runner.
///
/// The lookup follows `:`-separated POSIX semantics; on Windows callers
/// should not use this (Phase B target platforms are Unix-only for the
/// hermetic verifier path).
#[allow(dead_code)] // wired by `VerifierCommand::resolved_runner_path`
fn resolve_runner_in_path(path_value: &str, runner: &str) -> Option<PathBuf> {
    if runner.is_empty() || runner.contains('/') {
        // Already-absolute or contains-slash runner names are not subject to
        // PATH search; return as-is when absolute, otherwise reject.
        let p = Path::new(runner);
        if p.is_absolute() {
            return Some(p.to_path_buf());
        }
        return None;
    }
    for dir in path_value.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(runner);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// CB-008 (Codex iteration-5 high, DR4-001 fail-closed): sanitize a raw
/// `PATH` string so only `absolute + canonicalize-able` components that
/// do NOT live under `work_root` survive. Empty / relative / work_root-
/// anchored components are dropped. The output is joined back with
/// `:` (POSIX) so callers can keep their `:`-split lookup unchanged.
///
/// Rationale: trusting the parent process PATH for runner resolution
/// allows a hostile workspace-local binary (e.g. `<work_root>/bin/python3`)
/// to hijack the verifier when the parent CWD inadvertently placed
/// `<work_root>/bin` ahead of `/usr/bin`. The sanitized PATH never
/// contains such components — even if the absolute path resolves under
/// `work_root`, it is removed. `CARGO_HOME` / `RUSTUP_HOME` / `HOME`-rooted
/// bins are accepted implicitly because they are absolute and outside
/// `work_root`.
#[allow(dead_code)] // wired by `resolved_runner_program_fail_closed`
fn sanitize_path_for_runner_resolution(path_value: &str, work_root: Option<&Path>) -> String {
    let work_root_canon =
        work_root.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut accepted: Vec<String> = Vec::new();
    for raw in path_value.split(':') {
        if raw.is_empty() {
            continue;
        }
        let p = Path::new(raw);
        if !p.is_absolute() {
            // CB-008: relative components are never trusted — CWD-relative
            // resolution could pick up a hostile `./bin/python3`.
            continue;
        }
        if let Some(ref work_canon) = work_root_canon {
            let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            if canon.starts_with(work_canon) {
                // CB-008: a workspace-local PATH component MUST NOT win the
                // runner search. Drop it.
                continue;
            }
        }
        accepted.push(raw.to_string());
    }
    accepted.join(":")
}

/// CB-008 (Codex iteration-5 high, DR4-001 fail-closed): resolve a runner
/// inside the sanitized PATH. Returns `None` when no absolute resolution
/// is possible (caller MUST then surface a TransportError rather than
/// falling back to the relative runner name).
#[allow(dead_code)] // wired by `VerifierCommand::resolved_runner_program_fail_closed`
fn resolve_runner_in_sanitized_path(
    path_value: &str,
    runner: &str,
    work_root: Option<&Path>,
) -> Option<PathBuf> {
    if runner.is_empty() {
        return None;
    }
    if runner.contains('/') {
        let p = Path::new(runner);
        if !p.is_absolute() {
            return None;
        }
        if let Some(work_root) = work_root {
            let work_canon =
                std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
            let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            if canon.starts_with(&work_canon) {
                return None;
            }
        }
        return Some(p.to_path_buf());
    }
    let sanitized = sanitize_path_for_runner_resolution(path_value, work_root);
    resolve_runner_in_path(&sanitized, runner)
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Issue #661 iteration-5 Task 7.2 / CB-009 (Codex iteration-5 medium):
/// result of post-execution external import detection. Carries the
/// (capped) entries that will be hashed into the event payload alongside
/// the **pre-cap** `total_count` and a `truncated` flag derived from
/// `total_count > EXTERNAL_IMPORT_DETECTED_CAP`. Section 8-2's contract is
/// "上限 8 件に truncate しつつ full count/truncated を伝える"; computing
/// `detected_count` from the capped list (the pre-CB-009 shape) collapsed
/// the true total onto `len() <= cap`, so the caller could not tell
/// "exactly 8" from "many more". `truncated` only flips when the full
/// total strictly exceeds the cap.
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired by post-execution emit site in Phase B
pub(super) struct DetectedExternalImports {
    /// Detected raw path tokens, capped at `EXTERNAL_IMPORT_DETECTED_CAP`.
    pub entries: Vec<String>,
    /// Full pre-cap count (every detection that passed the safe-path /
    /// work_root filters, regardless of how many fit in `entries`).
    pub total_count: usize,
    /// `true` iff `total_count > EXTERNAL_IMPORT_DETECTED_CAP`.
    pub truncated: bool,
}

/// Issue #661 iteration-5 Task 7.2 (post-execution external import
/// detection): pattern-match stdout/stderr for external-workspace imports.
/// Returns the set of detected (raw) module identifiers; caller hashes
/// before emit. The list is capped at `EXTERNAL_IMPORT_DETECTED_CAP=8` per
/// DR4-005 / cardinality bound; the caller is responsible for propagating
/// `detected_count` / `detected_truncated`.
///
/// CB-009 (Codex iteration-5 medium): the function walks every candidate
/// line/token to count the pre-cap total even after the per-emit cap is
/// reached. Only the first `EXTERNAL_IMPORT_DETECTED_CAP` distinct raw
/// strings are retained in `entries`; subsequent unique hits still bump
/// `total_count` so the caller can carry a faithful `detected_count`.
///
/// Safe-path filter: paths under common toolchain cache roots
/// (`~/.cargo`, `~/.pyenv/versions`, `~/.rustup`, `~/.local/share/uv`,
/// pip cache, npm cache, cargo target dirs) are NOT signaled as external.
/// They are infrastructure for the build, not project source.
///
/// Pattern coverage:
/// - Python `ImportError: ... '/external/...'` (lines containing `ImportError`
///   + an absolute path component outside work_root)
/// - Python `ModuleNotFoundError: ... '/external/...'`
/// - Python `from /external/... import ...` style (rare in real logs)
/// - Cargo `error: could not find ... /external/...` (rare; cargo normally
///   resolves through Cargo.toml)
#[allow(dead_code)] // wired by post-execution emit site in Phase B
pub(super) fn detect_external_imports_in_output(
    work_root: &Path,
    stdout: &str,
    stderr: &str,
) -> DetectedExternalImports {
    let work_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let mut entries: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total_count: usize = 0;
    let combined = format!("{stdout}\n{stderr}");
    for line in combined.lines() {
        if !line.contains("ImportError")
            && !line.contains("ModuleNotFoundError")
            && !line.contains("from /")
        {
            continue;
        }
        // Extract candidate absolute paths from the line (very loose tokenizer:
        // split on whitespace, single-quote, double-quote and inspect tokens
        // that look like absolute filesystem paths).
        for raw in line
            .split(|c: char| {
                c.is_whitespace()
                    || c == '\''
                    || c == '"'
                    || c == ','
                    || c == '('
                    || c == ')'
                    || c == '['
                    || c == ']'
            })
            .filter(|s| s.starts_with('/'))
        {
            let candidate = Path::new(raw);
            if !candidate.is_absolute() {
                continue;
            }
            if path_is_known_safe_external(candidate, &work_canon) {
                continue;
            }
            // Resolve symlinks if possible; fall back to the raw path.
            let canon =
                std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
            if canon.starts_with(&work_canon) {
                continue;
            }
            let raw_owned = raw.to_string();
            if !seen.insert(raw_owned.clone()) {
                // Already counted on a previous match — neither bump total
                // nor consider for entries.
                continue;
            }
            total_count += 1;
            if entries.len() < EXTERNAL_IMPORT_DETECTED_CAP {
                entries.push(raw_owned);
            }
        }
    }
    let truncated = total_count > EXTERNAL_IMPORT_DETECTED_CAP;
    DetectedExternalImports {
        entries,
        total_count,
        truncated,
    }
}

pub(super) fn python_package_marker_candidates_for_external_import(
    work_root: &Path,
    stdout: &str,
    stderr: &str,
) -> Vec<String> {
    let work_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let mut candidates = BTreeSet::new();
    let combined = format!("{stdout}\n{stderr}");
    for line in combined.lines() {
        if !line.contains("ImportError") && !line.contains("ModuleNotFoundError") {
            continue;
        }
        for module in quoted_module_tokens(line) {
            let Some(top) = module.split('.').next() else {
                continue;
            };
            if !is_safe_python_module_segment(top) || matches!(top, "test" | "tests") {
                continue;
            }
            let package_dir = work_root.join(top);
            if !package_dir.is_dir() || package_dir.join("__init__.py").exists() {
                continue;
            }
            let package_canon =
                std::fs::canonicalize(&package_dir).unwrap_or_else(|_| package_dir.clone());
            if !package_canon.starts_with(&work_canon) {
                continue;
            }
            let has_direct_python_file = std::fs::read_dir(&package_dir)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(Result::ok))
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "py"));
            if has_direct_python_file {
                candidates.insert(format!("{top}/__init__.py"));
            }
        }
    }
    candidates.into_iter().collect()
}

pub(super) fn python_package_marker_candidates_for_owned_test_imports(
    work_root: &Path,
    owned_test_artifacts: &[String],
) -> Vec<String> {
    let work_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let mut candidates = BTreeSet::new();
    for relative_path in owned_test_artifacts {
        if !safe_relative_workspace_path(relative_path) {
            continue;
        }
        let test_path = work_root.join(relative_path);
        let Ok(test_canon) = std::fs::canonicalize(&test_path) else {
            continue;
        };
        if !test_canon.starts_with(&work_canon) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&test_canon) else {
            continue;
        };
        for module in imported_python_modules(&contents) {
            for candidate in python_package_marker_candidates_for_module(work_root, &module) {
                candidates.insert(candidate);
            }
        }
    }
    candidates.into_iter().collect()
}

fn safe_relative_workspace_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(|ch| ch.is_control())
        && !Path::new(path).is_absolute()
        && !Path::new(path)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn imported_python_modules(contents: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in contents.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("from ") {
            let module = rest.split_whitespace().next().unwrap_or_default();
            if !module.starts_with('.') {
                modules.push(module.trim_end_matches(',').to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("import ") {
            for part in rest.split(',') {
                let module = part.split_whitespace().next().unwrap_or_default();
                if !module.starts_with('.') {
                    modules.push(module.to_string());
                }
            }
        }
    }
    modules
}

fn python_package_marker_candidates_for_module(work_root: &Path, module: &str) -> Vec<String> {
    let work_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let components: Vec<&str> = module
        .split('.')
        .filter(|part| is_safe_python_module_segment(part))
        .collect();
    if components.is_empty() || matches!(components[0], "test" | "tests") {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut prefix = PathBuf::new();
    for component in components {
        prefix.push(component);
        let package_dir = work_root.join(&prefix);
        if !package_dir.is_dir() {
            break;
        }
        if package_dir.join("__init__.py").exists() {
            continue;
        }
        let package_canon =
            std::fs::canonicalize(&package_dir).unwrap_or_else(|_| package_dir.clone());
        if !package_canon.starts_with(&work_canon) {
            break;
        }
        if python_dir_has_source_signal(&package_dir) {
            candidates.push(format!(
                "{}/__init__.py",
                prefix.to_string_lossy().replace('\\', "/")
            ));
        }
    }
    candidates
}

fn python_dir_has_source_signal(package_dir: &Path) -> bool {
    std::fs::read_dir(package_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .any(|entry| {
            let path = entry.path();
            path.extension().is_some_and(|ext| ext == "py") || path.is_dir()
        })
}

fn quoted_module_tokens(line: &str) -> Vec<&str> {
    line.split(['\'', '"'])
        .enumerate()
        .filter_map(|(idx, token)| {
            if idx % 2 == 1 && token.contains('.') {
                Some(token)
            } else {
                None
            }
        })
        .collect()
}

fn is_safe_python_module_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Issue #661 iteration-5 Task 7.2: cap on `detected_modules` carried in
/// the `agent.verifier.external_import_rejected` event payload. Excess
/// detections are dropped and the caller sets `detected_truncated=true`
/// (see Section 8-2). Mirrors `VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP` style.
#[allow(dead_code)] // wired by emit site
pub(super) const EXTERNAL_IMPORT_DETECTED_CAP: usize = 8;

/// Issue #661 iteration-5 Task 7.2 / CB-011 (Codex iteration-5 medium):
/// well-known safe external roots that the pattern matcher must NOT flag
/// as external imports. Mirrors the design Section 6 security table
/// ("cargo registry / pip cache / pyenv versions などは false positive 回避").
///
/// CB-011 narrows the original substring-only filter: a substring like
/// `/lib/python` or any `target` component (at arbitrary depth) was too
/// permissive — `/tmp/other/lib/python/...` and `/tmp/target/...` were
/// hidden from detection. The CB-011 fix anchors safe paths to either:
/// - HOME-anchored toolchain / package caches (`$HOME/.cargo/`,
///   `$HOME/.rustup/`, `$HOME/.pyenv/`, `$HOME/.local/share/uv/`, etc.);
///   the HOME prefix is required so an arbitrary `/tmp/.cargo/...` does
///   NOT pass the filter, and
/// - canonical Python toolchain trees recognised by an explicit prefix
///   match (`/usr/lib/python`, `/usr/local/lib/python`, `/opt/...lib/python`,
///   `/Library/Frameworks/Python.framework/`,
///   system-wide `/lib/python` and `/lib64/python`). These are the only
///   well-known absolute paths that Python sys.path inhabits without
///   project sources and are kept narrow enough that
///   `/tmp/other/lib/python/...` does NOT match.
/// - the canonical CARGO_TARGET_DIR (when set) or `<work_root>/target/`;
///   arbitrary `target` components elsewhere are NO LONGER safe.
fn path_is_known_safe_external(p: &Path, work_root_canon: &Path) -> bool {
    let s = p.to_string_lossy();
    // CB-011 (pass 2): canonicalize the candidate first so symlinked /tmp/.cargo
    // → real cache locations are evaluated against the canonical anchor.
    let p_canon: PathBuf = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());

    // Helper: anchor a candidate to a canonical root using component-boundary
    // path semantics (avoids `/home/u2` matching `/home/u`).
    fn descends_from(candidate: &Path, root_canon: &Path) -> bool {
        candidate.strip_prefix(root_canon).is_ok()
    }

    // CB-011 (pass 2): canonicalize HOME so /tmp-symlinked HOME caches do not
    // bypass the anchor, then require component-boundary descent.
    if let Some(home) = std::env::var_os("HOME") {
        let home_raw = PathBuf::from(home);
        let home_canon = std::fs::canonicalize(&home_raw).unwrap_or(home_raw);
        if descends_from(&p_canon, &home_canon) {
            // After the HOME prefix, the suffix must look like one of the
            // known cache roots, evaluated component-wise (not as a raw
            // string prefix, so `.cargooverride` cannot impersonate `.cargo`).
            const HOME_RELATIVE_SAFE_FIRST_COMPONENTS: &[&[&str]] = &[
                &[".cargo"],
                &[".rustup"],
                &[".pyenv"],
                &[".local", "share", "uv"],
                &[".local", "share", "virtualenvs"],
                &[".cache", "pip"],
                &[".cache", "pypoetry"],
                &[".cache", "uv"],
                &[".npm"],
            ];
            if let Ok(rel) = p_canon.strip_prefix(&home_canon) {
                let rel_components: Vec<_> = rel
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect();
                for safe in HOME_RELATIVE_SAFE_FIRST_COMPONENTS {
                    if rel_components.len() >= safe.len()
                        && rel_components.iter().zip(safe.iter()).all(|(a, b)| a == *b)
                    {
                        return true;
                    }
                }
            }
        }
    }
    // CB-011 (pass 2): also anchor to canonical CARGO_HOME / RUSTUP_HOME if
    // they are set outside HOME (e.g. CI runners with `CARGO_HOME=/cache/.cargo`).
    for env_key in ["CARGO_HOME", "RUSTUP_HOME"] {
        if let Some(raw) = std::env::var_os(env_key) {
            let raw_path = PathBuf::from(raw);
            let canon = std::fs::canonicalize(&raw_path).unwrap_or(raw_path);
            if descends_from(&p_canon, &canon) {
                return true;
            }
        }
    }
    // CB-011: explicit absolute prefixes for system-wide Python toolchain
    // trees. These are bound to known absolute roots — they do NOT match
    // arbitrary `/tmp/.../lib/python` / `/whatever/site-packages` paths.
    const ABSOLUTE_PYTHON_TOOLCHAIN_PREFIXES: &[&str] = &[
        "/usr/lib/python",
        "/usr/lib64/python",
        "/usr/local/lib/python",
        "/usr/local/lib64/python",
        "/opt/homebrew/lib/python",
        "/Library/Frameworks/Python.framework/",
        "/Applications/Xcode.app/Contents/Developer/",
        "/Library/Developer/CommandLineTools/",
        // Wide POSIX system libs (only at /lib and /lib64 root, NOT
        // `/tmp/.../lib/python`).
        "/lib/python",
        "/lib64/python",
    ];
    for prefix in ABSOLUTE_PYTHON_TOOLCHAIN_PREFIXES {
        if s.starts_with(prefix) {
            return true;
        }
    }
    // CB-011: cpython-tagged interpreter installs (e.g. uv-managed
    // `/.../cpython-3.12.1-...`). Restricted to HOME-anchored matches via
    // the HOME branch above; standalone `/tmp/cpython-...` is NOT safe.
    // (Intentional drop of the legacy "/cpython-" anywhere substring.)
    //
    // CB-011: `target` directory containment is anchored to either
    // `<work_root>/target/` or the canonical `CARGO_TARGET_DIR`. Bare
    // `target` components elsewhere are detected as external.
    if let Ok(rel) = p.strip_prefix(work_root_canon)
        && rel
            .components()
            .next()
            .map(|c| matches!(c, std::path::Component::Normal(n) if n == "target"))
            .unwrap_or(false)
    {
        return true;
    }
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir_canon =
            std::fs::canonicalize(&target_dir).unwrap_or(PathBuf::from(target_dir));
        if p.starts_with(&target_dir_canon) {
            return true;
        }
    }
    false
}

/// Issue #661 iteration-4 Task 2.5 (DR1-005 / DR1-012 / DR2-007):
/// pre-spawn `agent.verifier.invoked` event の構造化 snapshot。`turn.rs`
/// facade と `verifier_skill::execute_with_invocation_observer` が
/// `run_structured` 直前にこの snapshot を構築し、emit ownership は
/// `TurnState::last_verifier_invoked_payload_digest` を持つ caller に閉じる
/// (DR1-005 emit ownership facade plumbing)。
///
/// 型レベル invariant:
/// - `runner: RunnerKind` — string 値経路を許さない (DR4-004 security gate)
/// - `bound_artifacts: Vec<PathHashHex>` — raw path がそのまま流れ込むことを
///   コンパイル時にブロック (DR1-012 / DR2-007)
/// - `bound_artifacts_truncated` — `bound_artifacts.len() <= 16` の cap 後の
///   フラグ。`bound_test_artifacts_count` は cap 前の full count を保持する
///   (Section 8-1 / 6 セキュリティ表 payload size explosion 対策)
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired by `turn.rs::run_task_contract_verifier_once` (iteration-4 Task 5.2 producer site) + `verifier_skill::execute_with_invocation_observer` (Task 5.3)
pub(super) struct VerifierInvokedSnapshot {
    pub runner: RunnerKind,
    pub bound_artifacts: Vec<PathHashHex>,
    pub bound_test_artifacts_count: usize,
    pub bound_artifacts_truncated: bool,
    pub env_summary: HermeticEnvSummary,
}

/// Issue #661 iteration-4: maximum number of `bound_artifacts` entries the
/// `agent.verifier.invoked` payload carries. Excess entries are truncated and
/// `bound_artifacts_truncated=true` is set so log consumers can detect the
/// cap. `bound_test_artifacts_count` keeps the full pre-cap count
/// (Section 8-1 / セキュリティ表 payload size explosion 対策)。
#[allow(dead_code)] // wired by `VerifierInvokedSnapshot::from_command_and_env` (iteration-4 Task 5.2 producer site)
pub(super) const VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP: usize = 16;

impl VerifierInvokedSnapshot {
    /// Construct the snapshot from a `VerifierCommand` and a built
    /// `HermeticEnvPlan`. Only callable for runners with a `RunnerKind`
    /// variant — production `OwnedTestVerifierPlan::Runnable` yields
    /// `cargo` / `python3` / `npm` today, so this returns `None` for any future or
    /// unsupported runner (DR4-004 security gate: unknown runner string
    /// cannot reach the event payload).
    ///
    /// `bound_artifacts` are hashed via `PathHashHex::from_relative_str`
    /// (which goes through `crate::logging::stable_path_hash` SSOT after
    /// `mask_secrets`) and capped at `VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP`.
    /// The cap-front truncation deliberately preserves the order callers
    /// provided so the path_hash sequence is reproducible across re-emits.
    #[allow(dead_code)] // wired by `turn.rs::run_task_contract_verifier_once` (iteration-4 Task 5.2 producer site)
    pub(super) fn from_command_and_env(
        command: &VerifierCommand,
        env_plan: &HermeticEnvPlan,
    ) -> Option<Self> {
        let runner = RunnerKind::from_runner_str(command.runner())?;
        let bound_paths = command.bound_test_artifacts();
        let bound_test_artifacts_count = bound_paths.len();
        let bound_artifacts_truncated =
            bound_test_artifacts_count > VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP;
        let bound_artifacts: Vec<PathHashHex> = bound_paths
            .iter()
            .take(VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP)
            .map(|p| PathHashHex::from_relative_str(p))
            .collect();
        Some(Self {
            runner,
            bound_artifacts,
            bound_test_artifacts_count,
            bound_artifacts_truncated,
            env_summary: env_plan.summary(),
        })
    }
}

/// Pure builder for the `agent.verifier.invoked` event payload.
///
/// `cwd_inside_work_root` is intentionally root-level even though
/// `HermeticEnvSummary` carries it. `env_summary` only carries
/// `allowlist_keys` and `pythonpath_root`.
pub(super) fn build_agent_verifier_invoked_payload(
    session_id: &str,
    turn_index: usize,
    iteration_seq: usize,
    snapshot: &VerifierInvokedSnapshot,
) -> serde_json::Value {
    let bound_artifacts: Vec<serde_json::Value> = snapshot
        .bound_artifacts
        .iter()
        .map(|h| serde_json::json!({ "path_hash": h.as_str() }))
        .collect();
    let env_summary = serde_json::json!({
        "allowlist_keys": snapshot.env_summary.allowlist_keys,
        "pythonpath_root": snapshot.env_summary.pythonpath_root,
    });
    serde_json::json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "iteration_seq": iteration_seq,
        "runner": snapshot.runner.as_str(),
        "bound_artifacts": bound_artifacts,
        "bound_test_artifacts_count": snapshot.bound_test_artifacts_count,
        "bound_artifacts_truncated": snapshot.bound_artifacts_truncated,
        "env_summary": env_summary,
        "cwd_inside_work_root": snapshot.env_summary.cwd_inside_work_root,
    })
}

/// Pure builder for the `agent.verifier.external_import_rejected` event
/// payload. The caller passes only pre-hashed strings; raw module names,
/// filesystem paths, and executable paths must not enter this payload.
pub(super) fn build_agent_verifier_external_import_rejected_payload(
    session_id: &str,
    turn_index: usize,
    runner: &str,
    reason: &str,
    detected_hashes: &[(&str, &'static str)],
    detected_count: usize,
    detected_truncated: bool,
) -> serde_json::Value {
    let detected_modules: Vec<serde_json::Value> = detected_hashes
        .iter()
        .take(EXTERNAL_IMPORT_DETECTED_CAP)
        .map(|(hash, source_kind)| {
            serde_json::json!({
                "module_hash": *hash,
                "path_hash": *hash,
                "source_kind": *source_kind,
            })
        })
        .collect();
    serde_json::json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "runner": runner,
        "reason": reason,
        "detected_count": detected_count,
        "detected_truncated": detected_truncated,
        "detected_modules": detected_modules,
    })
}

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
    "cargo", "python3", "python", "pytest", "uv", "poetry", "hatch", "node", "npm", "pnpm", "yarn",
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
        // Issue #661 iteration-5 Task 6.4 (DR4-002 leading-dash argv injection
        // 防止): bound test artifact paths that begin with `-` would be parsed
        // as command-line options by pytest / cargo / npm etc. Reject the
        // construction so the caller can fall back to `Weak` rather than
        // silently rewriting argv with a `--` separator (which could still
        // misfire if the runner doesn't honor `--`).
        for path in &bound_test_artifacts {
            if path.starts_with('-') {
                return None;
            }
        }
        Some(Self {
            runner: runner.to_string(),
            args,
            bound_test_artifacts,
        })
    }

    /// Issue #661 iteration-5 Task 6.4 (DR4-001 PATH hijack 防止): resolve
    /// the runner binary to an absolute path under the planned PATH and
    /// return it. Returns the relative `runner` name unchanged when the
    /// lookup fails (best-effort fallback per Task 6.4: "which が利用できない
    /// 場合は relative argv 維持") so production CI without `cargo`/`python3`
    /// in PATH does not crash spawn — child process error reporting takes
    /// over.
    ///
    /// Caller MUST pass the env plan's effective PATH (post-`filter_env_for_tester`)
    /// so the resolution honors the hermetic boundary. raw executable path
    /// is not exposed in any event payload (Section 6 / DR4-005).
    #[allow(dead_code)]
    pub(super) fn resolved_runner_program(&self, planned_path_value: Option<&str>) -> String {
        if let Some(path_value) = planned_path_value
            && let Some(abs) = resolve_runner_in_path(path_value, &self.runner)
        {
            return abs.to_string_lossy().into_owned();
        }
        self.runner.clone()
    }

    /// CB-008 (Codex iteration-5 high, DR4-001 fail-closed): resolve the
    /// runner via the **sanitized** PATH (no relative / empty / work_root-
    /// anchored components survive). Returns `None` when no absolute
    /// resolution is possible — callers MUST then surface a TransportError
    /// rather than letting the child process inherit the bare runner name
    /// and re-resolve through whatever PATH the parent shell carried.
    ///
    /// The legacy best-effort `resolved_runner_program` is retained for
    /// callers that intentionally accept a degraded resolution (spawning
    /// with the bare runner name) — `run_structured` instead uses the
    /// fail-closed variant.
    #[allow(dead_code)]
    pub(super) fn resolved_runner_program_fail_closed(
        &self,
        planned_path_value: Option<&str>,
        work_root: Option<&Path>,
    ) -> Option<String> {
        let path_value = planned_path_value?;
        let abs = resolve_runner_in_sanitized_path(path_value, &self.runner, work_root)?;
        Some(abs.to_string_lossy().into_owned())
    }

    /// Full `cargo test` structured constructor.
    ///
    /// Issue #865: Rust task final success must be the full suite, not a
    /// partial `cargo test --test <name>` run. The owned test artifacts are
    /// still retained in `bound_test_artifacts` so planning/execution can
    /// prove that task-owned tests exist and are in scope before spawning the
    /// verifier, but they are intentionally not forwarded as cargo filters.
    ///
    /// CB-001 defense in depth: empty `owned_test_artifacts` is also a
    /// hard reject. An unbound cargo verifier could otherwise execute
    /// the entire test suite without satisfying the design contract
    /// that the current task owns at least one test artifact.
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
        let mut args = Vec::with_capacity(args_prefix.len() + 1);
        args.push("test".to_string());
        args.extend(args_prefix);
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

    /// `python3 -m pytest -q -p no:cacheprovider` structured constructor for
    /// the stdlib-Python toolchain detected by `python_pytest_command`.
    /// Toolchain-specific runners (`uv run` etc.) still go through Weak in
    /// Phase 2.2 because parsing their shell shape is out of scope.
    ///
    /// Issue #651 CB-001 defense in depth: empty `owned_test_artifacts`
    /// is rejected. The verifier intentionally runs the full pytest suite,
    /// while retaining owned artifacts as binding metadata. This matches the
    /// Cargo/npm contract: a task-owned test must exist and be in scope, but
    /// existing regression tests are still part of completion evidence.
    #[allow(dead_code)]
    pub(super) fn from_python3_pytest_stdlib(owned_test_artifacts: &[String]) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        let args = vec![
            "-m".to_string(),
            "pytest".to_string(),
            "-q".to_string(),
            "-p".to_string(),
            "no:cacheprovider".to_string(),
        ];
        Self::new_allowlisted("python3", args, owned_test_artifacts.to_vec())
    }

    /// `python3 -m unittest discover -s tests` structured constructor for
    /// explicit stdlib-unittest requests. Like the pytest stdlib constructor,
    /// it runs the discovered suite while retaining owned test artifacts as
    /// binding metadata.
    #[allow(dead_code)]
    pub(super) fn from_python3_unittest_discover(owned_test_artifacts: &[String]) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        let args = vec![
            "-m".to_string(),
            "unittest".to_string(),
            "discover".to_string(),
            "-s".to_string(),
            "tests".to_string(),
        ];
        Self::new_allowlisted("python3", args, owned_test_artifacts.to_vec())
    }

    /// Full `npm test` structured constructor.
    ///
    /// Issue #865: package-script verification should accept `npm test` as
    /// completion evidence. Owned test artifacts are validated before spawn
    /// but are not passed as argv filters because package scripts own their
    /// test discovery contract.
    #[allow(dead_code)]
    pub(super) fn from_npm_test(owned_test_artifacts: &[String]) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        Self::new_allowlisted(
            "npm",
            vec!["test".to_string()],
            owned_test_artifacts.to_vec(),
        )
    }

    /// `node --test <owned_test_artifacts>` structured constructor.
    ///
    /// Node's built-in test runner accepts file paths as positional args after
    /// `--test`, so this preserves the bound-test invariant without parsing or
    /// trusting package.json script bodies. TypeScript/Jest/Vitest shapes return
    /// `None` and remain Weak until a dedicated structured adapter exists.
    #[allow(dead_code)]
    pub(super) fn from_node_test(owned_test_artifacts: &[String]) -> Option<Self> {
        if owned_test_artifacts.is_empty() {
            return None;
        }
        if owned_test_artifacts
            .iter()
            .any(|path| !node_test_artifact_path_is_directly_runnable(path))
        {
            return None;
        }
        let mut args = Vec::with_capacity(1 + owned_test_artifacts.len());
        args.push("--test".to_string());
        args.extend(owned_test_artifacts.iter().cloned());
        Self::new_allowlisted("node", args, owned_test_artifacts.to_vec())
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

fn node_test_artifact_path_is_directly_runnable(path: &str) -> bool {
    let p = Path::new(path);
    if !crate::util::file_classify::is_test_file(p) {
        return false;
    }
    matches!(
        p.extension().and_then(|ext| ext.to_str()),
        Some("js" | "mjs" | "cjs")
    )
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
    pub(super) fn plan_from_evidence_command_hint(command: &str) -> Option<AutoTestPlan> {
        let command = command.trim();
        if command.is_empty() || command.len() > 300 {
            return None;
        }
        if !is_completion_verifier_command(command) {
            return None;
        }
        if !super::verifier_command_policy::is_evidence_command_hint_allowed(command) {
            return None;
        }
        Some(AutoTestPlan {
            command: command.to_string(),
            reason: "controller evidence command".to_string(),
        })
    }

    pub(super) fn detect(work_root: &Path, changed_files: &[String]) -> Option<AutoTestPlan> {
        Self::detect_candidate(work_root, changed_files).map(VerifierCandidate::into_plan)
    }

    pub(super) fn detect_with_project_unit(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
        project_unit: Option<&ProjectUnit>,
    ) -> Option<AutoTestPlan> {
        let project_unit = project_unit?;
        Self::detect_candidate_with_project_unit(
            work_root,
            changed_files,
            recent_successful_bash_commands,
            Some(project_unit),
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

    pub(super) fn detect_candidate_with_project_unit(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
        project_unit: Option<&ProjectUnit>,
    ) -> Option<VerifierCandidate> {
        let candidates = verifier_candidates_for_selection(
            work_root,
            changed_files,
            recent_successful_bash_commands,
            &[],
            project_unit,
        );
        let selected = select_verifier_candidate(candidates.clone());
        emit_verifier_candidate_telemetry(&candidates, selected.as_ref());
        selected
    }

    fn detect_candidate_with_project_unit_and_owned_test_artifacts(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
        owned_test_artifacts: &[String],
        project_unit: Option<&ProjectUnit>,
    ) -> Option<VerifierCandidate> {
        let candidates = verifier_candidates_for_selection(
            work_root,
            changed_files,
            recent_successful_bash_commands,
            owned_test_artifacts,
            project_unit,
        );
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
    ///   `PythonTests` stdlib → `python3 -m pytest -q`).
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
        Self::detect_with_owned_test_artifacts_and_project_unit(
            work_root,
            changed_files,
            recent_successful_bash_commands,
            owned_test_artifacts,
            None,
        )
    }

    pub(super) fn detect_with_owned_test_artifacts_and_project_unit(
        work_root: &Path,
        changed_files: &[String],
        recent_successful_bash_commands: &[String],
        owned_test_artifacts: &[String],
        project_unit: Option<&ProjectUnit>,
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
        let changed_files_with_owned_tests =
            changed_files_with_owned_test_artifacts(changed_files, owned_test_artifacts);
        let Some(candidate) = Self::detect_candidate_with_project_unit_and_owned_test_artifacts(
            work_root,
            &changed_files_with_owned_tests,
            recent_successful_bash_commands,
            owned_test_artifacts,
            project_unit,
        ) else {
            return OwnedTestVerifierPlan::Missing;
        };
        let display_command = candidate.plan.command.clone();
        let source = candidate.source;
        let plan = candidate.into_plan();
        match source {
            VerifierCandidateSource::CargoManifest => {
                if let Some(command) =
                    VerifierCommand::from_cargo_test(Vec::new(), owned_test_artifacts)
                {
                    return OwnedTestVerifierPlan::Runnable { plan, command };
                }
                // `from_cargo_test` returns `None` only when the allowlist
                // rejects the command shape. Rust final success uses full
                // `cargo test`, so owned artifacts are validated as binding
                // metadata rather than converted to partial test filters.
                OwnedTestVerifierPlan::Weak {
                    reason: "cargo runner cannot bind owned test artifacts",
                    detected_source: source.as_str(),
                    display_command: Some(display_command),
                }
            }
            VerifierCandidateSource::PythonTests => {
                // For task-contract verification, binding the current task's
                // owned test artifact is stronger than replaying a detected
                // shell-shaped setup command. Even when pyproject.toml would
                // make the generic detector prefer a `pip install && pytest`
                // string, run the allowlisted stdlib pytest constructor with
                // explicit owned test paths. Missing dependencies then surface
                // as normal verifier failures instead of collapsing the task
                // into VerifierWeak.
                let verifier_flavor =
                    super::verifier_command_policy::python_project_unit_verifier_flavor(Some(
                        &display_command,
                    ));
                let authoring_style_decision =
                    super::authoring_style::decide_python_authoring_style(
                        super::authoring_style::PythonAuthoringStyleSignals::default(),
                        verifier_flavor,
                    );
                debug_assert_eq!(
                    authoring_style_decision.style,
                    super::authoring_style::AuthoringStyle::Unspecified
                );
                let command = match verifier_flavor {
                    PythonProjectUnitVerifierFlavor::UnittestDiscover => {
                        VerifierCommand::from_python3_unittest_discover(owned_test_artifacts)
                    }
                    PythonProjectUnitVerifierFlavor::PytestStdlib => {
                        VerifierCommand::from_python3_pytest_stdlib(owned_test_artifacts)
                    }
                };
                if let Some(command) = command {
                    return OwnedTestVerifierPlan::Runnable { plan, command };
                }
                OwnedTestVerifierPlan::Weak {
                    reason: "python toolchain not structurally bindable",
                    detected_source: source.as_str(),
                    display_command: Some(display_command),
                }
            }
            VerifierCandidateSource::PackageJsonScripts => {
                if let Some(command) = VerifierCommand::from_npm_test(owned_test_artifacts) {
                    return OwnedTestVerifierPlan::Runnable { plan, command };
                }
                OwnedTestVerifierPlan::Weak {
                    reason: "package.json test script cannot be structurally bound to owned test artifacts",
                    detected_source: source.as_str(),
                    display_command: Some(display_command),
                }
            }
            VerifierCandidateSource::ProjectInstruction
            | VerifierCandidateSource::RecentSuccessfulBash
            | VerifierCandidateSource::NativeNodeFramework
            | VerifierCandidateSource::PythonCompileFallback => OwnedTestVerifierPlan::Weak {
                reason: "verifier source has no allowlisted structured constructor",
                detected_source: source.as_str(),
                display_command: Some(display_command),
            },
        }
    }

    pub(super) fn run(work_root: &Path, plan: &AutoTestPlan) -> Result<AutoTestResult, String> {
        Self::run_with_timeout(
            work_root,
            plan,
            Duration::from_secs(AUTO_TEST_RUN_TIMEOUT_SECS),
        )
    }

    fn run_with_timeout(
        work_root: &Path,
        plan: &AutoTestPlan,
        timeout: Duration,
    ) -> Result<AutoTestResult, String> {
        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg(&plan.command)
            .current_dir(work_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::tools::bash::apply_unix_pgroup(&mut command);
        let output = wait_with_auto_test_timeout(&mut command, timeout)?;
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
        task_kind: super::task_contract::TaskKind,
    ) -> Result<AutoTestResult, String> {
        // Issue #918 (P1): fail-closed process-spawn gate. Only the Coding
        // capability may spawn a structured-verifier child process. This is a
        // defense-in-depth backstop — the §5.1 invariant (the verification gate
        // in CompletionPolicy::from_contract_parts) already prevents a non-coding
        // task from ever requiring test execution and reaching here. The guard is
        // PROFILE-SYMMETRIC (a real `Err` in both debug and release, no
        // `debug_assert!`) so it is observable in the repo's debug `cargo test` CI
        // and a §5.1 regression fails closed rather than panicking / spawning.
        if !super::verifier::capability_for(task_kind).allows_process_exec() {
            tracing::error!(
                target: "agent.verifier",
                ?task_kind,
                "non-coding verifier process spawn blocked; failing closed"
            );
            return Err(format!(
                "structured verifier process spawn is not permitted for task kind {task_kind:?} \
                 (only Coding may spawn a verifier process)"
            ));
        }
        validate_bound_test_artifacts_for_execution(work_root, scope, command)?;

        // Issue #661 iteration-5 Task 6.1 / 6.2 / 6.4 + CB-008 fail-closed:
        // build hermetic env plan with runner-specific extras (Python
        // adapter gets VERIFIER_ENV_PYTHON_EXTRA, others &[]), then
        // resolve runner argv[0] inside the **sanitized** planned PATH so
        // (a) relative / empty PATH components cannot inject a workspace-
        //     local hijack via cwd resolution, and
        // (b) any absolute component under work_root is removed before the
        //     search runs.
        // CB-008 hardens the resolver to fail-closed: if no absolute
        // resolution survives the sanitization filter, return an explicit
        // `Err(...)` rather than letting the child
        // process inherit the bare runner name and re-resolve through
        // the parent process PATH (which the previous best-effort
        // fallback exposed). The caller classifies this Err as either a
        // verifier timeout failure or a transport error.
        let extras: &[(&'static str, &'static str)] = match command.runner() {
            "python3" => VERIFIER_ENV_PYTHON_EXTRA,
            _ => &[],
        };
        let env_plan = build_hermetic_env_plan(work_root, extras);
        let runner_program = command
            .resolved_runner_program_fail_closed(env_plan.planned_path(), Some(work_root))
            .ok_or_else(|| {
                format!(
                    "verifier runner {runner:?} could not be resolved against the sanitized PATH \
                     (work_root-local / relative / empty PATH components are rejected by CB-008)",
                    runner = command.runner()
                )
            })?;

        if let Some(preflight_result) = run_structured_generated_test_preflight(
            &runner_program,
            &env_plan,
            work_root,
            command,
            display_command,
        )? {
            return Ok(preflight_result);
        }

        let dependency_site =
            match structured_python_pytest_dependency_setup_packages(work_root, command) {
                Some(packages) => {
                    let dependency_site = structured_python_dependency_site_dir(work_root);
                    if let Some(result) = run_structured_python_dependency_setup(
                        &runner_program,
                        &env_plan,
                        &dependency_site,
                        &packages,
                        display_command,
                    )? {
                        return Ok(result);
                    }
                    Some(dependency_site)
                }
                None => None,
            };

        if structured_node_dependency_setup_required(work_root, command)
            && let Some(result) =
                run_structured_node_dependency_setup(&runner_program, &env_plan, display_command)?
        {
            return Ok(result);
        }

        let mut child_cmd = Command::new(runner_program);
        child_cmd
            .args(command.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Apply hermetic env after args / stdio so caller's `current_dir`
        // and env overrides land in a single SSOT (env_clear + allowlist
        // re-inject + extras + PYTHONPATH + cwd).
        env_plan.apply_to(&mut child_cmd);
        if let Some(dependency_site) = dependency_site.as_deref() {
            apply_structured_python_dependency_site_pythonpath(
                &mut child_cmd,
                work_root,
                dependency_site,
            )?;
        }
        // PR-003: put the verifier in its own process group on Unix so
        // `wait_with_auto_test_timeout` can SIGKILL the whole descendant
        // tree on timeout. On non-Unix this is a no-op. SSOT lives in
        // `crate::tools::bash::apply_unix_pgroup` (Issue #461).
        crate::tools::bash::apply_unix_pgroup(&mut child_cmd);
        let output = wait_with_auto_test_timeout(
            &mut child_cmd,
            Duration::from_secs(AUTO_TEST_RUN_TIMEOUT_SECS),
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

fn run_structured_generated_test_preflight(
    runner_program: &str,
    env_plan: &HermeticEnvPlan,
    work_root: &Path,
    command: &VerifierCommand,
    display_command: &str,
) -> Result<Option<AutoTestResult>, String> {
    match command.runner() {
        "python3" | "python" => {
            let python_tests = command
                .bound_test_artifacts()
                .iter()
                .filter(|path| Path::new(path).extension().is_some_and(|ext| ext == "py"))
                .cloned()
                .collect::<Vec<_>>();
            if python_tests.is_empty() {
                return Ok(None);
            }
            let mut args = vec!["-B".to_string(), "-m".to_string(), "py_compile".to_string()];
            args.extend(python_tests);
            run_structured_preflight_command(
                runner_program,
                env_plan,
                args,
                &format!("python3 -B -m py_compile + {display_command}"),
            )
        }
        "cargo" => {
            let mut args = vec!["test".to_string(), "--no-run".to_string()];
            for path in command.bound_test_artifacts() {
                let Some(name) = cargo_integration_test_name_for_manifest(work_root, path)
                    .or_else(|| cargo_integration_test_name(path))
                else {
                    return Ok(None);
                };
                args.push("--test".to_string());
                args.push(name);
            }
            if args.len() == 2 {
                return Ok(None);
            }
            run_structured_preflight_command(
                runner_program,
                env_plan,
                args,
                &format!("cargo test --no-run + {display_command}"),
            )
        }
        _ => Ok(None),
    }
}

fn run_structured_preflight_command(
    runner_program: &str,
    env_plan: &HermeticEnvPlan,
    args: Vec<String>,
    display_command: &str,
) -> Result<Option<AutoTestResult>, String> {
    let mut preflight_cmd = Command::new(runner_program);
    preflight_cmd
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env_plan.apply_to(&mut preflight_cmd);
    crate::tools::bash::apply_unix_pgroup(&mut preflight_cmd);
    let output = wait_with_auto_test_timeout(
        &mut preflight_cmd,
        Duration::from_secs(AUTO_TEST_RUN_TIMEOUT_SECS),
    )
    .map_err(|err| format!("generated test preflight {err}"))?;
    if output.status.success() {
        return Ok(None);
    }

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
    let redacted = crate::session::feedback::redact_verifier_command_for_storage(display_command);
    Ok(Some(AutoTestResult {
        command: redacted,
        passed: false,
        output: truncate(&formatted, MAX_OUTPUT_BYTES),
        exit_code: output.status.code(),
        stdout,
        stderr,
    }))
}

fn structured_python_pytest_dependency_setup_packages(
    work_root: &Path,
    command: &VerifierCommand,
) -> Option<Vec<String>> {
    if command.runner() != "python3" || !command_invokes_python_pytest_module(command) {
        return None;
    }
    let pyproject_path = work_root.join("pyproject.toml");
    let packages = if pyproject_path.is_file() {
        python_pyproject_test_packages(&PythonProjectEvidence::from_file(work_root))
    } else {
        python_inferred_test_packages_from_sources(work_root)
    };
    if packages.is_empty() {
        None
    } else {
        Some(packages)
    }
}

fn command_invokes_python_pytest_module(command: &VerifierCommand) -> bool {
    command
        .args()
        .windows(2)
        .any(|window| window[0] == "-m" && window[1] == "pytest")
}

fn structured_python_dependency_site_dir(work_root: &Path) -> PathBuf {
    work_root
        .join(".anvil-state")
        .join("verifier-python")
        .join("site")
}

fn structured_node_dependency_setup_required(work_root: &Path, command: &VerifierCommand) -> bool {
    if command.runner() != "npm" || !command.args().iter().any(|arg| arg == "test") {
        return false;
    }
    if work_root.join("node_modules").is_dir() {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(work_root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    package_json_declares_dependency_table(&value)
}

fn package_json_declares_dependency_table(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .iter()
    .any(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|deps| !deps.is_empty())
    })
}

fn run_structured_node_dependency_setup(
    runner_program: &str,
    env_plan: &HermeticEnvPlan,
    display_command: &str,
) -> Result<Option<AutoTestResult>, String> {
    let mut setup_cmd = Command::new(runner_program);
    setup_cmd
        .args(["install", "--ignore-scripts"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env_plan.apply_to(&mut setup_cmd);
    crate::tools::bash::apply_unix_pgroup(&mut setup_cmd);

    let output = wait_with_auto_test_timeout(
        &mut setup_cmd,
        Duration::from_secs(AUTO_TEST_DEPENDENCY_SETUP_TIMEOUT_SECS),
    )
    .map_err(|err| format!("structured Node dependency setup {err}"))?;
    if output.status.success() {
        return Ok(None);
    }

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
    let setup_display = format!("npm install --ignore-scripts + {display_command}");
    let redacted = crate::session::feedback::redact_verifier_command_for_storage(&setup_display);
    Ok(Some(AutoTestResult {
        command: redacted,
        passed: false,
        output: truncate(&formatted, MAX_OUTPUT_BYTES),
        exit_code: output.status.code(),
        stdout,
        stderr,
    }))
}

fn run_structured_python_dependency_setup(
    runner_program: &str,
    env_plan: &HermeticEnvPlan,
    dependency_site: &Path,
    packages: &[String],
    display_command: &str,
) -> Result<Option<AutoTestResult>, String> {
    std::fs::create_dir_all(dependency_site).map_err(|err| {
        format!(
            "failed to create structured Python verifier dependency directory {}: {err}",
            dependency_site.display()
        )
    })?;

    let mut setup_cmd = Command::new(runner_program);
    setup_cmd
        .args([
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "--upgrade",
            "--target",
        ])
        .arg(dependency_site)
        .args(packages)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env_plan.apply_to(&mut setup_cmd);
    crate::tools::bash::apply_unix_pgroup(&mut setup_cmd);

    let output = wait_with_auto_test_timeout(
        &mut setup_cmd,
        Duration::from_secs(AUTO_TEST_DEPENDENCY_SETUP_TIMEOUT_SECS),
    )
    .map_err(|err| format!("structured Python dependency setup {err}"))?;
    if output.status.success() {
        return Ok(None);
    }

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
    let setup_display = format!(
        "python3 -m pip install --target <workspace>/.anvil-state/verifier-python/site {} + {}",
        packages.join(" "),
        display_command
    );
    let redacted = crate::session::feedback::redact_verifier_command_for_storage(&setup_display);
    Ok(Some(AutoTestResult {
        command: redacted,
        passed: false,
        output: truncate(&formatted, MAX_OUTPUT_BYTES),
        exit_code: output.status.code(),
        stdout,
        stderr,
    }))
}

fn apply_structured_python_dependency_site_pythonpath(
    command: &mut Command,
    work_root: &Path,
    dependency_site: &Path,
) -> Result<(), String> {
    let pythonpath = structured_python_dependency_site_pythonpath(work_root, dependency_site)?;
    command.env("PYTHONPATH", pythonpath);
    Ok(())
}

fn structured_python_dependency_site_pythonpath(
    work_root: &Path,
    dependency_site: &Path,
) -> Result<OsString, String> {
    std::env::join_paths([work_root, dependency_site]).map_err(|err| {
        format!(
            "failed to build structured Python verifier PYTHONPATH for {}: {err}",
            dependency_site.display()
        )
    })
}

/// Upper bound for one verifier evidence command.
///
/// Local-first repair needs verifier hangs to become typed failure evidence
/// quickly enough for the next repair turn. Dependency installation keeps a
/// separate, longer bound below because network / cache setup is a different
/// evidence phase from running generated or project tests.
const AUTO_TEST_RUN_TIMEOUT_SECS: u64 = 60;

/// Upper bound for structured dependency setup before the actual verifier run.
const AUTO_TEST_DEPENDENCY_SETUP_TIMEOUT_SECS: u64 = 300;

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
/// - paths the `classify_ownership` SSOT does not admit (Issue #661
///   iteration-3 Task 3.4 path #3 / 判断 #1 / DR2-003): switched from
///   `TaskWorkspaceScope::contains` to `classify_ownership` so the
///   verifier-path SSOT propagates `NestedTestAdmission::enabled()`
///   into the same security gate used by `record_repo_edit_event` /
///   `record_verifier_observation`. Workspace-relative / symlink
///   containment / ignored_top_dir checks remain authoritative and
///   the explicit manual checks above are kept as defense in depth
///   (their error messages stay stable for downstream observability)
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
        // Issue #661 (iteration-3 Task 3.4 path #3 / 判断 #1 / DR2-003):
        // route through `classify_ownership` with
        // `NestedTestAdmission::enabled()` so the verifier-path SSOT
        // is one of the 4 propagation sites (record_repo_edit_event /
        // record_verifier_observation / this function /
        // seed_artifact_ledger_verifier_observation).
        let ownership = super::artifact_ownership::classify_ownership(
            super::artifact_ownership::OwnershipInputs {
                work_root,
                relative_path: path,
                scope,
                edited_this_session: false,
                scaffold_changed: false,
                verifier_passed_in_scope: false,
                nested_test_admission: super::artifact_ownership::NestedTestAdmission::enabled(),
            },
        );
        if matches!(
            ownership,
            super::artifact_ownership::ArtifactOwnership::OutOfScope
        ) {
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
    let visible_changed_files = visible_changed_files(changed_files);
    let changed_files = visible_changed_files.as_slice();
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

fn verifier_candidates_from_project_unit(project_unit: &ProjectUnit) -> Vec<VerifierCandidate> {
    project_unit
        .verifier_candidates
        .iter()
        .filter_map(|candidate| {
            let source = VerifierCandidateSource::from_project_unit_source(candidate.source)?;
            Some(VerifierCandidate {
                plan: AutoTestPlan {
                    command: candidate.command_preview.clone(),
                    reason: format!(
                        "ProjectUnit verifier candidate selected from {}",
                        candidate.source
                    ),
                },
                source,
                confidence: 0.9,
                evidence: vec![
                    format!("project-unit-root:{}", project_unit.root),
                    format!("project-unit-source:{}", candidate.source),
                    format!("project-unit-timeout:{}", candidate.timeout_class.as_str()),
                ],
            })
        })
        .collect()
}

fn verifier_candidates_for_selection(
    work_root: &Path,
    changed_files: &[String],
    recent_successful_bash_commands: &[String],
    owned_test_artifacts: &[String],
    project_unit: Option<&ProjectUnit>,
) -> Vec<VerifierCandidate> {
    let Some(project_unit) = project_unit else {
        return detect_verifier_candidates(
            work_root,
            changed_files,
            recent_successful_bash_commands,
        );
    };
    let candidates = verifier_candidates_from_project_unit(project_unit);
    if !candidates.is_empty() || !project_unit_allows_owned_test_fallback(project_unit) {
        return candidates;
    }
    let owned_changed_files =
        changed_files_with_owned_test_artifacts(changed_files, owned_test_artifacts);
    detect_verifier_candidates(
        work_root,
        &owned_changed_files,
        recent_successful_bash_commands,
    )
    .into_iter()
    .filter(|candidate| {
        project_unit_candidate_matches_observed_stack(project_unit, candidate.source)
            && candidate_source_matches_owned_test_artifacts(candidate.source, owned_test_artifacts)
    })
    .collect()
}

fn project_unit_allows_owned_test_fallback(project_unit: &ProjectUnit) -> bool {
    project_unit
        .artifact_roles
        .contains(&super::task_contract::ArtifactRole::Test)
}

fn project_unit_candidate_matches_observed_stack(
    project_unit: &ProjectUnit,
    source: VerifierCandidateSource,
) -> bool {
    match source {
        VerifierCandidateSource::PythonTests | VerifierCandidateSource::PythonCompileFallback => {
            project_unit.observed_stacks.contains(&"python")
        }
        VerifierCandidateSource::CargoManifest => project_unit.observed_stacks.contains(&"rust"),
        VerifierCandidateSource::PackageJsonScripts
        | VerifierCandidateSource::NativeNodeFramework => {
            project_unit.observed_stacks.contains(&"node")
                || project_unit.observed_stacks.contains(&"typescript")
        }
        VerifierCandidateSource::ProjectInstruction
        | VerifierCandidateSource::RecentSuccessfulBash => false,
    }
}

fn candidate_source_matches_owned_test_artifacts(
    source: VerifierCandidateSource,
    owned_test_artifacts: &[String],
) -> bool {
    match source {
        VerifierCandidateSource::PythonTests => owned_test_artifacts
            .iter()
            .any(|path| path.ends_with(".py")),
        VerifierCandidateSource::CargoManifest => owned_test_artifacts
            .iter()
            .any(|path| path.ends_with(".rs")),
        VerifierCandidateSource::PackageJsonScripts
        | VerifierCandidateSource::NativeNodeFramework => owned_test_artifacts.iter().any(|path| {
            matches!(
                Path::new(path).extension().and_then(|ext| ext.to_str()),
                Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs")
            )
        }),
        VerifierCandidateSource::ProjectInstruction
        | VerifierCandidateSource::RecentSuccessfulBash
        | VerifierCandidateSource::PythonCompileFallback => false,
    }
}

fn changed_files_with_owned_test_artifacts(
    changed_files: &[String],
    owned_test_artifacts: &[String],
) -> Vec<String> {
    let mut out = changed_files.to_vec();
    out.extend(owned_test_artifacts.iter().cloned());
    out.sort();
    out.dedup();
    out
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
        let summary = super::package_manifest_summary::parse_package_manifest_summary(raw).ok()?;
        Some(Self {
            scripts: summary.scripts,
            packages: summary.packages,
        })
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CargoTestTarget {
    name: String,
    path: String,
}

impl CargoManifestEvidence {
    fn from_str(raw: &str) -> Self {
        Self {
            has_explicit_test_target: cargo_test_targets_from_manifest(raw).has_explicit_section,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CargoTestTargets {
    has_explicit_section: bool,
    targets: Vec<CargoTestTarget>,
}

fn cargo_test_targets_from_manifest(raw: &str) -> CargoTestTargets {
    let mut parsed = CargoTestTargets::default();
    let mut in_test_section = false;
    let mut current_name: Option<String> = None;
    let mut current_path: Option<String> = None;

    for line in raw.lines().map(str::trim_start) {
        if line.starts_with('#') {
            continue;
        }
        if let Some(section) = parse_toml_array_section(line) {
            push_cargo_test_target(&mut parsed.targets, &mut current_name, &mut current_path);
            in_test_section = section == "test";
            if in_test_section {
                parsed.has_explicit_section = true;
            }
            continue;
        }
        if parse_toml_section(line).is_some() {
            push_cargo_test_target(&mut parsed.targets, &mut current_name, &mut current_path);
            in_test_section = false;
            continue;
        }
        if !in_test_section {
            continue;
        }
        if let Some(value) = parse_toml_string_value(line, "name") {
            current_name = Some(value);
        } else if let Some(value) = parse_toml_string_value(line, "path") {
            current_path = Some(value);
        }
    }
    push_cargo_test_target(&mut parsed.targets, &mut current_name, &mut current_path);
    parsed
}

fn push_cargo_test_target(
    targets: &mut Vec<CargoTestTarget>,
    name: &mut Option<String>,
    path: &mut Option<String>,
) {
    let (Some(name_value), Some(path_value)) = (name.take(), path.take()) else {
        return;
    };
    if name_value.is_empty() || path_value.is_empty() {
        return;
    }
    targets.push(CargoTestTarget {
        name: name_value,
        path: path_value,
    });
}

fn parse_toml_string_value(line: &str, key: &str) -> Option<String> {
    let (raw_key, raw_value) = line.split_once('=')?;
    if raw_key.trim() != key {
        return None;
    }
    let value = raw_value.trim();
    let quote = value.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &value[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn cargo_integration_test_name_for_manifest(work_root: &Path, path: &str) -> Option<String> {
    let manifest = std::fs::read_to_string(work_root.join("Cargo.toml")).ok()?;
    cargo_test_targets_from_manifest(&manifest)
        .targets
        .into_iter()
        .find(|target| target.path == path)
        .map(|target| target.name)
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
                "python3 -m pip install {} && PYTHONPATH=src:. python3 -m pytest -q -p no:cacheprovider",
                packages.join(" ")
            ),
            reason: "Python tests detected with pyproject.toml; installing declared test dependencies before pytest".to_string(),
            confidence: 0.8,
            evidence: vec!["python-toolchain:pip-direct-deps".to_string()],
        };
    }
    PythonPytestCommand {
        command: "python3 -m pytest -q -p no:cacheprovider".to_string(),
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

fn python_inferred_test_packages_from_sources(work_root: &Path) -> Vec<String> {
    const MAX_PYTHON_SOURCE_FILES: usize = 64;
    const MAX_PYTHON_SOURCE_BYTES: u64 = 256 * 1024;
    const MAX_INFERRED_PACKAGES: usize = 24;

    let mut local_modules = python_local_top_level_modules(work_root);
    local_modules.insert("tests".to_string());

    let mut packages = BTreeSet::new();
    packages.insert("pytest".to_string());
    let mut stack = vec![work_root.to_path_buf()];
    let mut visited_files = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if python_dependency_scan_ignored_name(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("py") {
                continue;
            }
            if visited_files >= MAX_PYTHON_SOURCE_FILES {
                break;
            }
            visited_files = visited_files.saturating_add(1);
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_PYTHON_SOURCE_BYTES {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            for import in python_imported_top_level_names(&contents) {
                if local_modules.contains(&import) || python_stdlib_top_level_name(&import) {
                    continue;
                }
                let package = normalize_dependency_name(&import);
                if is_safe_python_package_name(&package) {
                    packages.insert(package);
                }
            }
            if contents.contains("fastapi.testclient") {
                packages.insert("httpx".to_string());
            }
        }
    }

    packages.into_iter().take(MAX_INFERRED_PACKAGES).collect()
}

fn python_local_top_level_modules(work_root: &Path) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return modules;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if python_dependency_scan_ignored_name(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            if path.extension().and_then(|ext| ext.to_str()) == Some("py")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                modules.insert(stem.to_string());
            }
            continue;
        }
        if file_type.is_dir() && python_dir_contains_python_source(&path) {
            modules.insert(name);
        }
    }
    modules
}

fn python_dir_contains_python_source(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.extension().and_then(|ext| ext.to_str()) == Some("py")
    })
}

fn python_dependency_scan_ignored_name(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "__pycache__"
                | ".anvil-state"
                | ".git"
                | ".hg"
                | ".mypy_cache"
                | ".pytest_cache"
                | ".ruff_cache"
                | ".tox"
                | ".venv"
                | "dist"
                | "node_modules"
                | "target"
                | "venv"
        )
}

fn python_imported_top_level_names(contents: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with("from .") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("import ") {
            for part in rest.split(',') {
                let candidate = part
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .split('.')
                    .next()
                    .unwrap_or("");
                if python_import_name_is_safe(candidate) {
                    names.insert(candidate.to_string());
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("from ") {
            let candidate = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("");
            if python_import_name_is_safe(candidate) {
                names.insert(candidate.to_string());
            }
        }
    }
    names
}

fn python_import_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
}

fn python_stdlib_top_level_name(name: &str) -> bool {
    matches!(
        name,
        "__future__"
            | "abc"
            | "argparse"
            | "asyncio"
            | "base64"
            | "collections"
            | "contextlib"
            | "csv"
            | "dataclasses"
            | "datetime"
            | "decimal"
            | "enum"
            | "functools"
            | "glob"
            | "hashlib"
            | "http"
            | "importlib"
            | "inspect"
            | "io"
            | "itertools"
            | "json"
            | "logging"
            | "math"
            | "os"
            | "pathlib"
            | "random"
            | "re"
            | "shutil"
            | "sqlite3"
            | "statistics"
            | "string"
            | "subprocess"
            | "sys"
            | "tempfile"
            | "time"
            | "traceback"
            | "types"
            | "typing"
            | "unittest"
            | "uuid"
    )
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
    changed_files
        .iter()
        .any(|path| path.ends_with(".py") && !is_ignored_workspace_display_path(path))
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
            path.ends_with(".py")
                && !path.starts_with("tests/")
                && !path.starts_with("test_")
                && !is_ignored_workspace_display_path(path)
        })
        .map(PathBuf::from)
}

fn visible_changed_files(changed_files: &[String]) -> Vec<String> {
    changed_files
        .iter()
        .filter(|path| !is_ignored_workspace_display_path(path))
        .cloned()
        .collect()
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
    use super::super::task_contract::TaskKind;
    use super::*;
    use tempfile::tempdir;

    // Issue #918 (P1) Task 6+7: the structured-verifier process-spawn gate.
    // Every non-coding kind must fail closed with an Err (no process spawned),
    // observable in the repo's DEBUG `cargo test` CI (the gate is a real `Err`,
    // not a `debug_assert!` panic). This is the security backstop for High gap #2.
    #[test]
    fn run_structured_blocks_non_coding_process_spawn_fail_closed() {
        let dir = tempdir().expect("tempdir");
        let scope = TaskWorkspaceScope::detect(dir.path(), "");
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&["tests/test_main.py".to_string()])
                .expect("pytest command");

        for kind in [
            TaskKind::Docs,
            TaskKind::Data,
            TaskKind::Research,
            TaskKind::Ops,
            TaskKind::Authoring,
        ] {
            let result =
                AutoTestRunner::run_structured(dir.path(), &scope, &command, "pytest -q", kind);
            let err = result.expect_err("non-coding kind must fail closed at the spawn gate");
            assert!(
                err.contains("not permitted for task kind"),
                "{kind:?}: expected spawn-gate Err, got: {err}"
            );
        }
    }

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
        assert_eq!(plan.command, "python3 -m pytest -q -p no:cacheprovider");
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

    #[test]
    fn evidence_command_hint_accepts_safe_project_verifier() {
        let plan = AutoTestRunner::plan_from_evidence_command_hint(
            "cargo test --manifest-path Cargo.toml",
        )
        .expect("safe cargo test hint");

        assert_eq!(plan.command, "cargo test --manifest-path Cargo.toml");
        assert_eq!(plan.reason, "controller evidence command");
    }

    #[test]
    fn evidence_command_hint_accepts_stdlib_unittest_verifier() {
        let plan = AutoTestRunner::plan_from_evidence_command_hint(
            "python3 -m unittest discover -s tests",
        )
        .expect("safe unittest hint");

        assert_eq!(plan.command, "python3 -m unittest discover -s tests");
        assert_eq!(plan.reason, "controller evidence command");
    }

    #[test]
    fn evidence_command_hint_rejects_shell_control() {
        assert!(
            AutoTestRunner::plan_from_evidence_command_hint("cargo test || true").is_none(),
            "controller hints must not be able to mask verifier failures"
        );
    }

    #[test]
    fn evidence_command_hint_rejects_non_verifier_command() {
        assert!(AutoTestRunner::plan_from_evidence_command_hint("echo ok").is_none());
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
            command: "python3 -m pytest -q -p no:cacheprovider".to_string(),
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

    #[test]
    fn cargo_test_targets_from_manifest_maps_explicit_path_to_name() {
        let parsed = cargo_test_targets_from_manifest(
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\n\n[[test]]\nname = \"password_strength_tests\"\npath = \"tests/password_strength.rs\"\n",
        );

        assert!(parsed.has_explicit_section);
        assert_eq!(
            parsed.targets,
            vec![CargoTestTarget {
                name: "password_strength_tests".to_string(),
                path: "tests/password_strength.rs".to_string(),
            }]
        );
    }

    #[test]
    fn cargo_manifest_target_name_precedes_path_stem_for_preflight() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"password_strength\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"password_strength_tests\"\npath = \"tests/password_strength.rs\"\n",
        )
        .expect("write");

        assert_eq!(
            cargo_integration_test_name_for_manifest(dir.path(), "tests/password_strength.rs")
                .as_deref(),
            Some("password_strength_tests")
        );
        assert_eq!(
            cargo_integration_test_name("tests/password_strength.rs").as_deref(),
            Some("password_strength")
        );
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
            "python3 -m pip install pytest && PYTHONPATH=src:. python3 -m pytest -q -p no:cacheprovider"
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
    fn python_pytest_without_pyproject_infers_safe_import_packages_for_structured_setup() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("main.py"),
            "from fastapi import FastAPI\nfrom pydantic import BaseModel\nfrom uuid import uuid4\nimport types\n",
        )
        .expect("main");
        std::fs::write(
            dir.path().join("tests/test_main.py"),
            "from fastapi.testclient import TestClient\nfrom main import app\n",
        )
        .expect("test");
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&["tests/test_main.py".to_string()])
                .expect("pytest command");

        let packages =
            structured_python_pytest_dependency_setup_packages(dir.path(), &command).unwrap();

        assert!(packages.contains(&"fastapi".to_string()));
        assert!(packages.contains(&"httpx".to_string()));
        assert!(packages.contains(&"pydantic".to_string()));
        assert!(packages.contains(&"pytest".to_string()));
        assert!(!packages.contains(&"main".to_string()));
        assert!(!packages.contains(&"types".to_string()));
        assert!(!packages.contains(&"uuid".to_string()));
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
        let owned = vec!["tests/test_a.rs".to_string()];
        let command =
            VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo is allowlisted");
        assert_eq!(command.runner(), "cargo");
        assert_eq!(command.args(), vec!["test".to_string()].as_slice());
        assert_eq!(command.bound_test_artifacts(), owned.as_slice());
        // Display string joins with single spaces — no `shlex`.
        assert_eq!(command.to_display_string(), "cargo test");
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
                assert_eq!(command.args(), vec!["test".to_string()].as_slice());
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
                assert!(command.args().contains(&"-q".to_string()));
                assert_eq!(
                    command.bound_test_artifacts(),
                    &["tests/test_x.py".to_string()],
                    "owned test artifact must be retained as binding metadata"
                );
                assert!(
                    !command.args().contains(&"tests/test_x.py".to_string()),
                    "python verifier should run the full suite instead of filtering to owned tests"
                );
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    #[test]
    fn detect_owned_for_pyproject_python_project_prefers_bound_stdlib_pytest() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\ndependencies = ['fastapi', 'pytest']\n",
        )
        .expect("pyproject");
        let owned = vec!["tests/test_main.py".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["main.py".to_string()],
            &[],
            &owned,
        );
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "python3");
                assert!(command.args().contains(&"pytest".to_string()));
                assert_eq!(
                    command.bound_test_artifacts(),
                    &["tests/test_main.py".to_string()],
                    "pyproject verifier must keep owned test artifacts as binding metadata"
                );
                assert!(
                    !command.args().contains(&"tests/test_main.py".to_string()),
                    "python verifier should run the full suite instead of filtering to owned tests"
                );
                assert!(
                    !command.to_display_string().contains("pip install"),
                    "structured task-contract verifier must not rely on shell setup"
                );
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    #[test]
    fn project_unit_filters_owned_verifier_to_current_task_stack() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='outer'\nversion='0.0.0'\n",
        )
        .expect("cargo manifest");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests dir");

        let owned = vec!["tests/test_main.py".to_string()];
        let unfiltered = AutoTestRunner::detect_with_owned_test_artifacts(
            dir.path(),
            &["app/main.py".to_string(), "tests/test_main.py".to_string()],
            &[],
            &owned,
        );
        match unfiltered {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "cargo");
                assert_eq!(command.args(), vec!["test".to_string()].as_slice());
            }
            other => panic!("expected unfiltered cargo verifier, got {other:?}"),
        }

        let mut artifact_roles = BTreeSet::new();
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Implementation);
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Test);
        let project_unit = super::super::project_probe::ProjectUnit {
            root: ".".to_string(),
            manifests: Vec::new(),
            artifact_roles,
            verifier_candidates: vec![super::super::project_probe::ProjectUnitVerifierCandidate {
                command_preview: "python3 -m pytest -q -p no:cacheprovider".to_string(),
                source: "python_tests",
                timeout_class: super::super::project_probe::ProjectUnitTimeoutClass::ShortUnitTest,
            }],
            observed_stacks: vec!["python"],
            confidence: super::super::project_probe::ProjectUnitConfidence::High,
        };

        let filtered = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            dir.path(),
            &["app/main.py".to_string(), "tests/test_main.py".to_string()],
            &[],
            &owned,
            Some(&project_unit),
        );
        match filtered {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "python3");
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected project-unit filtered python verifier, got {other:?}"),
        }
    }

    #[test]
    fn project_unit_unittest_preview_builds_structured_unittest_verifier() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests dir");

        let owned = vec!["tests/test_math_utils.py".to_string()];
        let mut artifact_roles = BTreeSet::new();
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Implementation);
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Test);
        let project_unit = super::super::project_probe::ProjectUnit {
            root: ".".to_string(),
            manifests: Vec::new(),
            artifact_roles,
            verifier_candidates: vec![super::super::project_probe::ProjectUnitVerifierCandidate {
                command_preview: "python3 -m unittest discover -s tests".to_string(),
                source: "python_tests",
                timeout_class: super::super::project_probe::ProjectUnitTimeoutClass::ShortUnitTest,
            }],
            observed_stacks: vec!["python"],
            confidence: super::super::project_probe::ProjectUnitConfidence::High,
        };

        let plan = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            dir.path(),
            &[
                "math_utils.py".to_string(),
                "tests/test_math_utils.py".to_string(),
            ],
            &[],
            &owned,
            Some(&project_unit),
        );

        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "python3");
                assert_eq!(
                    command.args(),
                    vec![
                        "-m".to_string(),
                        "unittest".to_string(),
                        "discover".to_string(),
                        "-s".to_string(),
                        "tests".to_string(),
                    ]
                    .as_slice()
                );
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected structured unittest verifier, got {other:?}"),
        }
    }

    #[test]
    fn project_unit_without_verifier_candidates_does_not_fall_back_to_root_guess() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='outer'\nversion='0.0.0'\n",
        )
        .expect("cargo manifest");
        let mut artifact_roles = BTreeSet::new();
        artifact_roles.insert(super::super::task_contract::ArtifactRole::UsageDocs);
        let project_unit = super::super::project_probe::ProjectUnit {
            root: ".".to_string(),
            manifests: Vec::new(),
            artifact_roles,
            verifier_candidates: Vec::new(),
            observed_stacks: Vec::new(),
            confidence: super::super::project_probe::ProjectUnitConfidence::Low,
        };

        let plan = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            dir.path(),
            &["README.md".to_string()],
            &[],
            &["tests/test_main.py".to_string()],
            Some(&project_unit),
        );
        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    #[test]
    fn owned_python_test_fallback_restores_runnable_when_project_unit_lost_candidate() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests dir");
        std::fs::write(
            dir.path().join("password_strength.py"),
            "def password_score(password: str) -> int:\n    return 0\n",
        )
        .expect("impl");
        std::fs::write(
            dir.path().join("tests/test_password_strength.py"),
            "from password_strength import password_score\n\ndef test_empty():\n    assert password_score('') == 0\n",
        )
        .expect("test");
        std::fs::write(
            dir.path().join("pytest.ini"),
            "[pytest]\ntestpaths = tests\n",
        )
        .expect("pytest");

        let mut artifact_roles = BTreeSet::new();
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Implementation);
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Test);
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Setup);
        let project_unit = super::super::project_probe::ProjectUnit {
            root: ".".to_string(),
            manifests: Vec::new(),
            artifact_roles,
            verifier_candidates: Vec::new(),
            observed_stacks: vec!["python"],
            confidence: super::super::project_probe::ProjectUnitConfidence::Medium,
        };
        let owned = vec!["tests/test_password_strength.py".to_string()];

        let plan = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            dir.path(),
            &["pytest.ini".to_string()],
            &[],
            &owned,
            Some(&project_unit),
        );

        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "python3");
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected owned python test fallback runnable, got {other:?}"),
        }
    }

    #[test]
    fn owned_test_fallback_does_not_cross_stack_or_extension() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests dir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='outer'\nversion='0.0.0'\n",
        )
        .expect("manifest");
        std::fs::write(dir.path().join("src.rs"), "pub fn value()->u8{1}\n").expect("rust");
        std::fs::write(
            dir.path().join("tests/test_main.py"),
            "def test_x(): pass\n",
        )
        .expect("python test");

        let mut artifact_roles = BTreeSet::new();
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Implementation);
        artifact_roles.insert(super::super::task_contract::ArtifactRole::Test);
        let project_unit = super::super::project_probe::ProjectUnit {
            root: ".".to_string(),
            manifests: vec!["Cargo.toml".to_string()],
            artifact_roles,
            verifier_candidates: Vec::new(),
            observed_stacks: vec!["rust"],
            confidence: super::super::project_probe::ProjectUnitConfidence::Medium,
        };

        let plan = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            dir.path(),
            &["pytest.ini".to_string()],
            &[],
            &["tests/test_main.py".to_string()],
            Some(&project_unit),
        );

        assert_eq!(plan, OwnedTestVerifierPlan::Missing);
    }

    #[test]
    fn changed_files_ignore_controller_owned_state_for_verifier_detection() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".anvil-state/generated")).expect("state dir");
        std::fs::write(
            dir.path().join(".anvil-state/generated/test_generated.py"),
            "def test_generated(): pass\n",
        )
        .expect("state file");

        let changed = vec![".anvil-state/generated/test_generated.py".to_string()];
        assert!(!has_python_surface(dir.path(), &changed));
        assert_eq!(first_python_script(&changed), None);
        assert!(
            AutoTestRunner::detect_candidates(dir.path(), &changed).is_empty(),
            "controller-owned changed files must not create verifier candidates"
        );
    }

    #[test]
    fn structured_python_verifier_preflights_generated_test_syntax() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".anvil-state/generated")).expect("state dir");
        std::fs::write(
            dir.path().join(".anvil-state/generated/test_generated.py"),
            "def test_generated(:\n    pass\n",
        )
        .expect("generated test");
        let owned = vec![".anvil-state/generated/test_generated.py".to_string()];
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 pytest command");
        let env_plan = build_hermetic_env_plan(dir.path(), VERIFIER_ENV_PYTHON_EXTRA);

        let result = run_structured_generated_test_preflight(
            "python3",
            &env_plan,
            dir.path(),
            &command,
            &command.to_display_string(),
        )
        .expect("preflight should run")
        .expect("syntax failure should be returned before pytest");

        assert!(!result.passed);
        assert!(result.command.contains("py_compile"), "{}", result.command);
        assert!(
            result.output.contains("SyntaxError") || result.output.contains("syntax"),
            "{}",
            result.output
        );
    }

    #[test]
    fn structured_python_pytest_dependency_setup_packages_reads_pyproject() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\ndependencies = ['fastapi', 'httpx']\n",
        )
        .expect("pyproject");
        let owned = vec!["tests/test_main.py".to_string()];
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 pytest command");

        assert_eq!(
            structured_python_pytest_dependency_setup_packages(dir.path(), &command),
            Some(vec![
                "fastapi".to_string(),
                "httpx".to_string(),
                "pytest".to_string()
            ])
        );
    }

    #[test]
    fn structured_python_pytest_dependency_setup_packages_bootstraps_pytest_without_pyproject() {
        let dir = tempdir().expect("tempdir");
        let owned = vec!["tests/test_main.py".to_string()];
        let pytest_command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 pytest command");
        assert_eq!(
            structured_python_pytest_dependency_setup_packages(dir.path(), &pytest_command),
            Some(vec!["pytest".to_string()])
        );

        std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = 'x'\n")
            .expect("pyproject");
        let cargo_command =
            VerifierCommand::from_cargo_test(Vec::new(), &["tests/test_main.rs".to_string()])
                .expect("cargo command");
        assert_eq!(
            structured_python_pytest_dependency_setup_packages(dir.path(), &cargo_command),
            None
        );
    }

    #[test]
    fn structured_node_dependency_setup_required_for_npm_test_dependencies_without_node_modules() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"^1.0.0"}}"#,
        )
        .expect("package");
        let owned = vec!["tests/index.test.js".to_string()];
        let command = VerifierCommand::from_npm_test(&owned).expect("npm test");

        assert!(structured_node_dependency_setup_required(
            dir.path(),
            &command
        ));
    }

    #[test]
    fn structured_node_dependency_setup_skips_when_dependencies_absent_or_installed() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node tests/index.test.js"}}"#,
        )
        .expect("package");
        let owned = vec!["tests/index.test.js".to_string()];
        let command = VerifierCommand::from_npm_test(&owned).expect("npm test");

        assert!(!structured_node_dependency_setup_required(
            dir.path(),
            &command
        ));

        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"vitest run"},"dependencies":{"vitest":"^1.0.0"}}"#,
        )
        .expect("package with deps");
        std::fs::create_dir(dir.path().join("node_modules")).expect("node_modules");

        assert!(!structured_node_dependency_setup_required(
            dir.path(),
            &command
        ));
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
        // Issue #661 iteration-3 Task 3.4 path #3: the scope/symlink
        // gate is now routed through `classify_ownership` which fires
        // `OutOfScope` (workspace-relative / canonical_escape SSOT)
        // before the explicit canonicalize-strip-prefix backstop below.
        // Either error message satisfies the regression (symlink
        // escape must reject), with the classify_ownership path being
        // the primary route after the SSOT switch.
        assert!(
            err.contains("not in TaskWorkspaceScope")
                || err.contains("canonicalization escapes work_root"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_shell_auto_test_is_bounded_by_timeout() {
        let dir = tempdir().expect("tempdir");
        let plan = AutoTestPlan {
            command: "sleep 60".to_string(),
            reason: "timeout regression".to_string(),
        };

        let result = AutoTestRunner::run_with_timeout(dir.path(), &plan, Duration::from_millis(50));

        let err = result.expect_err("legacy shell verifier must be bounded");
        assert!(
            err.contains("auto test command timed out after"),
            "expected timeout error, got: {err}"
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
    // Issue #865: Rust final success verifier is full `cargo test`.
    // -----------------------------------------------------------------

    #[test]
    fn verifier_command_from_cargo_test_runs_full_suite() {
        let owned = vec!["tests/integration_one.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        assert_eq!(
            command.args(),
            vec!["test".to_string()].as_slice(),
            "Rust final success verifier must be full cargo test"
        );
    }

    #[test]
    fn verifier_command_from_cargo_test_accepts_src_internal_test_artifact() {
        let owned = vec!["src/lib/foo.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        assert_eq!(command.args(), vec!["test".to_string()].as_slice());
        assert_eq!(command.bound_test_artifacts(), owned.as_slice());
    }

    #[test]
    fn verifier_command_from_cargo_test_accepts_owned_artifact_without_filtering() {
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        assert_eq!(command.args(), vec!["test".to_string()].as_slice());
    }

    #[test]
    fn verifier_command_from_node_test_binds_direct_js_test_paths() {
        let owned = vec!["tests/index.test.js".to_string()];
        let command = VerifierCommand::from_node_test(&owned).expect("node");
        assert_eq!(
            command.args(),
            vec!["--test".to_string(), "tests/index.test.js".to_string()].as_slice()
        );
        assert_eq!(command.bound_test_artifacts(), owned.as_slice());
    }

    #[test]
    fn verifier_command_from_node_test_rejects_typescript_paths() {
        let owned = vec!["tests/index.test.ts".to_string()];
        assert!(
            VerifierCommand::from_node_test(&owned).is_none(),
            "plain node --test must not claim TypeScript test artifacts"
        );
    }

    #[test]
    fn detect_owned_for_package_json_script_returns_npm_test_runnable() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node --test tests/*.test.js"}}"#,
        )
        .expect("package");
        let owned = vec!["tests/index.test.js".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &owned);
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "npm");
                assert_eq!(command.args(), vec!["test".to_string()].as_slice());
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected Runnable, got {other:?}"),
        }
    }

    #[test]
    fn verifier_command_from_cargo_test_accepts_nested_tests_path_for_full_suite() {
        let owned = vec!["tests/sub/dir.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        assert_eq!(command.args(), vec!["test".to_string()].as_slice());
    }

    #[test]
    fn detect_owned_for_cargo_project_with_src_test_returns_runnable() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let owned = vec!["src/lib/foo.rs".to_string()];
        let plan = AutoTestRunner::detect_with_owned_test_artifacts(dir.path(), &[], &[], &owned);
        match plan {
            OwnedTestVerifierPlan::Runnable { command, .. } => {
                assert_eq!(command.runner(), "cargo");
                assert_eq!(command.args(), vec!["test".to_string()].as_slice());
                assert_eq!(command.bound_test_artifacts(), owned.as_slice());
            }
            other => panic!("expected Runnable, got {other:?}"),
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
            path: None,
        });
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Test,
            count: 1,
            path: None,
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

    // ----------------------------------------------------------------
    // Issue #661 Task 2.2: RunnerKind enum contract (OCP 拡張口 / serialize
    // 戦略 DR2-004)。as_str() is the **only** serialize path — adding a new
    // variant without updating the match must fail to compile.
    // ----------------------------------------------------------------

    #[test]
    fn runner_kind_cargo_serializes_as_cargo_string() {
        assert_eq!(RunnerKind::Cargo.as_str(), "cargo");
    }

    #[test]
    fn runner_kind_python3_serializes_as_python3_string() {
        assert_eq!(RunnerKind::Python3.as_str(), "python3");
    }

    #[test]
    fn runner_kind_npm_serializes_as_npm_string() {
        assert_eq!(RunnerKind::Npm.as_str(), "npm");
    }

    #[test]
    fn runner_kind_is_copy_clone_eq() {
        // Compile-time: Copy + Clone + PartialEq + Eq (required for
        // downstream payload dedup and value passing).
        let kind = RunnerKind::Cargo;
        let copy = kind;
        let clone = kind;
        assert_eq!(kind, copy);
        assert_eq!(copy, clone);
    }

    /// CB-007 regression: every `RunnerKind` variant must round-trip through
    /// `from_runner_str(as_str())`. Adding a new variant without updating
    /// `from_runner_str` will silently fall through to `None` at runtime;
    /// this test makes that case fail at test time (next-best guard since
    /// `#[non_exhaustive]` + free function `from_runner_str` cannot enforce
    /// it at compile time).
    #[test]
    fn runner_kind_as_str_from_runner_str_round_trip_covers_all_variants() {
        // List every variant explicitly. When a new variant is added, this
        // test forces the author to extend both the list and `from_runner_str`.
        let all = [RunnerKind::Cargo, RunnerKind::Python3, RunnerKind::Npm];
        for kind in all {
            let serialized = kind.as_str();
            let parsed = RunnerKind::from_runner_str(serialized).unwrap_or_else(|| {
                panic!(
                    "RunnerKind::{:?}.as_str() = {:?} must round-trip via from_runner_str",
                    kind, serialized
                )
            });
            assert_eq!(parsed, kind, "round-trip mismatch for {:?}", kind);
        }
    }

    // ----------------------------------------------------------------
    // Issue #661 Task 2.3: PathHashHex newtype contract (DR1-012 / DR2-007)。
    // - raw path を渡したら mask_secrets 後の文字列を入力に stable_path_hash で
    //   16-hex に統一化。同じ入力には deterministic な出力。
    // - 異なる入力は別 hash。
    // ----------------------------------------------------------------

    #[test]
    fn path_hash_hex_returns_sixteen_hex_chars() {
        let hash = PathHashHex::from_relative_str("src/foo.rs");
        let s = hash.as_str();
        assert_eq!(
            s.len(),
            16,
            "PathHashHex must always be 16 hex chars, got {s:?}"
        );
        assert!(
            s.chars().all(|c| c.is_ascii_hexdigit()),
            "PathHashHex must contain only ASCII hex digits, got {s:?}"
        );
    }

    #[test]
    fn path_hash_hex_is_deterministic_for_identical_inputs() {
        let a = PathHashHex::from_relative_str("app/tests/test_a.py");
        let b = PathHashHex::from_relative_str("app/tests/test_a.py");
        assert_eq!(
            a.as_str(),
            b.as_str(),
            "identical inputs must hash to identical 16-hex strings"
        );
    }

    #[test]
    fn path_hash_hex_differs_for_different_inputs() {
        let a = PathHashHex::from_relative_str("app/tests/test_a.py");
        let b = PathHashHex::from_relative_str("app/tests/test_b.py");
        assert_ne!(
            a.as_str(),
            b.as_str(),
            "distinct paths must hash to distinct 16-hex strings (collision unlikely with DefaultHasher)"
        );
    }

    #[test]
    fn path_hash_hex_matches_logging_ssot() {
        // PathHashHex MUST go through `crate::logging::stable_path_hash`
        // applied to `mask_secrets(input)` so the per-Issue path_hash
        // representations stay byte-identical across #659/#660/#661.
        let input = "src/foo.rs";
        let masked = crate::session::feedback::mask_secrets(input);
        let expected = crate::logging::stable_path_hash(&masked);
        assert_eq!(
            PathHashHex::from_relative_str(input).as_str(),
            expected.as_str(),
            "PathHashHex must reuse logging::stable_path_hash SSOT after mask_secrets"
        );
    }

    // ----------------------------------------------------------------
    // Issue #661 (iteration-3 Task 3.4 path #3): execution-time
    // validator now consults `classify_ownership` with
    // `NestedTestAdmission::enabled()` so nested test subdirs are
    // admitted via the SSOT propagation route.
    //
    // The validator already rejected absolute / symlink-escape /
    // ignored-top-dir paths via independent manual checks; the
    // classify_ownership call replaces the older `scope.contains` step
    // and adds workspace-relative / role-confirm coverage as defense
    // in depth.
    // ----------------------------------------------------------------

    #[test]
    fn validate_bound_artifacts_accepts_nested_test_subdir_with_admission() {
        // `app/tests/test_a.py` must pass execution-time validation
        // when the file exists and the path is in scope. Iteration-3
        // pins the new SSOT route: `classify_ownership` with
        // `NestedTestAdmission::enabled()`.
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("app/tests")).unwrap();
        std::fs::write(dir.path().join("app/tests/test_a.py"), "").unwrap();
        let scope = single_root_scope_for_validation();
        let owned = vec!["app/tests/test_a.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(dir.path(), &scope, &command);
        assert!(
            result.is_ok(),
            "nested-test-subdir path must pass with admission enabled, got {result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_bound_artifacts_rejects_symlink_escape_via_classify_ownership() {
        // SSOT consistency: when a symlinked leaf escapes work_root,
        // the classify_ownership-based gate fires first and produces
        // the canonical-escape error. The defense-in-depth canonicalize
        // step at the bottom of the helper remains as a backstop, but
        // the classify_ownership path must reject the same input shape
        // so the same gate covers both record_* helpers and the
        // execution-time validator.
        use std::os::unix::fs::symlink;
        let outside = tempdir().expect("outside");
        std::fs::write(outside.path().join("escape_test.py"), "").unwrap();
        let work = tempdir().expect("work");
        symlink(
            outside.path().join("escape_test.py"),
            work.path().join("test_escape.py"),
        )
        .expect("symlink");
        let scope = single_root_scope_for_validation();
        let owned = vec!["test_escape.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let result = validate_bound_test_artifacts_for_execution(work.path(), &scope, &command);
        assert!(
            result.is_err(),
            "symlink-escape must reject regardless of which gate fires first"
        );
    }

    // ----------------------------------------------------------------
    // Issue #661 iteration-5 Phase B: HermeticEnvPlan / HermeticEnvSummary
    // 本実装 (Task 6.1 apply_to + Task 6.2 VERIFIER_ENV_PYTHON_EXTRA +
    // Task 6.3 summary 実値 + Task 7.1 PYTHONPATH 検査)
    // ----------------------------------------------------------------

    #[test]
    fn build_hermetic_env_plan_phase_b_sets_cwd_to_work_root() {
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), &[]);
        let summary = plan.summary();
        assert!(
            summary.cwd_inside_work_root,
            "build_hermetic_env_plan must set cwd to work_root itself"
        );
    }

    #[test]
    fn build_hermetic_env_plan_python_extras_pin_pythonpath_to_work_root() {
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), VERIFIER_ENV_PYTHON_EXTRA);
        let summary = plan.summary();
        assert!(
            summary.pythonpath_root,
            "Python verifier extras must pin PYTHONPATH to work_root even when parent PYTHONPATH is absent"
        );
    }

    #[test]
    fn structured_python_dependency_site_is_controller_state_and_second_on_pythonpath() {
        let work = tempdir().expect("work");
        let dependency_site = structured_python_dependency_site_dir(work.path());
        let relative_site = dependency_site
            .strip_prefix(work.path())
            .expect("site under work root");
        assert!(
            crate::util::workspace_paths::is_ignored_workspace_relative_path(relative_site),
            "structured dependency site must remain controller-owned ignored state"
        );

        let pythonpath =
            structured_python_dependency_site_pythonpath(work.path(), &dependency_site)
                .expect("pythonpath");
        let entries: Vec<PathBuf> = std::env::split_paths(&pythonpath).collect();
        assert_eq!(entries.first().map(PathBuf::as_path), Some(work.path()));
        assert_eq!(entries.get(1), Some(&dependency_site));
        assert_eq!(
            entries.len(),
            2,
            "user workspace must be searched before verifier dependency site, with no extra parent PYTHONPATH"
        );
    }

    /// Phase B Task 6.1: `apply_to` MUST `env_clear()` + re-inject allowlist
    /// keys. We spawn `printenv` (Unix) and check that a non-allowlist key
    /// (`API_KEY_FAKE`) is dropped while PATH is preserved.
    #[cfg(unix)]
    #[test]
    fn hermetic_env_plan_apply_to_drops_secret_like_keys_phase_b() {
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), &[]);
        // Use a fresh Command without inheriting our test's env via
        // `Command::new("sh")`-then-`env_clear` already happens in apply_to.
        // We set the secret on the child via cmd.env() to prove apply_to's
        // env_clear actually wipes it (env_clear must run before re-inject).
        let mut cmd = Command::new("sh");
        cmd.env("API_KEY_FAKE", "supersecret");
        cmd.args([
            "-c",
            "printf 'API_KEY_FAKE=%s\\n' \"${API_KEY_FAKE:-MISSING}\"",
        ]);
        plan.apply_to(&mut cmd);
        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn shell");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("API_KEY_FAKE=MISSING"),
            "apply_to must env_clear + re-inject allowlist only; got stdout={stdout:?}"
        );
    }

    /// Phase B Task 6.1: PATH MUST stay inherited (it's on the allowlist).
    #[cfg(unix)]
    #[test]
    fn hermetic_env_plan_apply_to_preserves_path_phase_b() {
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), &[]);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf '%s' \"$PATH\""]);
        plan.apply_to(&mut cmd);
        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn sh");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.trim().is_empty(),
            "Phase B: PATH must remain inherited via filter_env_for_tester allowlist"
        );
    }

    /// Phase B Task 6.1 / DR1-009: `CARGO_HOME` / `RUSTUP_HOME` must remain
    /// inherited via `TESTER_ENV_ALLOWLIST_EXACT` so cargo cache warmup is
    /// not destroyed by hermetic env. This is the regression guard for the
    /// design decision in セクション 4 判断 #2 (allowlist 通過 inherit) and
    /// CB-008 (PATH hijack 防止後の cargo path 解決を実用範囲に保つ).
    #[cfg(unix)]
    #[test]
    fn hermetic_env_plan_apply_to_preserves_cargo_home_phase_b() {
        // `apply_to` always re-injects from `filter_env_for_tester(std::env::vars())`
        // which reads the **parent** (test runner) process env. CARGO_HOME is
        // virtually always set in the cargo test runtime, so we use the parent
        // value as the source of truth and assert it round-trips into the child.
        let parent_cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| String::new());
        if parent_cargo_home.is_empty() {
            // Skip on the rare environment that does not set CARGO_HOME at all
            // (e.g. a minimal sandbox); the regression target is "inherit when
            // present", not "synthesise from nothing".
            return;
        }
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), &[]);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'CARGO_HOME=%s\\n' \"${CARGO_HOME:-MISSING}\""]);
        plan.apply_to(&mut cmd);
        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn sh");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = format!("CARGO_HOME={parent_cargo_home}");
        assert!(
            stdout.contains(&expected),
            "Phase B: CARGO_HOME (from parent {parent_cargo_home:?}) must survive env_clear via TESTER_ENV_ALLOWLIST_EXACT inherit; got stdout={stdout:?}"
        );
        assert!(
            !stdout.contains("CARGO_HOME=MISSING"),
            "Phase B: apply_to MUST NOT strip CARGO_HOME (cargo cache warmup would break)"
        );
    }

    /// Phase B Task 6.3: `HermeticEnvSummary` direct construction shape.
    #[test]
    fn hermetic_env_summary_phase_b_shape() {
        let summary = HermeticEnvSummary {
            allowlist_keys: vec!["PATH", "HOME"],
            pythonpath_root: true,
            cwd_inside_work_root: true,
        };
        assert_eq!(summary.allowlist_keys, vec!["PATH", "HOME"]);
        assert!(summary.pythonpath_root);
        assert!(summary.cwd_inside_work_root);
    }

    /// CB-006 (Phase B 移行 signal flipped): `summary.allowlist_keys` is
    /// populated with the actual `filter_env_for_tester` outcome once the
    /// hermetic env implementation is in place. The Phase A invariant that
    /// kept the field empty has been retired; we now require PATH to be
    /// present (it is essentially always set on any CI / dev shell).
    #[test]
    fn build_hermetic_env_plan_phase_b_summary_carries_real_allowlist_keys() {
        let work = tempdir().expect("work");
        let plan = build_hermetic_env_plan(work.path(), &[]);
        let summary = plan.summary();
        // PATH should always be present in any reasonable env we test under.
        assert!(
            summary.allowlist_keys.contains(&"PATH"),
            "Phase B: summary.allowlist_keys must include PATH from filter_env_for_tester; got {:?}",
            summary.allowlist_keys
        );
        assert!(
            summary.cwd_inside_work_root,
            "Phase B: cwd_inside_work_root must remain true (cwd == work_root)"
        );
    }

    /// Phase B Task 6.2 + v0.4.8 verifier sandbox: Python verifier extras
    /// must contain cache suppression, user-site suppression, and pytest
    /// third-party plugin autoload suppression.
    #[test]
    fn verifier_env_python_extra_contains_expected_keys() {
        let mut keys: Vec<&str> = VERIFIER_ENV_PYTHON_EXTRA.iter().map(|(k, _)| *k).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
                "PYTHONDONTWRITEBYTECODE",
                "PYTHONNOUSERSITE"
            ]
        );
        for (_, v) in VERIFIER_ENV_PYTHON_EXTRA {
            assert_eq!(*v, "1");
        }
    }

    /// Phase B Task 6.4 (DR4-002 leading-dash argv injection): a path that
    /// begins with `-` MUST reject `VerifierCommand` construction. Both
    /// `from_pytest` (forwards path verbatim) and `from_cargo_test` (stores
    /// bound metadata for full `cargo test`) must refuse.
    #[test]
    fn verifier_command_rejects_leading_dash_bound_path_from_pytest() {
        let owned = vec!["-malicious_test.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned);
        assert!(
            command.is_none(),
            "Phase B DR4-002: leading-dash bound path must reject construction"
        );
    }

    #[test]
    fn verifier_command_accepts_normal_relative_path() {
        // Regression guard: ensure the leading-dash check did not over-fire.
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned);
        assert!(command.is_some());
    }

    /// Phase B Task 6.4 (DR4-001 PATH hijack): `resolved_runner_program`
    /// must return an absolute path when the planned PATH contains a real
    /// `sh` (always present on Unix). On non-Unix or missing-binary cases
    /// it falls back to the relative runner string.
    #[cfg(unix)]
    #[test]
    fn resolved_runner_program_returns_absolute_path_under_planned_path() {
        // Build a VerifierCommand with `python3` as runner; even if python3
        // isn't installed on the test runner, the resolver returns the bare
        // "python3" string. To deterministically exercise the success path,
        // we synthesize a tempdir with an executable file and inject it.
        use std::os::unix::fs::PermissionsExt;
        let work = tempdir().expect("work");
        let bin_dir = work.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_runner = bin_dir.join("python3");
        std::fs::write(&fake_runner, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&fake_runner).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_runner, perms).unwrap();
        let path_value = bin_dir.to_string_lossy().into_owned();
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 command");
        let resolved = command.resolved_runner_program(Some(&path_value));
        assert_eq!(resolved, fake_runner.to_string_lossy());
    }

    #[test]
    fn resolved_runner_program_falls_back_to_relative_when_unresolved() {
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 command");
        // Empty PATH → no candidate; relative argv is the best-effort fallback.
        let resolved = command.resolved_runner_program(Some(""));
        assert_eq!(resolved, "python3");
        // None planned PATH → same fallback.
        let resolved_none = command.resolved_runner_program(None);
        assert_eq!(resolved_none, "python3");
    }

    /// CB-008 (Codex iteration-5 high, DR4-001): a sanitized PATH must
    /// reject relative components and empty components. The hermetic
    /// runner-resolution path must NOT trust those — otherwise a process
    /// CWD with a hostile binary could hijack `python3` / `cargo`.
    #[cfg(unix)]
    #[test]
    fn sanitize_path_for_runner_resolution_drops_relative_and_empty() {
        let dropped =
            sanitize_path_for_runner_resolution("/usr/bin:relative/path::./also-relative", None);
        assert!(
            dropped.contains("/usr/bin"),
            "absolute /usr/bin must survive sanitization; got {dropped:?}"
        );
        for forbidden in ["relative/path", "./also-relative"] {
            assert!(
                !dropped.split(':').any(|c| c == forbidden),
                "relative component {forbidden:?} must be removed; got {dropped:?}"
            );
        }
        for component in dropped.split(':') {
            assert!(
                !component.is_empty(),
                "empty PATH components must be removed; got {dropped:?}"
            );
            assert!(
                std::path::Path::new(component).is_absolute(),
                "every surviving PATH component must be absolute; got {component:?} from {dropped:?}"
            );
        }
    }

    /// CB-008 (Codex iteration-5 high): a PATH component pointing inside
    /// `work_root` must be removed even if it is absolute, so a hostile
    /// `work_root/bin/python3` cannot hijack the runner. The sanitizer
    /// removes the work_root-anchored component but keeps `/usr/bin`.
    #[cfg(unix)]
    #[test]
    fn sanitize_path_for_runner_resolution_drops_work_root_anchored_components() {
        let work = tempdir().expect("work");
        let hostile_bin = work.path().join("bin");
        std::fs::create_dir_all(&hostile_bin).unwrap();
        let path = format!(
            "{hostile_bin}:/usr/bin",
            hostile_bin = hostile_bin.to_string_lossy()
        );
        let sanitized = sanitize_path_for_runner_resolution(&path, Some(work.path()));
        for component in sanitized.split(':') {
            assert!(
                !std::path::Path::new(component).starts_with(work.path()),
                "PATH component under work_root must be removed; got {component:?} from {sanitized:?}"
            );
        }
        assert!(
            sanitized.split(':').any(|c| c == "/usr/bin"),
            "/usr/bin must survive sanitization when work_root is supplied; got {sanitized:?}"
        );
    }

    /// CB-008 (Codex iteration-5 high, fail-closed): if the sanitized PATH
    /// has zero usable components, `resolved_runner_program` MUST return
    /// `None` (fail-closed) so the caller can surface a TransportError
    /// rather than spawning with a bare runner name that the parent PATH
    /// could then resolve to a hostile binary.
    #[cfg(unix)]
    #[test]
    fn resolved_runner_program_fail_closed_returns_none_when_no_resolution() {
        let owned = vec!["tests/test_a.py".to_string()];
        let command = VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 command");
        // A PATH made entirely of relative / empty components: every entry
        // must be sanitized out. fail_closed must return None.
        let result = command.resolved_runner_program_fail_closed(Some("relative:./also:"), None);
        assert!(
            result.is_none(),
            "all-relative PATH must produce a fail-closed None; got {result:?}"
        );
    }

    /// CB-008 (Codex iteration-5 high): CARGO_HOME / RUSTUP_HOME bins must
    /// be allowed by the sanitizer. We just check they survive when
    /// supplied as absolute paths; the toolchain root allowance is encoded
    /// implicitly by absolute-path acceptance (no extra denylist).
    #[cfg(unix)]
    #[test]
    fn sanitize_path_for_runner_resolution_keeps_absolute_toolchain_roots() {
        let cargo_bin = "/Users/me/.cargo/bin";
        let rustup_bin = "/Users/me/.rustup/bin";
        let path = format!("{cargo_bin}:{rustup_bin}");
        let sanitized = sanitize_path_for_runner_resolution(&path, None);
        assert!(
            sanitized.split(':').any(|c| c == cargo_bin),
            "CARGO_HOME bin must survive sanitization; got {sanitized:?}"
        );
        assert!(
            sanitized.split(':').any(|c| c == rustup_bin),
            "RUSTUP_HOME bin must survive sanitization; got {sanitized:?}"
        );
    }

    /// Phase B Task 7.1: `evaluate_pythonpath_for_hermetic_env` is a pure
    /// function over (work_root, raw PYTHONPATH) — exercising it directly
    /// avoids relying on the parent process's PYTHONPATH.
    #[test]
    fn evaluate_pythonpath_external_component_is_rejected() {
        let work = tempdir().expect("work");
        let (root, rejected) = evaluate_pythonpath_for_hermetic_env(
            work.path(),
            Some("/external/path:/another/external"),
        );
        assert!(
            root.is_some(),
            "PYTHONPATH presence must pin pythonpath_root to work_root"
        );
        assert!(
            rejected.is_some(),
            "external PYTHONPATH component must be flagged"
        );
    }

    #[test]
    fn evaluate_pythonpath_workspace_relative_is_accepted() {
        let work = tempdir().expect("work");
        let (root, rejected) = evaluate_pythonpath_for_hermetic_env(work.path(), Some("src:lib"));
        assert!(root.is_some());
        assert!(
            rejected.is_none(),
            "relative components must resolve under work_root without rejection"
        );
    }

    #[test]
    fn evaluate_pythonpath_empty_returns_none() {
        let work = tempdir().expect("work");
        let (root, rejected) = evaluate_pythonpath_for_hermetic_env(work.path(), None);
        assert!(
            root.is_none(),
            "no PYTHONPATH must result in env_remove (None)"
        );
        assert!(rejected.is_none());
        let (root_empty, rejected_empty) =
            evaluate_pythonpath_for_hermetic_env(work.path(), Some(""));
        assert!(root_empty.is_none());
        assert!(rejected_empty.is_none());
    }

    /// CB-010 (Codex iteration-5 medium): use the platform's path separator
    /// via `std::env::split_paths` rather than hard-coded `:` so callers on
    /// Windows do not get drive letters (`C:`) split incorrectly. The pure
    /// parser variant takes a `separator: char` so unit tests can drive
    /// both POSIX (`:`) and Windows (`;`) cases deterministically.
    #[test]
    fn evaluate_pythonpath_with_separator_handles_unix_colon() {
        let work = tempdir().expect("work");
        let (root, rejected) = evaluate_pythonpath_with_separator(
            work.path(),
            Some("/external/path:/another/external"),
            ':',
        );
        assert!(root.is_some());
        assert!(
            rejected.is_some(),
            "POSIX colon-separated external PYTHONPATH must flag the first external component"
        );
    }

    #[test]
    fn evaluate_pythonpath_with_separator_does_not_split_drive_letter() {
        let work = tempdir().expect("work");
        // Two-component Windows-shape PYTHONPATH separated by `;`. With the
        // CB-010 fix, the parser splits on `;` (NOT `:`), so the drive
        // letter `C:` survives in the first component and the external
        // POSIX path `/external/repo` is reachable as the second component
        // and gets flagged. Pre-fix, the legacy parser split on `:` and
        // would have produced spurious bare-letter components.
        let (root, rejected) = evaluate_pythonpath_with_separator(
            work.path(),
            Some("C:\\projects\\inside;/external/repo"),
            ';',
        );
        assert!(root.is_some());
        assert!(
            rejected.is_some(),
            "Windows ';'-separated external POSIX component must still be flagged when the parser does not split on `:`"
        );
        let raw = rejected.unwrap();
        // The flagged raw must be one of the real components — NOT a
        // single bare letter caused by erroneous `:` splitting.
        assert!(
            raw.len() > 1,
            "rejected raw must be a real path component, not a single-letter remnant of `:` splitting; got {raw:?}"
        );
    }

    #[test]
    fn evaluate_pythonpath_with_separator_handles_unix_trailing_colon() {
        let work = tempdir().expect("work");
        // Trailing `:` produces an empty component; it MUST be skipped without
        // either flagging it as external or panicking.
        let (root, rejected) =
            evaluate_pythonpath_with_separator(work.path(), Some("src:lib:"), ':');
        assert!(root.is_some());
        assert!(rejected.is_none());
    }

    /// Phase B Task 7.2: post-execution import detection must signal an
    /// external module path while ignoring known-safe toolchain caches.
    #[test]
    fn detect_external_imports_picks_up_python_import_error() {
        let work = tempdir().expect("work");
        // Make the external path appear in an ImportError-style line. The
        // canonicalize fallback path keeps the comparison defensive even when
        // the path does not physically exist.
        let stderr = "ImportError: cannot import name 'X' from '/external/repo/app/main.py'";
        let detected = detect_external_imports_in_output(work.path(), "", stderr);
        assert!(
            !detected.entries.is_empty(),
            "external /external/repo/.. path must be detected; got {detected:?}"
        );
    }

    #[test]
    fn detect_external_imports_picks_up_import_error_parenthesized_path() {
        let work = tempdir().expect("work");
        let stderr =
            "E   ImportError: cannot import name 'X' from 'app.main' (/external/repo/app/main.py)";
        let detected = detect_external_imports_in_output(work.path(), "", stderr);
        assert!(
            !detected.entries.is_empty(),
            "parenthesized ImportError path must be detected; got {detected:?}"
        );
    }

    #[test]
    fn python_package_marker_candidates_detects_local_namespace_package() {
        let work = tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("app")).expect("app dir");
        std::fs::write(work.path().join("app/main.py"), "x = 1\n").expect("main");
        let stderr =
            "E   ImportError: cannot import name 'X' from 'app.main' (/external/repo/app/main.py)";
        let candidates =
            python_package_marker_candidates_for_external_import(work.path(), "", stderr);
        assert_eq!(candidates, vec!["app/__init__.py"]);
    }

    #[test]
    fn python_package_marker_candidates_for_owned_test_imports_detects_app_import() {
        let work = tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("app")).expect("app dir");
        std::fs::write(work.path().join("app/main.py"), "x = 1\n").expect("main");
        std::fs::create_dir_all(work.path().join("tests")).expect("tests dir");
        std::fs::write(
            work.path().join("tests/test_main.py"),
            "from app.main import app\n",
        )
        .expect("test");
        let candidates = python_package_marker_candidates_for_owned_test_imports(
            work.path(),
            &["tests/test_main.py".to_string()],
        );
        assert_eq!(candidates, vec!["app/__init__.py"]);
    }

    #[test]
    fn python_package_marker_candidates_for_owned_test_imports_detects_nested_packages() {
        let work = tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("src/app")).expect("src app dir");
        std::fs::write(work.path().join("src/app/main.py"), "x = 1\n").expect("main");
        std::fs::create_dir_all(work.path().join("tests")).expect("tests dir");
        std::fs::write(
            work.path().join("tests/test_main.py"),
            "import src.app.main as subject\n",
        )
        .expect("test");
        let candidates = python_package_marker_candidates_for_owned_test_imports(
            work.path(),
            &["tests/test_main.py".to_string()],
        );
        assert_eq!(candidates, vec!["src/__init__.py", "src/app/__init__.py"]);
    }

    #[test]
    fn python_package_marker_candidates_skip_tests_and_existing_init() {
        let work = tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("tests")).expect("tests dir");
        std::fs::write(work.path().join("tests/test_main.py"), "x = 1\n").expect("test");
        std::fs::create_dir_all(work.path().join("app")).expect("app dir");
        std::fs::write(work.path().join("app/main.py"), "x = 1\n").expect("main");
        std::fs::write(work.path().join("app/__init__.py"), "").expect("init");
        let stderr = "ImportError: cannot import name 'X' from 'tests.test_main' (/external/tests/test_main.py)\n\
             ImportError: cannot import name 'Y' from 'app.main' (/external/app/main.py)";
        let candidates =
            python_package_marker_candidates_for_external_import(work.path(), "", stderr);
        assert!(
            candidates.is_empty(),
            "tests package and already-initialized app package must be skipped"
        );
    }

    #[test]
    fn detect_external_imports_ignores_cargo_home_cache() {
        let work = tempdir().expect("work");
        // CB-011: the safe-path filter requires the substring to actually be
        // anchored to the resolved HOME. Use the live HOME (always set on
        // POSIX and on macOS CI) so the test exercises the production code
        // path; if HOME is unset, fall back to a literal `/root` prefix.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let stderr = format!(
            "error[E0432]: unresolved import `serde` (looked at {home}/.cargo/registry/src/...)"
        );
        let detected = detect_external_imports_in_output(work.path(), "", &stderr);
        assert!(
            detected.entries.is_empty(),
            "~/.cargo cache paths must NOT be flagged; got {detected:?}"
        );
    }

    #[test]
    fn detect_external_imports_ignores_pyenv_versions() {
        let work = tempdir().expect("work");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let stderr = format!(
            "ModuleNotFoundError: No module named 'foo' (sys.path=['{home}/.pyenv/versions/3.11.0/lib/python3.11/site-packages'])"
        );
        let detected = detect_external_imports_in_output(work.path(), "", &stderr);
        assert!(
            detected.entries.is_empty(),
            "~/.pyenv/versions paths must NOT be flagged; got {detected:?}"
        );
    }

    /// CB-011 (Codex iteration-5 medium): a substring filter on `/lib/python`
    /// is too permissive — an external workspace `/tmp/other/lib/python/...`
    /// must be flagged because it is NOT under a known cache root.
    #[test]
    fn detect_external_imports_flags_external_lib_python_outside_cache() {
        let work = tempdir().expect("work");
        let stderr = "ImportError: cannot import name 'X' from '/tmp/other/lib/python/app/main.py'";
        let detected = detect_external_imports_in_output(work.path(), "", stderr);
        assert!(
            !detected.entries.is_empty(),
            "external workspace /tmp/other/lib/python/... MUST be detected; got {detected:?}"
        );
    }

    /// CB-011 (Codex iteration-5 medium): an arbitrary `target` component
    /// outside of CARGO_TARGET_DIR / work_root MUST be detected; only
    /// `<work_root>/target/...` or `$CARGO_TARGET_DIR/...` should be safe.
    #[test]
    fn detect_external_imports_flags_external_target_outside_work_root() {
        let work = tempdir().expect("work");
        let stderr = "ImportError: cannot import 'X' from '/tmp/target/app/main.py'";
        let detected = detect_external_imports_in_output(work.path(), "", stderr);
        assert!(
            !detected.entries.is_empty(),
            "external `target` component outside CARGO_TARGET_DIR / work_root MUST be detected; got {detected:?}"
        );
    }

    #[test]
    fn detect_external_imports_caps_detected_count() {
        let work = tempdir().expect("work");
        // Generate cap + 3 distinct external paths in stderr.
        let mut stderr = String::new();
        for i in 0..(EXTERNAL_IMPORT_DETECTED_CAP + 3) {
            stderr.push_str(&format!(
                "ImportError: cannot import X from '/external/path_{i}/mod.py'\n"
            ));
        }
        let detected = detect_external_imports_in_output(work.path(), "", &stderr);
        assert_eq!(detected.entries.len(), EXTERNAL_IMPORT_DETECTED_CAP);
    }

    /// CB-009 (Codex iteration-5 medium): `DetectedExternalImports.total_count`
    /// MUST reflect the pre-cap total even when the per-emit cap drops
    /// excess entries. `truncated` reflects total > cap.
    #[test]
    fn detect_external_imports_total_count_preserves_pre_cap_count() {
        let work = tempdir().expect("work");
        let excess = EXTERNAL_IMPORT_DETECTED_CAP + 3;
        let mut stderr = String::new();
        for i in 0..excess {
            stderr.push_str(&format!(
                "ImportError: cannot import X from '/external/path_{i}/mod.py'\n"
            ));
        }
        let detected = detect_external_imports_in_output(work.path(), "", &stderr);
        assert_eq!(detected.entries.len(), EXTERNAL_IMPORT_DETECTED_CAP);
        assert_eq!(detected.total_count, excess);
        assert!(detected.truncated);
    }

    /// CB-009 (Codex iteration-5 medium): exactly-at-cap input MUST NOT
    /// set `truncated=true`. truncated only flips when total_count > cap.
    #[test]
    fn detect_external_imports_at_cap_is_not_truncated() {
        let work = tempdir().expect("work");
        let mut stderr = String::new();
        for i in 0..EXTERNAL_IMPORT_DETECTED_CAP {
            stderr.push_str(&format!(
                "ImportError: cannot import X from '/external/path_{i}/mod.py'\n"
            ));
        }
        let detected = detect_external_imports_in_output(work.path(), "", &stderr);
        assert_eq!(detected.entries.len(), EXTERNAL_IMPORT_DETECTED_CAP);
        assert_eq!(detected.total_count, EXTERNAL_IMPORT_DETECTED_CAP);
        assert!(
            !detected.truncated,
            "exactly-at-cap input MUST NOT set truncated=true"
        );
    }

    // ----------------------------------------------------------------
    // Issue #661 iteration-4 Task 2.5: VerifierInvokedSnapshot
    // - runner: RunnerKind (string でない、DR4-004 security gate)
    // - bound_artifacts: Vec<PathHashHex> (raw path 漏洩を型でブロック)
    // - bound_artifacts_truncated: true when len > VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP
    // - bound_test_artifacts_count: pre-cap full count
    // ----------------------------------------------------------------

    #[test]
    fn verifier_invoked_snapshot_maps_cargo_runner_to_runnerkind_cargo() {
        let work = tempdir().expect("work");
        let owned = vec!["tests/test_a.rs".to_string()];
        let command = VerifierCommand::from_cargo_test(Vec::new(), &owned).expect("cargo");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan)
            .expect("cargo runner must map to RunnerKind::Cargo");
        assert_eq!(snapshot.runner, RunnerKind::Cargo);
        assert_eq!(snapshot.runner.as_str(), "cargo");
        assert_eq!(snapshot.bound_test_artifacts_count, 1);
        assert!(!snapshot.bound_artifacts_truncated);
        assert_eq!(snapshot.bound_artifacts.len(), 1);
    }

    #[test]
    fn verifier_invoked_snapshot_maps_python3_runner_to_runnerkind_python3() {
        let work = tempdir().expect("work");
        let owned = vec!["app/tests/test_a.py".to_string()];
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 stdlib pytest");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan)
            .expect("python3 runner must map to RunnerKind::Python3");
        assert_eq!(snapshot.runner, RunnerKind::Python3);
        assert_eq!(snapshot.runner.as_str(), "python3");
        assert_eq!(snapshot.bound_test_artifacts_count, 1);
        assert!(!snapshot.bound_artifacts_truncated);
        // bound_artifacts must reuse the PathHashHex SSOT (`mask_secrets` +
        // `crate::logging::stable_path_hash`).
        let expected = crate::logging::stable_path_hash(&crate::session::feedback::mask_secrets(
            "app/tests/test_a.py",
        ));
        assert_eq!(snapshot.bound_artifacts[0].as_str(), expected);
    }

    #[test]
    fn verifier_invoked_snapshot_maps_npm_runner_to_runnerkind_npm() {
        let work = tempdir().expect("work");
        let owned = vec!["tests/index.test.js".to_string()];
        let command = VerifierCommand::from_npm_test(&owned).expect("npm test");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan)
            .expect("npm runner must map to RunnerKind::Npm");
        assert_eq!(snapshot.runner, RunnerKind::Npm);
        assert_eq!(snapshot.runner.as_str(), "npm");
        assert_eq!(snapshot.bound_test_artifacts_count, 1);
    }

    #[test]
    fn verifier_invoked_snapshot_returns_none_for_unknown_runner() {
        // DR4-004 security gate: an unknown runner string (e.g. `pytest`
        // bare — production never produces this, but unit tests do) must
        // NOT yield a VerifierInvokedSnapshot. The event emit site can
        // then skip emitting rather than synthesize a label that bypasses
        // the closed `RunnerKind` set.
        let work = tempdir().expect("work");
        let owned = vec!["app/tests/test_a.py".to_string()];
        let command = VerifierCommand::from_pytest(Vec::new(), &owned).expect("pytest");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan);
        assert!(
            snapshot.is_none(),
            "bare `pytest` runner has no RunnerKind variant — snapshot must be None"
        );
    }

    #[test]
    fn verifier_invoked_snapshot_truncates_bound_artifacts_above_cap() {
        // Generate `VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP + 3` owned test
        // paths and assert bound_artifacts is capped while
        // bound_test_artifacts_count keeps the full pre-cap count.
        let work = tempdir().expect("work");
        let owned: Vec<String> = (0..(VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP + 3))
            .map(|i| format!("app/tests/test_{i}.py"))
            .collect();
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 stdlib pytest");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan)
            .expect("python3 runner");
        assert_eq!(snapshot.bound_test_artifacts_count, owned.len());
        assert_eq!(
            snapshot.bound_artifacts.len(),
            VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP,
            "bound_artifacts must be capped at VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP entries"
        );
        assert!(
            snapshot.bound_artifacts_truncated,
            "bound_artifacts_truncated must be true when count > cap"
        );
    }

    #[test]
    fn verifier_invoked_snapshot_phase_b_env_summary_observable_values() {
        let work = tempdir().expect("work");
        let owned = vec!["app/tests/test_a.py".to_string()];
        let command =
            VerifierCommand::from_python3_pytest_stdlib(&owned).expect("python3 stdlib pytest");
        let env_plan = build_hermetic_env_plan(work.path(), &[]);
        let snapshot = VerifierInvokedSnapshot::from_command_and_env(&command, &env_plan)
            .expect("python3 runner");
        // Phase B invariant: env_clear + allowlist re-inject populates
        // `allowlist_keys` with the real filter_env_for_tester outcome.
        // PATH should always be present.
        assert!(
            snapshot.env_summary.allowlist_keys.contains(&"PATH"),
            "Phase B: snapshot.env_summary.allowlist_keys must include PATH"
        );
        assert!(snapshot.env_summary.cwd_inside_work_root);
    }

    #[test]
    fn runner_kind_from_runner_str_maps_known_runners() {
        assert_eq!(
            RunnerKind::from_runner_str("cargo"),
            Some(RunnerKind::Cargo)
        );
        assert_eq!(
            RunnerKind::from_runner_str("python3"),
            Some(RunnerKind::Python3)
        );
        assert_eq!(RunnerKind::from_runner_str("npm"), Some(RunnerKind::Npm));
        // Unknown / future runners MUST return None so they cannot reach
        // the `agent.verifier.invoked` payload without an explicit
        // `RunnerKind` variant + constructor + validator (DR4-004).
        assert!(RunnerKind::from_runner_str("pytest").is_none());
        assert!(RunnerKind::from_runner_str("").is_none());
    }
}
