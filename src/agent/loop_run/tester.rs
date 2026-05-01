//! Tester Skill v1 (Issue #459).
//!
//! `AutoTestRunner::detect()` が明示的な test verifier を返さない repo に対し、
//! main turn の追加 1 LLM call で smoke test を生成し session-scoped
//! `tmp-tests/files/` 配下に保存し、固定テンプレートの Bash で実行して
//! `FeedbackFrame` を記録する pure-function 群。
//!
//! Reminder Sidecar (#452 / `reminder.rs`) と対称に、LLM 呼び出しと Bash 実行を
//! closure DI で受けてユニットテストから fixture で全経路を駆動できる。
//!

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::auto_test::{
    self, AutoTestPlan, AutoTestRunner, has_cargo_manifest, has_python_surface,
    package_json_has_test_script, shell_quote,
};
use crate::session::feedback::{
    FeedbackFrame, FeedbackFrameDraft, FeedbackKind, build_feedback_frame,
};
use crate::tools::bash::BashExecutionOutcome;

// ============================================================================
// 1. 公開定数
// ============================================================================

/// smoke test 実行に対する明示 timeout (DR1-003 / DR2-003).
pub(super) const TESTER_SMOKE_TIMEOUT_SECS: u64 = 30;

/// Tester LLM reply 受け取り上限。これを超えれば malformed として全体 reject
/// (DR2-002). `pub` so the integration suite in
/// `tests/tester_skill_smoke.rs` can reference the exact cap boundary.
pub const MAX_TESTER_LLM_REPLY_BYTES: usize = 64 * 1024;

/// 1 turn で生成可能な smoke test ファイル数 (DR1-004 / DR2-002). `pub` so
/// integration tests can reference the cap directly.
pub const MAX_GENERATED_TESTS_PER_TURN: usize = 1;

/// `agent.tester.*` payload に raw secret / 巨大 LLM reply を残さないための
/// log truncation cap. Reminder Sidecar の `REMINDER_LOG_CAP` (20_000) と対称.
pub(super) const TESTER_LOG_CAP: usize = 20_000;

// ============================================================================
// 2. 型定義
// ============================================================================

/// Tester 起動候補の検出結果. v1 では `Unknown` variant を持たない (DR1-008):
/// 該当 stack 無しは `detect()` が `None` を返すことで表現する. `pub` for
/// integration tests; production code paths continue to use `super::tester`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TesterCandidate {
    /// Cargo.toml はあるが `tests/` も `[[test]]` も無い
    Rust {
        manifest_path: PathBuf,
        package_name: Option<String>,
    },
    /// package.json はあるが `"test"` script が無い
    Node {
        manifest_path: PathBuf,
        has_build_script: bool,
    },
    /// Python surface (.py / pyproject.toml) はあるが pytest config / tests/ が無い
    Python { surface_files: Vec<PathBuf> },
}

/// approval 関門 enum (write/run/promote 3 関門のうち run の関門, DR1-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// `auto_approve = true` (CI / `--yes`) → 即 OK
    Auto,
    /// 既定: stdin から y/n を受け取る
    Interactive,
    /// Plan mode 等で run の関門を踏ませない (= 即 deny)
    Forbidden,
}

/// Tester run の入力束.
#[derive(Debug, Clone)]
pub struct TesterRun<'a> {
    pub work_root: &'a Path,
    pub tmp_tests_root: &'a Path,
    pub tester_runs_root: &'a Path,
    pub approval_mode: ApprovalMode,
    pub plan_mode: bool,
    pub no_tester_env: bool,
    pub session_id: std::borrow::Cow<'a, str>,
}

/// caller (turn.rs) が分岐に使う 3 値 enum (DR1-007).
#[derive(Debug)]
pub enum TesterOutcome {
    /// FeedbackFrame を生成. `record_feedback_if_unset` に渡す.
    Recorded(FeedbackFrame),
    /// candidate なし / disable / Plan mode / per-turn cap 等で起動を見送り.
    NotInvoked(NotInvokedReason),
    /// candidate あったが LLM 出力 malformed / approval 拒否 / harness 構築失敗 等.
    Aborted(AbortReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotInvokedReason {
    NoCandidate,
    Disabled,
    PlanMode,
    PerTurnCapHit,
}

impl NotInvokedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotInvokedReason::NoCandidate => "no_candidate",
            NotInvokedReason::Disabled => "disabled",
            NotInvokedReason::PlanMode => "plan_mode",
            NotInvokedReason::PerTurnCapHit => "per_turn_cap_hit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    LlmCall(String),
    LlmMalformed(String),
    PathConfinementViolation(String),
    ApprovalDenied,
    HarnessBuildFailure(String),
    BashFailure(String),
    TmpTestWriteFailure(String),
    /// CB-001 (Issue #459): smoke runner exceeded `TESTER_SMOKE_TIMEOUT_SECS`.
    /// Treated as Aborted so a timed-out smoke is never recorded as TestPass /
    /// BuildPass even when `exit_code` defaulted to 0.
    SmokeTimeout(String),
    /// CB-001 (Issue #459): smoke runner was cancelled / interrupted before
    /// the child reported an exit code.
    SmokeInterrupted(String),
    /// CB-001 (Issue #459): smoke command was rejected by the dangerous /
    /// blocked-snippet filter. Distinct from `BashFailure` so logs can
    /// surface the safety reason without re-parsing.
    SmokeBlocked(String),
}

impl AbortReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            AbortReason::LlmCall(_) => "llm_call",
            AbortReason::LlmMalformed(_) => "llm_malformed",
            AbortReason::PathConfinementViolation(_) => "path_confinement_violation",
            AbortReason::ApprovalDenied => "approval_denied",
            AbortReason::HarnessBuildFailure(_) => "harness_build_failure",
            AbortReason::BashFailure(_) => "bash_failure",
            AbortReason::TmpTestWriteFailure(_) => "tmp_test_write_failure",
            AbortReason::SmokeTimeout(_) => "smoke_timeout",
            AbortReason::SmokeInterrupted(_) => "smoke_interrupted",
            AbortReason::SmokeBlocked(_) => "smoke_blocked",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            AbortReason::LlmCall(s)
            | AbortReason::LlmMalformed(s)
            | AbortReason::PathConfinementViolation(s)
            | AbortReason::HarnessBuildFailure(s)
            | AbortReason::BashFailure(s)
            | AbortReason::TmpTestWriteFailure(s)
            | AbortReason::SmokeTimeout(s)
            | AbortReason::SmokeInterrupted(s)
            | AbortReason::SmokeBlocked(s) => Some(s.as_str()),
            AbortReason::ApprovalDenied => None,
        }
    }
}

/// 単一 LLM reply の正規化形 (`validate_llm_reply` 出力).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TesterReply {
    pub test_files: Vec<TesterReplyFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct TesterReplyFile {
    pub relative_path: String,
    pub content: String,
}

/// classify_tester_outcome の戻り値. caller は `to_feedback_kind()` で
/// `FeedbackKind` に落とす.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TesterClass {
    TestPass,
    TestFailure,
    BuildPass,
    CompileError,
}

impl TesterClass {
    pub(super) fn to_feedback_kind(self) -> FeedbackKind {
        match self {
            TesterClass::TestPass => FeedbackKind::TestPass,
            TesterClass::TestFailure => FeedbackKind::TestFailure,
            TesterClass::BuildPass => FeedbackKind::BuildPass,
            TesterClass::CompileError => FeedbackKind::CompileError,
        }
    }
}

// ============================================================================
// 3. env 評価 helper (DR2-017)
// ============================================================================

/// `ANVIL_NO_TESTER` が non-empty で set されているとき disable.
/// Reminder Sidecar の `reminder_disabled` と完全対称.
pub fn tester_disabled<F>(get_env: F) -> bool
where
    F: FnOnce(&str) -> Option<String>,
{
    get_env("ANVIL_NO_TESTER")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Pure-function gate that mirrors the early-out checks performed in
/// `turn.rs::try_invoke_tester` BEFORE dispatch. Returns the canonical
/// `NotInvokedReason` if the Tester must be skipped, or `None` if dispatch
/// may proceed. Exposed so integration tests can validate per-turn cap /
/// Plan-mode / `ANVIL_NO_TESTER` ordering without needing to spin up an
/// `Agent` (DR1-004 / DR1-012).
///
/// Order of evaluation matches `try_invoke_tester` to keep the production
/// code path the single source of truth: per-turn cap → Plan mode →
/// `ANVIL_NO_TESTER`. Caller is responsible for `NoCandidate` since stack
/// detection lives outside the gate.
pub fn check_invocation_gate(
    already_called: bool,
    plan_mode: bool,
    disabled_by_env: bool,
) -> Option<NotInvokedReason> {
    if already_called {
        return Some(NotInvokedReason::PerTurnCapHit);
    }
    if plan_mode {
        return Some(NotInvokedReason::PlanMode);
    }
    if disabled_by_env {
        return Some(NotInvokedReason::Disabled);
    }
    None
}

// ============================================================================
// 4. TesterCandidate::detect (Task 1b.2)
// ============================================================================

impl TesterCandidate {
    /// `AutoTestRunner::detect` が explicit test verifier を返した場合は
    /// `None` を返す (= 既存 auto_test 経路を優先する).
    /// build-only verifier (`cargo test` from Cargo.toml-only repo,
    /// `npm run build`, `python3 -m py_compile`) は Tester 起動を妨げない
    /// (DR3-001).
    pub fn detect(work_root: &Path, changed_files: &[String]) -> Option<Self> {
        if let Some(plan) = AutoTestRunner::detect(work_root, changed_files)
            && is_explicit_test_verifier(&plan, work_root)
        {
            return None;
        }
        if has_cargo_manifest(work_root) {
            let manifest_path = work_root.join("Cargo.toml");
            let package_name = read_cargo_package_name(&manifest_path);
            return Some(TesterCandidate::Rust {
                manifest_path,
                package_name,
            });
        }
        if work_root.join("package.json").is_file() && !package_json_has_test_script(work_root) {
            let manifest_path = work_root.join("package.json");
            let has_build_script = std::fs::read_to_string(&manifest_path)
                .map(|s| s.contains("\"build\""))
                .unwrap_or(false);
            return Some(TesterCandidate::Node {
                manifest_path,
                has_build_script,
            });
        }
        if has_python_surface(work_root, changed_files) {
            // Tester は pytest / tests/ がない Python repo を狙う.
            let has_pytest =
                work_root.join("pytest.ini").is_file() || work_root.join("tests").is_dir();
            if !has_pytest {
                let surface_files = python_surface_files(work_root, changed_files);
                if !surface_files.is_empty() {
                    return Some(TesterCandidate::Python { surface_files });
                }
            }
        }
        None
    }
}

/// `AutoTestPlan` が explicit test verifier (npm test / pytest / tests/ 配下)
/// を表すかどうかの薄い分類器 (DR3-001). build-only verifier (cargo test from
/// Cargo.toml-only repo, npm run build, py_compile) は false を返す.
fn is_explicit_test_verifier(plan: &AutoTestPlan, work_root: &Path) -> bool {
    let cmd = plan.command.trim().to_ascii_lowercase();
    if cmd.starts_with("npm test")
        || cmd.starts_with("yarn test")
        || cmd.starts_with("pnpm test")
        || cmd.starts_with("python3 -m pytest")
        || cmd.starts_with("python -m pytest")
        || cmd.starts_with("pytest")
    {
        return true;
    }
    // `cargo test` は Cargo.toml-only repo でも返るので tests/ が実在するか
    // [[test]] entry が見える時のみ explicit と扱う.
    if cmd.starts_with("cargo test") && work_root.join("tests").is_dir() {
        return true;
    }
    false
}

fn read_cargo_package_name(manifest_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(manifest_path).ok()?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let after_eq = rest.trim_start().strip_prefix('=')?.trim();
            // strip surrounding quotes
            let stripped = after_eq.trim_matches(|c: char| c == '"' || c == '\'');
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

fn python_surface_files(work_root: &Path, changed_files: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = changed_files
        .iter()
        .filter(|p| p.ends_with(".py") && !p.starts_with("tests/") && !p.starts_with("test_"))
        .map(PathBuf::from)
        .collect();
    if out.is_empty()
        && let Some(first) = auto_test::first_python_script(changed_files)
    {
        out.push(first);
    }
    if out.is_empty() && work_root.join("pyproject.toml").is_file() {
        out.push(PathBuf::from("pyproject.toml"));
    }
    out
}

// ============================================================================
// 5. LLM reply parser + path validation (Task 1b.3)
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct RawTesterReply {
    #[serde(default)]
    test_files: Vec<TesterReplyFile>,
}

/// Validate raw LLM reply text into a `TesterReply`. Defensive: empty / >64KiB
/// / `<think>` のみ / >1 ファイル / tmp-tests 外 path を全体 reject (DR2-002).
pub(super) fn validate_llm_reply(reply: &str) -> Result<TesterReply, AbortReason> {
    if reply.is_empty() {
        return Err(AbortReason::LlmMalformed("empty reply".to_string()));
    }
    if reply.len() > MAX_TESTER_LLM_REPLY_BYTES {
        return Err(AbortReason::LlmMalformed(format!(
            "reply exceeded {MAX_TESTER_LLM_REPLY_BYTES} bytes"
        )));
    }
    let stripped = crate::ollama::xml_fallback::strip_think_tags(reply);
    let stripped = stripped.trim();
    if stripped.is_empty() {
        return Err(AbortReason::LlmMalformed(
            "reply is empty after <think> strip".to_string(),
        ));
    }
    let json_slice = super::lifecycle::extract_first_json_object(stripped)
        .ok_or_else(|| AbortReason::LlmMalformed("no JSON object found".to_string()))?;
    let raw: RawTesterReply = serde_json::from_str(json_slice)
        .map_err(|e| AbortReason::LlmMalformed(format!("JSON parse error: {e}")))?;
    if raw.test_files.is_empty() {
        return Err(AbortReason::LlmMalformed("test_files is empty".to_string()));
    }
    if raw.test_files.len() > MAX_GENERATED_TESTS_PER_TURN {
        return Err(AbortReason::LlmMalformed(format!(
            "test_files len {} exceeds cap {MAX_GENERATED_TESTS_PER_TURN}",
            raw.test_files.len()
        )));
    }
    for entry in &raw.test_files {
        validate_tester_generated_rel_path(&entry.relative_path)?;
    }
    Ok(TesterReply {
        test_files: raw.test_files,
    })
}

/// Reject paths that escape the `tmp-tests/files/` namespace or contain
/// ASCII control / newline / CR / TAB chars (DR4-004). Subset of
/// `validate_tmp_test_relative_path` plus extra control-char filter.
pub(super) fn validate_tester_generated_rel_path(rel: &str) -> Result<(), AbortReason> {
    if rel.is_empty() {
        return Err(AbortReason::PathConfinementViolation(
            "relative_path is empty".to_string(),
        ));
    }
    for ch in rel.chars() {
        if ch.is_ascii_control() {
            return Err(AbortReason::PathConfinementViolation(format!(
                "relative_path contains ASCII control char: {:?}",
                rel
            )));
        }
    }
    if let Err(err) = crate::session::tmp_tests::validate_tmp_test_relative_path(rel) {
        return Err(AbortReason::PathConfinementViolation(err));
    }
    Ok(())
}

// ============================================================================
// 6. shell template builder + classify_tester_outcome (Task 1b.4)
// ============================================================================

/// run_id allowlist (`run_[a-z0-9_-]{16,64}`). NUL / `/` / `..` / path
/// separator は弾く (DR4-003).
pub(super) fn validate_run_id(run_id: &str) -> Result<(), AbortReason> {
    if run_id.is_empty() {
        return Err(AbortReason::HarnessBuildFailure(
            "run_id is empty".to_string(),
        ));
    }
    let body = run_id.strip_prefix("run_").ok_or_else(|| {
        AbortReason::HarnessBuildFailure(format!("run_id must start with 'run_': {run_id}"))
    })?;
    if body.len() < 16 || body.len() > 64 {
        return Err(AbortReason::HarnessBuildFailure(format!(
            "run_id body must be 16..=64 bytes: {run_id}"
        )));
    }
    for ch in body.chars() {
        let allowed = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-';
        if !allowed {
            return Err(AbortReason::HarnessBuildFailure(format!(
                "run_id contains disallowed char '{ch}': {run_id}"
            )));
        }
    }
    Ok(())
}

/// Derive a deterministic, prefix-allowlisted `run_<sha-prefix>` run id from
/// the smoke content + creation timestamp (DR4-003).
pub(super) fn derive_run_id(content: &[u8], created_at: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.update(b"\x00");
    hasher.update(created_at.to_be_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    format!("run_{}", &hex[..24])
}

/// Build the shell command vector for a given stack. All paths are absolute
/// and `shell_quote`-escaped. DR4-003 / DR4-004: caller MUST have already
/// validated `run_id` and `rel_test`.
pub(super) fn build_command(
    stack: &TesterCandidate,
    run_id: &str,
    tmp_tests_root: &Path,
    tester_runs_root: &Path,
    rel_test: &Path,
) -> Vec<String> {
    match stack {
        TesterCandidate::Rust { .. } => {
            // CB-005 (Issue #459): pin `CARGO_TARGET_DIR` inside the per-run
            // harness dir so cargo never writes build artefacts back into the
            // workspace `target/`. The `env KEY=VAL cargo ...` form preserves
            // the existing sanitized-env policy applied by the bash runner
            // (CB-003) — cargo only sees `CARGO_TARGET_DIR` plus whatever
            // `bash::run_with_outcome_with_options` chose to forward.
            let manifest = tester_runs_root.join(run_id).join("Cargo.toml");
            let target_dir = tester_runs_root.join(run_id).join("target");
            vec![
                "env".to_string(),
                format!("CARGO_TARGET_DIR={}", shell_quote(&target_dir)),
                "cargo".to_string(),
                "test".to_string(),
                "--manifest-path".to_string(),
                shell_quote(&manifest),
            ]
        }
        TesterCandidate::Node { .. } => {
            let abs_test = tmp_tests_root.join("files").join(rel_test);
            vec![
                "node".to_string(),
                "--check".to_string(),
                shell_quote(&abs_test),
            ]
        }
        TesterCandidate::Python { .. } => {
            let abs_test = tmp_tests_root.join("files").join(rel_test);
            vec![
                "python3".to_string(),
                "-m".to_string(),
                "py_compile".to_string(),
                shell_quote(&abs_test),
            ]
        }
    }
}

/// Thin classifier — Rust では `cargo test` 出力ベース、Node は exit 0 →
/// BuildPass (`node --check` は build-only verifier)、Python は exit 0 →
/// TestPass (`py_compile` は build 系扱いせず TestPass に揃える、DR1-010).
///
/// CB-001 (Issue #459): operates directly on `BashExecutionOutcome` so the
/// `timed_out` / `interrupted` / `blocked_reason` flags map onto explicit
/// `AbortReason` variants instead of a synthesised `ExitStatus(0)` that would
/// mis-classify a hung smoke run as `TestPass` / `BuildPass`. `exit_code =
/// None` without those flags is treated as a hard abort too — there is no
/// safe default exit status for the success branch.
pub(super) fn classify_tester_outcome(
    stack: &TesterCandidate,
    outcome: &BashExecutionOutcome,
) -> Result<TesterClass, AbortReason> {
    if let Some(reason) = &outcome.blocked_reason {
        return Err(AbortReason::SmokeBlocked(reason.clone()));
    }
    if outcome.timed_out {
        return Err(AbortReason::SmokeTimeout(format!(
            "smoke test exceeded {TESTER_SMOKE_TIMEOUT_SECS}s timeout"
        )));
    }
    if outcome.interrupted {
        return Err(AbortReason::SmokeInterrupted(
            "smoke test interrupted before exit".to_string(),
        ));
    }
    let exit_code = match outcome.exit_code {
        Some(c) => c,
        None => {
            return Err(AbortReason::BashFailure(
                "smoke test produced no exit code".to_string(),
            ));
        }
    };
    let success = exit_code == 0;
    let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
    Ok(match stack {
        TesterCandidate::Rust { .. } => {
            if success {
                TesterClass::TestPass
            } else {
                let lower = combined.to_ascii_lowercase();
                if lower.contains("error[")
                    || lower.contains("could not compile")
                    || lower.contains("syntax error")
                {
                    TesterClass::CompileError
                } else {
                    TesterClass::TestFailure
                }
            }
        }
        TesterCandidate::Node { .. } => {
            if success {
                TesterClass::BuildPass
            } else {
                TesterClass::CompileError
            }
        }
        TesterCandidate::Python { .. } => {
            if success {
                TesterClass::TestPass
            } else {
                TesterClass::TestFailure
            }
        }
    })
}

// ============================================================================
// 7. approval prompt + execute_smoke_test (Task 1b.5)
// ============================================================================

/// Prompt the user for run-gate approval. `Auto` → 即 OK / `Forbidden` →
/// 即 deny / `Interactive` → stdin から y/n. write/run/promote の 3 関門と
/// 対称の書式 (DR1-013).
pub(super) fn prompt_for_approval<W: std::io::Write, R: std::io::BufRead>(
    approval_mode: ApprovalMode,
    command: &[String],
    stdout: &mut W,
    stdin: &mut R,
) -> Result<(), AbortReason> {
    match approval_mode {
        ApprovalMode::Auto => Ok(()),
        ApprovalMode::Forbidden => Err(AbortReason::ApprovalDenied),
        ApprovalMode::Interactive => {
            let cmd_label = command.join(" ");
            writeln!(stdout, "[Tester] Run smoke test ({cmd_label})? (y/N)")
                .map_err(|e| AbortReason::BashFailure(format!("approval write: {e}")))?;
            stdout
                .flush()
                .map_err(|e| AbortReason::BashFailure(format!("approval flush: {e}")))?;
            let mut line = String::new();
            stdin
                .read_line(&mut line)
                .map_err(|e| AbortReason::BashFailure(format!("approval read: {e}")))?;
            let ans = line.trim().to_ascii_lowercase();
            if ans == "y" || ans == "yes" {
                Ok(())
            } else {
                Err(AbortReason::ApprovalDenied)
            }
        }
    }
}

/// Execute the smoke test command via a closure-injected bash runner.
/// Production wires this to `bash::run_with_outcome` with `explicit_timeout =
/// Some(TESTER_SMOKE_TIMEOUT_SECS)` and a sanitized env. CI uses a fake.
pub(super) fn execute_smoke_test<F>(
    command: &[String],
    cwd: &Path,
    run_bash: F,
) -> Result<BashExecutionOutcome, AbortReason>
where
    F: FnOnce(&str, &Path, Option<Duration>) -> Result<BashExecutionOutcome, String>,
{
    let cmd_str = command.join(" ");
    let timeout = Duration::from_secs(TESTER_SMOKE_TIMEOUT_SECS);
    run_bash(&cmd_str, cwd, Some(timeout)).map_err(AbortReason::BashFailure)
}

// ============================================================================
// 8. orchestration: run_tester_with_strategy (Task 1b.6)
// ============================================================================

/// Inputs for the LLM call closure (built by `run_tester_with_strategy`).
///
/// `stack_label` lets the closure tag log payloads with the detected stack
/// (`rust` / `node` / `python`) without re-matching the candidate. `body` is
/// the LLM-facing prompt body itself (≤ 2 KiB, § 10 設計判断 #2).
#[derive(Debug, Clone)]
pub struct TesterPrompt {
    pub stack_label: &'static str,
    pub body: String,
}

impl TesterPrompt {
    pub fn stack_label(&self) -> &'static str {
        self.stack_label
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

/// LLM call error reported by the closure (transport / timeout etc.).
#[derive(Debug, Clone)]
pub struct TesterLlmError(pub String);

/// CB-004 (Issue #459): allowlist validation for a Cargo `package.name` we are
/// about to embed verbatim into the harness `Cargo.toml`. Reject anything that
/// could break TOML quoting or smuggle a section header. The cargo manifest
/// allows `[a-zA-Z0-9_-]` for crate names — we use the same character class.
fn validate_cargo_package_name_for_harness(name: &str) -> Result<(), AbortReason> {
    if name.is_empty() {
        return Err(AbortReason::HarnessBuildFailure(
            "package name is empty".to_string(),
        ));
    }
    if name.len() > 128 {
        return Err(AbortReason::HarnessBuildFailure(format!(
            "package name exceeds 128 bytes: {name:?}"
        )));
    }
    for ch in name.chars() {
        let allowed = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if !allowed {
            return Err(AbortReason::HarnessBuildFailure(format!(
                "package name contains disallowed char '{ch}': {name:?}"
            )));
        }
    }
    Ok(())
}

/// CB-004 (Issue #459): reject paths that would break TOML string quoting when
/// inlined as `path = "..."`. Disallow any `"`, backslash, or ASCII control
/// character (newline / CR / NUL / TAB) in the rendered path.
fn validate_workspace_path_for_harness(path: &Path) -> Result<String, AbortReason> {
    let rendered = path.to_string_lossy().into_owned();
    for ch in rendered.chars() {
        if ch == '"' || ch == '\\' || ch.is_ascii_control() {
            return Err(AbortReason::HarnessBuildFailure(format!(
                "workspace path contains TOML-unsafe char {:?}: {}",
                ch, rendered
            )));
        }
    }
    Ok(rendered)
}

/// `tester-runs/<run_id>/` の harness 生成失敗を抽象化する struct (Rust 経路のみ
/// 該当). Node / Python では Ok(()) を即返す.
///
/// CB-004 (Issue #459): both the dependency key (`package_name`) and the path
/// dependency value (`workspace_manifest_dir`) are LLM-/filesystem-derived. We
/// validate them with strict allowlists *before* string-formatting the TOML so
/// a literal `"` or newline cannot break the manifest. Missing `package_name`
/// is now a hard error — falling back to `anvil_tester_harness` would fail
/// `cargo test` against the real workspace anyway.
pub(super) fn prepare_rust_harness(
    tester_runs_root: &Path,
    run_id: &str,
    package_name: Option<&str>,
    workspace_manifest_dir: &Path,
    smoke_body: &str,
) -> Result<PathBuf, AbortReason> {
    let pkg = match package_name {
        Some(name) => name,
        None => {
            return Err(AbortReason::HarnessBuildFailure(
                "Cargo.toml package.name is missing — cannot build path dependency".to_string(),
            ));
        }
    };
    validate_cargo_package_name_for_harness(pkg)?;
    let workspace_path_str = validate_workspace_path_for_harness(workspace_manifest_dir)?;

    let run_dir = tester_runs_root.join(run_id);
    std::fs::create_dir_all(run_dir.join("tests")).map_err(|e| {
        AbortReason::HarnessBuildFailure(format!("mkdir {}: {e}", run_dir.display()))
    })?;
    let cargo_toml = format!(
        "[package]\nname = \"anvil_tester_harness_{run_id}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n{pkg} = {{ path = \"{workspace_path_str}\" }}\n\n[[test]]\nname = \"smoke\"\npath = \"tests/smoke.rs\"\n",
    );
    std::fs::write(run_dir.join("Cargo.toml"), cargo_toml)
        .map_err(|e| AbortReason::HarnessBuildFailure(format!("write Cargo.toml: {e}")))?;
    std::fs::write(run_dir.join("tests").join("smoke.rs"), smoke_body)
        .map_err(|e| AbortReason::HarnessBuildFailure(format!("write smoke.rs: {e}")))?;
    Ok(run_dir)
}

/// Best-effort cleanup of the transient harness dir (DR1-009).
pub(super) fn cleanup_tester_run_dir(run_dir: &Path) {
    let _ = std::fs::remove_dir_all(run_dir);
}

/// CB-005 (Issue #459): cap the number of `tester-runs/<run_id>/` directories
/// kept on disk. Anything beyond the most recent `keep` (sorted by mtime) is
/// removed best-effort. Catches accumulated harness dirs from process panics,
/// timeouts, and approval-denied cancels where per-run cleanup did not get to
/// run. `keep == 0` removes everything.
pub(super) fn cleanup_stale_tester_runs(tester_runs_root: &Path, keep: usize) {
    let entries = match std::fs::read_dir(tester_runs_root) {
        Ok(it) => it,
        Err(_) => return,
    };
    let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Defence-in-depth: only consider entries that look like a tester
        // run dir (`run_<allowlisted body>`). Anything else is left alone so
        // we cannot accidentally rm a sibling artifact dir.
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if validate_run_id(name).is_err() {
            continue;
        }
        let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        dirs.push((path, mtime));
    }
    if dirs.len() <= keep {
        return;
    }
    // Sort newest first; remove anything past `keep`.
    dirs.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (path, _) in dirs.into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// CB-005 (Issue #459): how many tester-runs we keep around between Tester
/// invocations. Older dirs are removed at the start of each dispatch so a
/// long-lived session cannot accumulate gigabytes of cargo build artefacts.
pub(super) const TESTER_RUNS_KEEP_LATEST: usize = 8;

/// Pure-function orchestration of a Tester run. The caller (turn.rs) wraps
/// this with `maybe_invoke_tester` to apply the per-turn cap + reset flag.
///
/// ステップ:
///   1. disable 判定 (Plan / env) → NotInvoked
///   2. prompt 構築 → llm_call (closure) → AbortReason::LlmCall on err
///   3. validate_llm_reply → AbortReason::LlmMalformed on err
///   4. tmp_tests::create_generated_test → AbortReason::TmpTestWriteFailure on err
///   5. (Rust only) prepare_rust_harness → AbortReason::HarnessBuildFailure on err
///   6. prompt_for_approval (closure-driven for tests) → AbortReason::ApprovalDenied
///   7. execute_smoke_test (closure) → AbortReason::BashFailure on err
///   8. classify_tester_outcome → build_feedback_frame → Recorded(frame)
///   9. cleanup_tester_run_dir (best-effort)
pub fn run_tester_with_strategy<F, B, A>(
    run: TesterRun<'_>,
    candidate: TesterCandidate,
    llm_call: F,
    run_bash: B,
    approver: A,
) -> TesterOutcome
where
    F: FnOnce(&TesterPrompt) -> Result<String, TesterLlmError>,
    B: FnOnce(&str, &Path, Option<Duration>) -> Result<BashExecutionOutcome, String>,
    A: FnOnce(ApprovalMode, &[String]) -> Result<(), AbortReason>,
{
    if run.plan_mode {
        return TesterOutcome::NotInvoked(NotInvokedReason::PlanMode);
    }
    if run.no_tester_env {
        return TesterOutcome::NotInvoked(NotInvokedReason::Disabled);
    }
    // CB-005 (Issue #459): trim accumulated harness dirs at dispatch time so
    // a session that ran into panics / timeouts / approval-denied flows on
    // earlier turns cannot grow `tester-runs/` without bound. The current
    // run's dir is created later by `prepare_rust_harness` so it is never a
    // cleanup target here.
    cleanup_stale_tester_runs(run.tester_runs_root, TESTER_RUNS_KEEP_LATEST);
    let prompt = build_tester_prompt(&run, &candidate);
    let raw_reply = match llm_call(&prompt) {
        Ok(r) => r,
        Err(TesterLlmError(e)) => return TesterOutcome::Aborted(AbortReason::LlmCall(e)),
    };
    let reply = match validate_llm_reply(&raw_reply) {
        Ok(r) => r,
        Err(reason) => return TesterOutcome::Aborted(reason),
    };
    let entry = match reply.test_files.into_iter().next() {
        Some(e) => e,
        None => {
            return TesterOutcome::Aborted(AbortReason::LlmMalformed(
                "test_files empty after validate".to_string(),
            ));
        }
    };
    let tmp_test = match crate::session::tmp_tests::create_generated_test(
        run.tmp_tests_root,
        &entry.relative_path,
        entry.content.as_bytes(),
    ) {
        Ok(t) => t,
        Err(e) => return TesterOutcome::Aborted(AbortReason::TmpTestWriteFailure(e)),
    };

    let created_at = tmp_test.created_at;
    let run_id = derive_run_id(entry.content.as_bytes(), created_at);
    if let Err(reason) = validate_run_id(&run_id) {
        return TesterOutcome::Aborted(reason);
    }

    // Rust harness 構築 (Rust 経路のみ).
    let mut harness_dir: Option<PathBuf> = None;
    if let TesterCandidate::Rust {
        manifest_path,
        package_name,
    } = &candidate
    {
        let workspace_manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        match prepare_rust_harness(
            run.tester_runs_root,
            &run_id,
            package_name.as_deref(),
            workspace_manifest_dir,
            &entry.content,
        ) {
            Ok(d) => harness_dir = Some(d),
            Err(reason) => return TesterOutcome::Aborted(reason),
        }
    }

    let rel_test = PathBuf::from(&entry.relative_path);
    let command = build_command(
        &candidate,
        &run_id,
        run.tmp_tests_root,
        run.tester_runs_root,
        &rel_test,
    );

    if let Err(reason) = approver(run.approval_mode, &command) {
        if let Some(dir) = harness_dir.as_ref() {
            cleanup_tester_run_dir(dir);
        }
        return TesterOutcome::Aborted(reason);
    }

    let cwd = harness_dir
        .clone()
        .unwrap_or_else(|| run.work_root.to_path_buf());
    let outcome = match execute_smoke_test(&command, &cwd, run_bash) {
        Ok(o) => o,
        Err(reason) => {
            if let Some(dir) = harness_dir.as_ref() {
                cleanup_tester_run_dir(dir);
            }
            return TesterOutcome::Aborted(reason);
        }
    };

    // CB-001 (Issue #459): classify operates on the raw outcome so timeout /
    // interrupt / blocked smokes abort instead of being mis-classified as
    // TestPass / BuildPass via a synthesised exit_code = 0.
    let class = match classify_tester_outcome(&candidate, &outcome) {
        Ok(c) => c,
        Err(reason) => {
            if let Some(dir) = harness_dir.as_ref() {
                cleanup_tester_run_dir(dir);
            }
            return TesterOutcome::Aborted(reason);
        }
    };
    let primary_error = if matches!(class, TesterClass::TestPass | TesterClass::BuildPass) {
        None
    } else {
        Some(format!(
            "tester smoke test failed (stack={})",
            stack_label(&candidate)
        ))
    };
    let draft = FeedbackFrameDraft {
        command: Some(command.join(" ")),
        exit_code: outcome.exit_code,
        kind: class.to_feedback_kind(),
        stdout: outcome.stdout.clone(),
        stderr: outcome.stderr.clone(),
        primary_error,
        suspected_files: vec![PathBuf::from(&entry.relative_path)],
        changed_files: Vec::new(),
    };
    let frame = build_feedback_frame(draft, run.work_root);

    if let Some(dir) = harness_dir.as_ref() {
        cleanup_tester_run_dir(dir);
    }

    TesterOutcome::Recorded(frame)
}

fn stack_label(candidate: &TesterCandidate) -> &'static str {
    match candidate {
        TesterCandidate::Rust { .. } => "rust",
        TesterCandidate::Node { .. } => "node",
        TesterCandidate::Python { .. } => "python",
    }
}

/// Build a stack-aware Tester prompt body. Kept ≤ 2 KiB per § 10 設計判断 #2.
pub(super) fn build_tester_prompt(
    run: &TesterRun<'_>,
    candidate: &TesterCandidate,
) -> TesterPrompt {
    let stack_label = stack_label(candidate);
    let mut body = String::with_capacity(2048);
    body.push_str(
        "You are the Tester. Generate ONE smoke test for this repo and return ONLY a JSON object:\n\
         {\"test_files\":[{\"relative_path\":\"smoke_basic.<ext>\",\"content\":\"...\"}]}\n\n\
         Rules:\n\
         - Output JSON only, no prose, no <think> tags.\n\
         - relative_path must be a forward-slash separated path with no '..' / absolute / NUL.\n\
         - Exactly ONE entry in test_files.\n\
         - Do not import network packages, do not use npx / install.\n\
         - The path is rooted under tmp-tests/files/ — do not include that prefix.\n\n",
    );
    body.push_str(&format!("# Stack\n{stack_label}\n\n"));
    body.push_str(&format!("# Session\n{}\n\n", run.session_id));
    match candidate {
        TesterCandidate::Rust { package_name, .. } => {
            body.push_str(&format!(
                "# Rust hints\n- harness uses path-dependency on package '{}'.\n- Smoke must compile against the workspace lib API.\n- Use std-only assert! / assert_eq! when in doubt.\n",
                package_name.as_deref().unwrap_or("(unknown)"),
            ));
        }
        TesterCandidate::Node {
            has_build_script, ..
        } => {
            body.push_str(&format!(
                "# Node hints\n- Output a JS file syntax-checkable by `node --check`.\n- has_build_script={has_build_script}.\n",
            ));
        }
        TesterCandidate::Python { surface_files } => {
            body.push_str(
                "# Python hints\n- Output a .py file that py_compile can syntax-check.\n",
            );
            for f in surface_files.iter().take(3) {
                body.push_str(&format!("- surface: {}\n", f.display()));
            }
        }
    }
    body.push_str("\n# Output\nJSON only.\n");
    if body.len() > 2048 {
        body.truncate(2048);
    }
    TesterPrompt { stack_label, body }
}

// ============================================================================
// 9. log payload helper (DR4-006)
// ============================================================================

pub(super) fn sanitize_tester_log(raw: &str, cap: usize) -> String {
    crate::ollama::parsing::truncate_for_log(&crate::session::feedback::mask_secrets(raw), cap)
}

// ============================================================================
// EXTENSION POINT: 新 stack 追加時はここを起点に 5 関数を grep して一括で書く.
//   grep -n "TesterCandidate::Rust\|TesterCandidate::Node\|TesterCandidate::Python" \
//     src/agent/loop_run/tester.rs
// が漏れ確認の正準コマンド (§ 5-1 / DR1-005).
// ============================================================================

// ============================================================================
// 10. Tests (Task 1b.1 ~ 1b.6 の TDD ペイロード)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::tempdir;

    // ---- Task 1b.1: 型コンパイル確認 ---------------------------------------

    #[test]
    fn tester_outcome_compiles() {
        let _ = TesterOutcome::NotInvoked(NotInvokedReason::NoCandidate);
        let _ = TesterOutcome::Aborted(AbortReason::ApprovalDenied);
    }

    #[test]
    fn approval_mode_is_copy() {
        let m = ApprovalMode::Auto;
        let n = m;
        assert_eq!(m, n);
    }

    #[test]
    fn constants_have_expected_values() {
        assert_eq!(TESTER_SMOKE_TIMEOUT_SECS, 30);
        assert_eq!(MAX_TESTER_LLM_REPLY_BYTES, 64 * 1024);
        assert_eq!(MAX_GENERATED_TESTS_PER_TURN, 1);
    }

    // ---- Task 1b.2: TesterCandidate::detect ---------------------------------

    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn detect_rust_when_only_cargo_manifest() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[package]\nname = \"anvil_test_fix\"\nversion = \"0.0.0\"\n",
        );
        let c = TesterCandidate::detect(dir.path(), &[]).expect("rust candidate");
        match c {
            TesterCandidate::Rust { package_name, .. } => {
                assert_eq!(package_name.as_deref(), Some("anvil_test_fix"));
            }
            other => panic!("expected Rust, got {other:?}"),
        }
    }

    #[test]
    fn detect_returns_none_when_tests_dir_present() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\n",
        );
        std::fs::create_dir(dir.path().join("tests")).unwrap();
        // explicit verifier: cargo test with tests/
        assert!(TesterCandidate::detect(dir.path(), &[]).is_none());
    }

    /// CB-006 (Issue #459): a `[[test]]` section in Cargo.toml is an explicit
    /// test verifier even if `tests/` is missing — Tester must not fire.
    #[test]
    fn detect_returns_none_when_cargo_has_explicit_test_target() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.0.0\"\n\n[[test]]\nname = \"smoke\"\npath = \"tests/smoke.rs\"\n",
        );
        // No tests/ dir, but [[test]] is declared → still explicit.
        assert!(TesterCandidate::detect(dir.path(), &[]).is_none());
    }

    #[test]
    fn detect_node_when_package_json_lacks_test_script() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("package.json"),
            r#"{"scripts": {"build": "tsc"}}"#,
        );
        let c = TesterCandidate::detect(dir.path(), &[]).expect("node candidate");
        match c {
            TesterCandidate::Node {
                has_build_script, ..
            } => {
                assert!(has_build_script);
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn detect_returns_none_when_package_json_has_test_script() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("package.json"),
            r#"{"scripts": {"test": "vitest"}}"#,
        );
        // explicit verifier: npm test → Tester は起動しない
        assert!(TesterCandidate::detect(dir.path(), &[]).is_none());
    }

    #[test]
    fn detect_python_when_only_py_files_no_pytest_config() {
        // No pyproject.toml / pytest.ini / tests/ → AutoTestRunner returns
        // py_compile (build-only verifier) which is *not* explicit, so Tester
        // takes the Python branch.
        let dir = tempdir().unwrap();
        write(&dir.path().join("app.py"), "print('ok')\n");
        let c =
            TesterCandidate::detect(dir.path(), &["app.py".to_string()]).expect("python candidate");
        match c {
            TesterCandidate::Python { surface_files } => {
                assert!(!surface_files.is_empty());
            }
            other => panic!("expected Python, got {other:?}"),
        }
    }

    #[test]
    fn detect_python_when_pyproject_has_no_pytest_signal() {
        // pyproject.toml alone no longer implies pytest. AutoTestRunner falls
        // back to py_compile, which is build-only, so Tester can still create
        // a Python smoke test.
        let dir = tempdir().unwrap();
        write(&dir.path().join("pyproject.toml"), "[tool.poetry]\n");
        write(&dir.path().join("app.py"), "print('ok')\n");
        assert!(matches!(
            TesterCandidate::detect(dir.path(), &["app.py".to_string()]),
            Some(TesterCandidate::Python { .. })
        ));
    }

    #[test]
    fn detect_returns_none_when_pyproject_has_pytest_dependency() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("pyproject.toml"),
            "[project]\ndependencies = ['pytest']\n",
        );
        write(&dir.path().join("app.py"), "print('ok')\n");
        assert!(TesterCandidate::detect(dir.path(), &["app.py".to_string()]).is_none());
    }

    #[test]
    fn detect_returns_none_for_empty_workspace() {
        let dir = tempdir().unwrap();
        assert!(TesterCandidate::detect(dir.path(), &[]).is_none());
    }

    // ---- Task 1b.3: validate_llm_reply -------------------------------------

    #[test]
    fn validate_rejects_empty() {
        assert!(matches!(
            validate_llm_reply(""),
            Err(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn validate_rejects_oversized() {
        let big = "x".repeat(MAX_TESTER_LLM_REPLY_BYTES + 1);
        assert!(matches!(
            validate_llm_reply(&big),
            Err(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn validate_rejects_think_only() {
        assert!(matches!(
            validate_llm_reply("<think>only</think>"),
            Err(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn validate_rejects_no_json() {
        assert!(matches!(
            validate_llm_reply("hello world"),
            Err(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn validate_rejects_two_files() {
        let body = r#"{"test_files":[
            {"relative_path":"a.rs","content":"x"},
            {"relative_path":"b.rs","content":"y"}
        ]}"#;
        assert!(matches!(
            validate_llm_reply(body),
            Err(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn validate_rejects_path_traversal() {
        let body = r#"{"test_files":[{"relative_path":"../escape.rs","content":"x"}]}"#;
        assert!(matches!(
            validate_llm_reply(body),
            Err(AbortReason::PathConfinementViolation(_))
        ));
    }

    #[test]
    fn validate_rejects_newline_in_path() {
        let body = "{\"test_files\":[{\"relative_path\":\"a\\nb.rs\",\"content\":\"x\"}]}";
        assert!(matches!(
            validate_llm_reply(body),
            Err(AbortReason::PathConfinementViolation(_))
        ));
    }

    #[test]
    fn validate_accepts_normal_single_file() {
        let body = "{\"test_files\":[{\"relative_path\":\"smoke.rs\",\"content\":\"fn t(){}\"}]}";
        let parsed = validate_llm_reply(body).expect("ok");
        assert_eq!(parsed.test_files.len(), 1);
        assert_eq!(parsed.test_files[0].relative_path, "smoke.rs");
    }

    // ---- Task 1b.4: build_command + classify -------------------------------

    fn rust_candidate() -> TesterCandidate {
        TesterCandidate::Rust {
            manifest_path: PathBuf::from("/repo/Cargo.toml"),
            package_name: Some("foo".to_string()),
        }
    }

    fn node_candidate() -> TesterCandidate {
        TesterCandidate::Node {
            manifest_path: PathBuf::from("/repo/package.json"),
            has_build_script: false,
        }
    }

    fn python_candidate() -> TesterCandidate {
        TesterCandidate::Python {
            surface_files: vec![PathBuf::from("app.py")],
        }
    }

    #[test]
    fn build_command_rust_uses_manifest_path() {
        let cmd = build_command(
            &rust_candidate(),
            "run_abcdef0123456789abcd",
            Path::new("/state/sessions/x/tmp-tests"),
            Path::new("/state/sessions/x/tester-runs"),
            Path::new("smoke.rs"),
        );
        // CB-005: Rust template prefixes `env CARGO_TARGET_DIR=...` so cargo
        // build artefacts land inside the tester-runs run dir, never in the
        // workspace `target/`.
        assert_eq!(cmd[0], "env");
        assert!(
            cmd[1].starts_with("CARGO_TARGET_DIR="),
            "expected CARGO_TARGET_DIR= prefix at index 1, got: {cmd:?}"
        );
        assert_eq!(cmd[2], "cargo");
        assert_eq!(cmd[3], "test");
        assert_eq!(cmd[4], "--manifest-path");
        assert!(cmd[5].contains("tester-runs"));
        assert!(cmd[5].contains("Cargo.toml"));
        // No tmp-tests/files/ literal in Rust command (DR4 security).
        assert!(
            !cmd.iter()
                .any(|s| s.contains("tmp-tests/files/") && !s.contains("tester-runs"))
        );
        // Manifest path is absolute (starts with `/`).
        assert!(cmd[5].starts_with("'/"));
    }

    #[test]
    fn build_command_node_uses_node_check() {
        let cmd = build_command(
            &node_candidate(),
            "run_abcdef0123456789abcd",
            Path::new("/state/sessions/x/tmp-tests"),
            Path::new("/state/sessions/x/tester-runs"),
            Path::new("smoke.js"),
        );
        assert_eq!(cmd[0], "node");
        assert_eq!(cmd[1], "--check");
        assert!(cmd[2].contains("tmp-tests/files/smoke.js"));
        // No npx / install in command.
        assert!(
            !cmd.iter()
                .any(|s| s.contains("npx") || s.contains("install"))
        );
    }

    #[test]
    fn build_command_python_uses_py_compile() {
        let cmd = build_command(
            &python_candidate(),
            "run_abcdef0123456789abcd",
            Path::new("/state/sessions/x/tmp-tests"),
            Path::new("/state/sessions/x/tester-runs"),
            Path::new("smoke.py"),
        );
        assert_eq!(cmd[0], "python3");
        assert_eq!(cmd[1], "-m");
        assert_eq!(cmd[2], "py_compile");
        assert!(cmd[3].contains("tmp-tests/files/smoke.py"));
    }

    #[test]
    fn build_command_escapes_shell_metachars() {
        let cmd = build_command(
            &node_candidate(),
            "run_abcdef0123456789abcd",
            Path::new("/state/sessions/x/tmp-tests"),
            Path::new("/state/sessions/x/tester-runs"),
            Path::new("a'b.js"),
        );
        // shell_quote wraps in single quotes and escapes inner '.
        assert!(cmd[2].starts_with('\''));
        assert!(cmd[2].ends_with('\''));
    }

    fn outcome_with(code: Option<i32>, stdout: &str, stderr: &str) -> BashExecutionOutcome {
        BashExecutionOutcome {
            command: "fake".to_string(),
            exit_code: code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            timed_out: false,
            blocked_reason: None,
            interrupted: false,
        }
    }

    #[test]
    fn classify_rust_pass_is_test_pass() {
        let outcome = outcome_with(Some(0), "test result: ok", "");
        let class = classify_tester_outcome(&rust_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::TestPass);
    }

    #[test]
    fn classify_rust_fail_with_compile_error() {
        let outcome = outcome_with(Some(101), "", "error[E0308]: mismatched types");
        let class = classify_tester_outcome(&rust_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::CompileError);
    }

    #[test]
    fn classify_rust_fail_test_failure() {
        let outcome = outcome_with(Some(101), "FAILED  test_x", "");
        let class = classify_tester_outcome(&rust_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::TestFailure);
    }

    #[test]
    fn classify_node_pass_is_build_pass() {
        let outcome = outcome_with(Some(0), "", "");
        let class = classify_tester_outcome(&node_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::BuildPass);
    }

    #[test]
    fn classify_node_fail_compile_error() {
        let outcome = outcome_with(Some(1), "", "SyntaxError");
        let class = classify_tester_outcome(&node_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::CompileError);
    }

    #[test]
    fn classify_python_pass_is_test_pass_not_build_pass() {
        let outcome = outcome_with(Some(0), "", "");
        let class = classify_tester_outcome(&python_candidate(), &outcome).expect("ok");
        assert_eq!(class, TesterClass::TestPass);
    }

    // ---- CB-001: timeout / interrupted / blocked must NOT classify as success

    #[test]
    fn classify_aborts_when_timed_out_even_if_exit_code_zero() {
        let mut outcome = outcome_with(Some(0), "", "");
        outcome.timed_out = true;
        let res = classify_tester_outcome(&rust_candidate(), &outcome);
        assert!(matches!(res, Err(AbortReason::SmokeTimeout(_))));
    }

    #[test]
    fn classify_aborts_when_timed_out_with_no_exit_code() {
        let mut outcome = outcome_with(None, "", "");
        outcome.timed_out = true;
        for stack in [rust_candidate(), node_candidate(), python_candidate()] {
            let res = classify_tester_outcome(&stack, &outcome);
            assert!(
                matches!(res, Err(AbortReason::SmokeTimeout(_))),
                "stack {stack:?} must abort with SmokeTimeout"
            );
        }
    }

    #[test]
    fn classify_aborts_when_interrupted() {
        let mut outcome = outcome_with(Some(0), "", "");
        outcome.interrupted = true;
        let res = classify_tester_outcome(&python_candidate(), &outcome);
        assert!(matches!(res, Err(AbortReason::SmokeInterrupted(_))));
    }

    #[test]
    fn classify_aborts_when_blocked() {
        let mut outcome = outcome_with(Some(0), "", "");
        outcome.blocked_reason = Some("blocked dangerous command fragment".to_string());
        let res = classify_tester_outcome(&node_candidate(), &outcome);
        assert!(matches!(res, Err(AbortReason::SmokeBlocked(_))));
    }

    #[test]
    fn classify_aborts_when_exit_code_is_none_without_other_flags() {
        // No timeout / interrupt / blocked — exit_code = None still must not
        // be silently coerced to TestPass / BuildPass.
        let outcome = outcome_with(None, "", "");
        let res = classify_tester_outcome(&rust_candidate(), &outcome);
        assert!(matches!(res, Err(AbortReason::BashFailure(_))));
    }

    #[test]
    fn validate_run_id_accepts_canonical() {
        validate_run_id("run_abcdef0123456789abcd").unwrap();
    }

    #[test]
    fn validate_run_id_rejects_traversal() {
        assert!(validate_run_id("run_../escape").is_err());
        assert!(validate_run_id("run_/abs").is_err());
        assert!(validate_run_id("run_a\0b1234567890abcd").is_err());
    }

    #[test]
    fn derive_run_id_is_deterministic() {
        let a = derive_run_id(b"hello", 42);
        let b = derive_run_id(b"hello", 42);
        assert_eq!(a, b);
        validate_run_id(&a).unwrap();
    }

    // ---- CB-005: stale tester-runs cleanup cap + CARGO_TARGET_DIR ----------

    #[test]
    fn cleanup_stale_tester_runs_caps_to_keep_count() {
        // Create 9 fake run dirs; expect oldest to be removed once we keep 8.
        let runs = tempdir().unwrap();
        let total = 9usize;
        let mut names = Vec::new();
        for i in 0..total {
            let name = format!("run_aaaaaaaaaaaaaaaaaa_{i:04}");
            let dir = runs.path().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("marker"), format!("marker {i}")).unwrap();
            names.push(name);
            // Sleep a tiny bit so mtimes are monotonically increasing on
            // filesystems with nanosecond precision (HFS / ext4 / APFS).
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        cleanup_stale_tester_runs(runs.path(), 8);
        let entries: Vec<_> = std::fs::read_dir(runs.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries.len(),
            8,
            "after cleanup, only the most recent 8 dirs should remain"
        );
        // The oldest (index 0) must be the one removed.
        assert!(
            !entries.contains(&names[0]),
            "oldest run dir should have been removed, but still present: {entries:?}"
        );
        // The newest (index 8) must still be present.
        assert!(
            entries.contains(&names[8]),
            "newest run dir should remain, got: {entries:?}"
        );
    }

    #[test]
    fn cleanup_stale_tester_runs_no_op_when_under_cap() {
        let runs = tempdir().unwrap();
        for i in 0..3 {
            std::fs::create_dir_all(runs.path().join(format!("run_aaaaaaaaaaaaaaaa_{i}"))).unwrap();
        }
        cleanup_stale_tester_runs(runs.path(), 8);
        let entries: Vec<_> = std::fs::read_dir(runs.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 3, "under-cap dirs must not be touched");
    }

    /// CB-005: Rust harness command must carry `CARGO_TARGET_DIR` pointing
    /// inside the tester-runs/<run_id>/ dir so build artifacts never leak
    /// into the workspace `target/`.
    #[test]
    fn build_command_rust_inlines_cargo_target_dir() {
        let cmd = build_command(
            &rust_candidate(),
            "run_abcdef0123456789abcd",
            Path::new("/state/sessions/x/tmp-tests"),
            Path::new("/state/sessions/x/tester-runs"),
            Path::new("smoke.rs"),
        );
        assert_eq!(cmd[0], "env");
        // CARGO_TARGET_DIR must be set as the first env var.
        assert!(
            cmd.iter()
                .any(|s| s.starts_with("CARGO_TARGET_DIR=") && s.contains("tester-runs")),
            "expected CARGO_TARGET_DIR=...tester-runs/run_.../target in: {cmd:?}"
        );
        assert!(
            cmd.iter().any(|s| s.contains("/target")),
            "CARGO_TARGET_DIR value must end at .../target"
        );
        // cargo test --manifest-path must follow the env wrapper.
        assert!(
            cmd.iter().any(|s| s == "cargo"),
            "cargo invocation must follow the env wrapper"
        );
        assert!(cmd.iter().any(|s| s == "test"));
    }

    // ---- CB-004: prepare_rust_harness Cargo.toml escaping -------------------

    #[test]
    fn prepare_rust_harness_rejects_unsafe_package_name_quote() {
        let runs = tempdir().unwrap();
        let work = tempdir().unwrap();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            Some("evil\"name"),
            work.path(),
            "fn smoke(){}",
        );
        assert!(
            matches!(res, Err(AbortReason::HarnessBuildFailure(_))),
            "double-quote in package name must reject"
        );
    }

    #[test]
    fn prepare_rust_harness_rejects_unsafe_package_name_newline() {
        let runs = tempdir().unwrap();
        let work = tempdir().unwrap();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            Some("evil\nname"),
            work.path(),
            "fn smoke(){}",
        );
        assert!(matches!(res, Err(AbortReason::HarnessBuildFailure(_))));
    }

    #[test]
    fn prepare_rust_harness_rejects_unsafe_package_name_section_header() {
        let runs = tempdir().unwrap();
        let work = tempdir().unwrap();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            Some("evil[section]"),
            work.path(),
            "fn smoke(){}",
        );
        assert!(matches!(res, Err(AbortReason::HarnessBuildFailure(_))));
    }

    #[test]
    fn prepare_rust_harness_rejects_missing_package_name() {
        // CB-004: an absent package name (e.g. virtual workspace) must not
        // silently fall back to a synthetic `anvil_tester_harness` dependency
        // key — that would never resolve against the real workspace.
        let runs = tempdir().unwrap();
        let work = tempdir().unwrap();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            None,
            work.path(),
            "fn smoke(){}",
        );
        assert!(matches!(res, Err(AbortReason::HarnessBuildFailure(_))));
    }

    #[test]
    fn prepare_rust_harness_accepts_normal_package_name() {
        let runs = tempdir().unwrap();
        let work = tempdir().unwrap();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            Some("anvil_test_fix-2"),
            work.path(),
            "fn smoke(){}",
        );
        let dir = res.expect("ok");
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        // package name appears as dependency key.
        assert!(manifest.contains("anvil_test_fix-2 = { path"));
        // smoke body landed on disk.
        assert!(dir.join("tests").join("smoke.rs").is_file());
    }

    #[test]
    fn prepare_rust_harness_rejects_workspace_path_with_quote() {
        // CB-004: a workspace path containing a `"` would terminate the TOML
        // string literal early. Reject up front.
        let runs = tempdir().unwrap();
        let work_root = tempdir().unwrap();
        let bad_path = work_root.path().join("evil\"path");
        std::fs::create_dir_all(&bad_path).ok();
        let res = prepare_rust_harness(
            runs.path(),
            "run_abcdef0123456789abcd",
            Some("ok_name"),
            &bad_path,
            "fn smoke(){}",
        );
        assert!(matches!(res, Err(AbortReason::HarnessBuildFailure(_))));
    }

    // ---- Task 1b.5: approval prompt + execute_smoke_test --------------------

    #[test]
    fn approval_auto_returns_ok() {
        let mut out = Vec::new();
        let mut input = Cursor::new(Vec::new());
        let res = prompt_for_approval(
            ApprovalMode::Auto,
            &["echo".to_string()],
            &mut out,
            &mut input,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn approval_forbidden_returns_denied() {
        let mut out = Vec::new();
        let mut input = Cursor::new(Vec::new());
        let res = prompt_for_approval(
            ApprovalMode::Forbidden,
            &["echo".to_string()],
            &mut out,
            &mut input,
        );
        assert!(matches!(res, Err(AbortReason::ApprovalDenied)));
    }

    #[test]
    fn approval_interactive_yes_passes() {
        let mut out = Vec::new();
        let mut input = Cursor::new(b"y\n".to_vec());
        let res = prompt_for_approval(
            ApprovalMode::Interactive,
            &["cargo".to_string(), "test".to_string()],
            &mut out,
            &mut input,
        );
        assert!(res.is_ok());
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("[Tester]"));
        assert!(text.contains("(y/N)"));
    }

    #[test]
    fn approval_interactive_no_denies() {
        let mut out = Vec::new();
        let mut input = Cursor::new(b"n\n".to_vec());
        let res = prompt_for_approval(
            ApprovalMode::Interactive,
            &["cargo".to_string()],
            &mut out,
            &mut input,
        );
        assert!(matches!(res, Err(AbortReason::ApprovalDenied)));
    }

    #[test]
    fn approval_interactive_blank_denies() {
        let mut out = Vec::new();
        let mut input = Cursor::new(b"\n".to_vec());
        let res = prompt_for_approval(
            ApprovalMode::Interactive,
            &["cargo".to_string()],
            &mut out,
            &mut input,
        );
        assert!(matches!(res, Err(AbortReason::ApprovalDenied)));
    }

    #[test]
    fn execute_smoke_test_passes_explicit_timeout() {
        let mut received: Option<Option<Duration>> = None;
        // Closure captures the timeout so we can assert it = TESTER_SMOKE_TIMEOUT_SECS.
        let cmd = vec!["echo".to_string(), "ok".to_string()];
        let result = execute_smoke_test(&cmd, Path::new("/tmp"), |_cmd, _cwd, t| {
            received = Some(t);
            Ok(BashExecutionOutcome {
                command: "echo ok".to_string(),
                exit_code: Some(0),
                stdout: "ok".to_string(),
                stderr: String::new(),
                timed_out: false,
                blocked_reason: None,
                interrupted: false,
            })
        });
        assert!(result.is_ok());
        assert_eq!(
            received,
            Some(Some(Duration::from_secs(TESTER_SMOKE_TIMEOUT_SECS)))
        );
    }

    // ---- Task 1b.6: run_tester_with_strategy --------------------------------

    fn empty_run<'a>(
        work_root: &'a Path,
        tmp_root: &'a Path,
        runs_root: &'a Path,
    ) -> TesterRun<'a> {
        TesterRun {
            work_root,
            tmp_tests_root: tmp_root,
            tester_runs_root: runs_root,
            approval_mode: ApprovalMode::Auto,
            plan_mode: false,
            no_tester_env: false,
            session_id: std::borrow::Cow::Borrowed("session-test"),
        }
    }

    fn ok_bash()
    -> impl FnOnce(&str, &Path, Option<Duration>) -> Result<BashExecutionOutcome, String> {
        |_cmd, _cwd, _t| {
            Ok(BashExecutionOutcome {
                command: "fake".to_string(),
                exit_code: Some(0),
                stdout: "test result: ok".to_string(),
                stderr: String::new(),
                timed_out: false,
                blocked_reason: None,
                interrupted: false,
            })
        }
    }

    fn fail_bash()
    -> impl FnOnce(&str, &Path, Option<Duration>) -> Result<BashExecutionOutcome, String> {
        |_cmd, _cwd, _t| {
            Ok(BashExecutionOutcome {
                command: "fake".to_string(),
                exit_code: Some(101),
                stdout: String::new(),
                stderr: "FAILED test_x".to_string(),
                timed_out: false,
                blocked_reason: None,
                interrupted: false,
            })
        }
    }

    fn auto_approver() -> impl FnOnce(ApprovalMode, &[String]) -> Result<(), AbortReason> {
        |_m, _c| Ok(())
    }

    fn deny_approver() -> impl FnOnce(ApprovalMode, &[String]) -> Result<(), AbortReason> {
        |_m, _c| Err(AbortReason::ApprovalDenied)
    }

    fn rust_workspace_fixture() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[package]\nname = \"fix\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        );
        write(
            &dir.path().join("src").join("lib.rs"),
            "pub fn ok() -> i32 { 1 }\n",
        );
        dir
    }

    fn rust_smoke_reply() -> String {
        // Note: deliberately avoid `#[test]` attribute literal here because raw
        // string literals cannot contain it and Rust 2021 reserves the prefix.
        "{\"test_files\":[{\"relative_path\":\"smoke_basic.rs\",\"content\":\"fn smoke(){ assert_eq!(1,1); }\\n\"}]}"
            .to_string()
    }

    fn node_smoke_reply() -> String {
        r#"{"test_files":[{"relative_path":"smoke_basic.js","content":"console.log('ok');\n"}]}"#
            .to_string()
    }

    fn python_smoke_reply() -> String {
        r#"{"test_files":[{"relative_path":"smoke_basic.py","content":"print('ok')\n"}]}"#
            .to_string()
    }

    #[test]
    fn run_skips_when_plan_mode() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let mut run = empty_run(work.path(), tmp.path(), runs.path());
        run.plan_mode = true;
        let outcome = run_tester_with_strategy(
            run,
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            ok_bash(),
            auto_approver(),
        );
        assert!(matches!(
            outcome,
            TesterOutcome::NotInvoked(NotInvokedReason::PlanMode)
        ));
    }

    #[test]
    fn run_skips_when_disabled_env() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let mut run = empty_run(work.path(), tmp.path(), runs.path());
        run.no_tester_env = true;
        let outcome = run_tester_with_strategy(
            run,
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            ok_bash(),
            auto_approver(),
        );
        assert!(matches!(
            outcome,
            TesterOutcome::NotInvoked(NotInvokedReason::Disabled)
        ));
    }

    #[test]
    fn run_aborts_on_llm_error() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Err(TesterLlmError("transport".into())),
            ok_bash(),
            auto_approver(),
        );
        assert!(matches!(
            outcome,
            TesterOutcome::Aborted(AbortReason::LlmCall(_))
        ));
    }

    #[test]
    fn run_aborts_on_malformed_reply() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok("not json".to_string()),
            ok_bash(),
            auto_approver(),
        );
        assert!(matches!(
            outcome,
            TesterOutcome::Aborted(AbortReason::LlmMalformed(_))
        ));
    }

    #[test]
    fn run_aborts_on_approval_denied() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            ok_bash(),
            deny_approver(),
        );
        assert!(matches!(
            outcome,
            TesterOutcome::Aborted(AbortReason::ApprovalDenied)
        ));
    }

    #[test]
    fn run_records_python_test_pass() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            ok_bash(),
            auto_approver(),
        );
        match outcome {
            TesterOutcome::Recorded(frame) => {
                assert_eq!(frame.kind, FeedbackKind::TestPass);
            }
            other => panic!("expected Recorded, got {other:?}"),
        }
    }

    #[test]
    fn run_records_node_build_pass() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            node_candidate(),
            |_p| Ok(node_smoke_reply()),
            ok_bash(),
            auto_approver(),
        );
        match outcome {
            TesterOutcome::Recorded(frame) => {
                assert_eq!(frame.kind, FeedbackKind::BuildPass);
            }
            other => panic!("expected Recorded, got {other:?}"),
        }
    }

    /// CB-001: a timed-out smoke run must not be silently classified as
    /// `Recorded(TestPass)` even when the bash outcome carries `exit_code =
    /// Some(0)` (the synthesised default). The orchestrator must surface the
    /// timeout as `Aborted(SmokeTimeout)`.
    #[test]
    fn run_aborts_when_smoke_times_out() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let timed_out_bash = |_cmd: &str,
                              _cwd: &Path,
                              _t: Option<Duration>|
         -> Result<BashExecutionOutcome, String> {
            Ok(BashExecutionOutcome {
                command: "fake".to_string(),
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
                blocked_reason: None,
                interrupted: false,
            })
        };
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            timed_out_bash,
            auto_approver(),
        );
        assert!(
            matches!(
                outcome,
                TesterOutcome::Aborted(AbortReason::SmokeTimeout(_))
            ),
            "expected SmokeTimeout abort, got {outcome:?}"
        );
    }

    #[test]
    fn run_aborts_when_smoke_interrupted() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let interrupted_bash = |_cmd: &str,
                                _cwd: &Path,
                                _t: Option<Duration>|
         -> Result<BashExecutionOutcome, String> {
            Ok(BashExecutionOutcome {
                command: "fake".to_string(),
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                blocked_reason: None,
                interrupted: true,
            })
        };
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            interrupted_bash,
            auto_approver(),
        );
        assert!(
            matches!(
                outcome,
                TesterOutcome::Aborted(AbortReason::SmokeInterrupted(_))
            ),
            "expected SmokeInterrupted abort, got {outcome:?}"
        );
    }

    #[test]
    fn run_records_python_test_failure() {
        let work = tempdir().unwrap();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let outcome = run_tester_with_strategy(
            empty_run(work.path(), tmp.path(), runs.path()),
            python_candidate(),
            |_p| Ok(python_smoke_reply()),
            fail_bash(),
            auto_approver(),
        );
        match outcome {
            TesterOutcome::Recorded(frame) => {
                assert_eq!(frame.kind, FeedbackKind::TestFailure);
            }
            other => panic!("expected Recorded, got {other:?}"),
        }
    }

    #[test]
    fn run_records_rust_with_harness_cleanup() {
        let work_dir = rust_workspace_fixture();
        let tmp = tempdir().unwrap();
        let runs = tempdir().unwrap();
        let candidate = TesterCandidate::Rust {
            manifest_path: work_dir.path().join("Cargo.toml"),
            package_name: Some("fix".to_string()),
        };
        let outcome = run_tester_with_strategy(
            empty_run(work_dir.path(), tmp.path(), runs.path()),
            candidate,
            |_p| Ok(rust_smoke_reply()),
            ok_bash(),
            auto_approver(),
        );
        assert!(matches!(outcome, TesterOutcome::Recorded(_)));
        // tester-runs/<run_id>/ should have been cleaned up (best-effort).
        let entries: Vec<_> = std::fs::read_dir(runs.path())
            .map(|it| it.collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(entries.is_empty(), "tester-runs dir should be empty");
    }

    // ---- env helper ---------------------------------------------------------

    #[test]
    fn tester_disabled_unset_is_false() {
        assert!(!tester_disabled(|_| None));
    }

    // ---- check_invocation_gate (per-turn cap ordering, AC18 mirror) --------

    #[test]
    fn invocation_gate_open_when_all_clear() {
        assert_eq!(check_invocation_gate(false, false, false), None);
    }

    #[test]
    fn invocation_gate_per_turn_cap_dominates() {
        // Cap takes precedence over Plan / Disabled (DR1-004 ordering).
        assert_eq!(
            check_invocation_gate(true, true, true),
            Some(NotInvokedReason::PerTurnCapHit)
        );
    }

    #[test]
    fn invocation_gate_plan_mode_dominates_disabled() {
        assert_eq!(
            check_invocation_gate(false, true, true),
            Some(NotInvokedReason::PlanMode)
        );
    }

    #[test]
    fn invocation_gate_disabled_fires_when_alone() {
        assert_eq!(
            check_invocation_gate(false, false, true),
            Some(NotInvokedReason::Disabled)
        );
    }

    #[test]
    fn tester_disabled_empty_is_false() {
        assert!(!tester_disabled(|_| Some(String::new())));
    }

    #[test]
    fn tester_disabled_set_is_true() {
        assert!(tester_disabled(|_| Some("1".to_string())));
    }

    // ---- log sanitizer ------------------------------------------------------

    #[test]
    fn sanitize_masks_known_secret_tokens() {
        let raw = "OPENAI_API_KEY=sk-test12345abcdefghij1234567890";
        let out = sanitize_tester_log(raw, 1024);
        assert!(!out.contains("sk-test12345abcdefghij1234567890"));
    }

    #[test]
    fn sanitize_truncates_to_cap() {
        let raw = "x".repeat(50_000);
        let out = sanitize_tester_log(&raw, 100);
        assert!(out.starts_with(&"x".repeat(100)));
        assert!(out.ends_with("[truncated]"));
    }
}
