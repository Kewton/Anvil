//! Issue #607 — integration smoke for env-setup commands as completion evidence.
//!
//! BP-04 / BP-06 / VR-β-05 are validated end-to-end at the
//! `BashExecutionOutcome` boundary: the real `tools::bash::run_with_outcome`
//! runner drives a temp workspace with a `PATH`-shadowed `npm` stub so we
//! exercise the actual classifier + outcome shape without depending on Node
//! / network. The pure `build_verifier_exit_zero_evidence_for_test` seam
//! then verifies how the agent observation hook would react to the outcome.
//!
//! These tests intentionally avoid spinning up a full Agent loop / Ollama
//! mock — the agent-level wiring is exercised by the unit suites in
//! `src/agent/loop_run/{turn,success,protocol,quality}.rs`. The integration
//! surface here only needs to pin:
//!
//!   * BP-04: a real `npm install` outcome flows through the classifier and
//!     yields `VerifierExitZero { class: env_setup, .. }` evidence.
//!   * BP-06: a real `npm install` failure outcome does NOT yield evidence
//!     and the `BashExecutionOutcome.exit_code != 0` propagates so a higher
//!     layer can build a feedback frame.
//!   * BP-07: the snake_case `command_class` label is emitted by the
//!     promotion helper (`env_setup`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

use anvil::agent::loop_run::build_verifier_exit_zero_evidence_for_test;
use anvil::tools::bash::{
    BashCommandClass, BashExecutionOutcome, classify_command, run_with_outcome,
};
use tempfile::tempdir;

/// Mutex serialising `PATH` mutations across tests in this file.
/// `run_with_outcome` inherits the parent process env (`BashEnvPolicy::Inherit`),
/// so two parallel tests racing on `PATH` would see each other's stubs.
static PATH_GUARD: Mutex<()> = Mutex::new(());

/// macOS CI runners use `sh -lc` + `/usr/libexec/path_helper` which rewrites
/// `PATH` from `/etc/paths`, dropping our prepended `stub_dir` and resolving
/// the pre-installed runner `npm` instead. The classifier and outcome shape
/// are platform-independent and fully covered by the unit suites in
/// `src/tools/bash.rs` and `src/agent/loop_run/turn.rs`; Linux CI exercises
/// the full E2E with the `PATH` stub honored.
///
/// Runtime skip (not `#[cfg_attr(..., ignore)]`) because the cfg_attr does
/// not consistently take effect on GitHub Actions macos-latest under
/// `cargo test --all`.
fn skip_on_macos_ci() -> bool {
    cfg!(target_os = "macos") && std::env::var("CI").is_ok()
}

/// Write a shell stub at `<dir>/<name>` that exits with `exit_code` and
/// echoes the args so the test can confirm it actually ran.
fn install_stub(dir: &std::path::Path, name: &str, exit_code: i32) {
    let path = dir.join(name);
    let script =
        format!("#!/bin/sh\necho 'stub {name} called with args:' \"$@\"\nexit {exit_code}\n");
    fs::write(&path, script).expect("write stub");
    let mut perms = fs::metadata(&path).expect("perms").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod stub");
}

/// Run `command` against `cwd` with the stub dir prepended to `PATH` for
/// the duration of the call. Holds `PATH_GUARD` for the dispatch so no
/// other test mutates `PATH` mid-flight.
fn run_with_stub_dir_on_path(
    command: &str,
    cwd: &std::path::Path,
    stub_dir: &std::path::Path,
) -> (String, BashExecutionOutcome) {
    let _guard = PATH_GUARD.lock().expect("path guard");
    let previous = std::env::var_os("PATH");
    let combined = match &previous {
        Some(prev) => format!("{}:{}", stub_dir.display(), prev.to_string_lossy()),
        None => stub_dir.display().to_string(),
    };
    // SAFETY: env::set_var / remove_var are unsafe in the 2024 edition.
    // Serialised by `PATH_GUARD` and immediately reverted below.
    unsafe { std::env::set_var("PATH", &combined) };
    let result = run_with_outcome(command, cwd, None, false, None, None);
    match &previous {
        Some(prev) => unsafe { std::env::set_var("PATH", prev) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    result.expect("bash dispatch")
}

/// VR-β-05 (1) — setup → test flow: a real `npm install` exit-0 outcome
/// classifies as `EnvSetup` and is promoted to `VerifierExitZero` evidence
/// with the `env_setup` class label (BP-01 / BP-07).
///
/// Skipped on macOS CI: `run_with_outcome` dispatches via `sh -lc` (login
/// shell), which on macOS sources `/etc/profile` and runs
/// `/usr/libexec/path_helper`. `path_helper` rewrites `PATH` from
/// `/etc/paths`, dropping our prepended `stub_dir` and resolving the
/// pre-installed runner `npm` instead. The classifier and outcome shape
/// are platform-independent and fully covered by the unit suites in
/// `src/tools/bash.rs` and `src/agent/loop_run/turn.rs`; Linux CI exercises
/// the full E2E with the `PATH` stub honored.
#[test]
fn npm_install_success_yields_env_setup_completion_evidence() {
    if skip_on_macos_ci() {
        return;
    }
    let workspace = tempdir().expect("workspace");
    let stub_dir = workspace.path().join("bin");
    fs::create_dir_all(&stub_dir).expect("bin dir");
    install_stub(&stub_dir, "npm", 0);

    let (text, outcome) = run_with_stub_dir_on_path("npm install", workspace.path(), &stub_dir);
    assert_eq!(outcome.exit_code, Some(0), "stub returned 0: {text}");
    assert_eq!(
        classify_command(&outcome.command),
        BashCommandClass::EnvSetup
    );
    assert_eq!(outcome.class, BashCommandClass::EnvSetup);

    let (command, class_label) = build_verifier_exit_zero_evidence_for_test(&outcome)
        .expect("EnvSetup success must produce verifier evidence");
    assert_eq!(class_label, "env_setup", "snake_case label for BP-07");
    assert!(
        command.contains("npm install"),
        "stored command preserved: {command}"
    );
}

/// VR-β-05 (2) — install failure: exit_code != 0 means no completion
/// evidence is promoted. Higher layers can build a `FeedbackFrame` from the
/// non-zero outcome so the model receives the failure (BP-06).
#[test]
fn npm_install_failure_does_not_yield_completion_evidence() {
    if skip_on_macos_ci() {
        return;
    }
    let workspace = tempdir().expect("workspace");
    let stub_dir = workspace.path().join("bin");
    fs::create_dir_all(&stub_dir).expect("bin dir");
    install_stub(&stub_dir, "npm", 1);

    let (text, outcome) = run_with_stub_dir_on_path("npm install", workspace.path(), &stub_dir);
    assert_eq!(outcome.exit_code, Some(1), "stub returned 1: {text}");
    assert_eq!(outcome.class, BashCommandClass::EnvSetup);

    let promoted = build_verifier_exit_zero_evidence_for_test(&outcome);
    assert!(
        promoted.is_none(),
        "EnvSetup failure must NOT push verifier evidence: {promoted:?}"
    );
    // The outcome's failure is still visible to the caller so the feedback
    // pipeline (build_feedback_for_bash) can return a non-None frame.
    assert!(outcome.exit_code != Some(0));
}

/// BP-04 — setup followed by test: separate Bash dispatches each produce
/// the correct class + evidence shape. Agent loop dispatches them as
/// separate tool calls in a single turn (compound commands carry shell
/// control operators which disqualify EnvSetup classification by design).
#[test]
fn install_then_test_flow_promotes_env_setup_evidence() {
    if skip_on_macos_ci() {
        return;
    }
    let workspace = tempdir().expect("workspace");
    let stub_dir = workspace.path().join("bin");
    fs::create_dir_all(&stub_dir).expect("bin dir");
    install_stub(&stub_dir, "npm", 0);

    let (_, install_outcome) =
        run_with_stub_dir_on_path("npm install", workspace.path(), &stub_dir);
    assert_eq!(install_outcome.class, BashCommandClass::EnvSetup);
    let (_, install_class) =
        build_verifier_exit_zero_evidence_for_test(&install_outcome).expect("env_setup evidence");
    assert_eq!(install_class, "env_setup");

    // Now run a BuildTest-classified command. `npm test` is on the
    // `is_build_test_command` SSOT list; the stub returns 0 so the
    // promotion gate accepts it.
    install_stub(&stub_dir, "npm", 0);
    let (_, test_outcome) = run_with_stub_dir_on_path("npm test", workspace.path(), &stub_dir);
    assert_eq!(test_outcome.class, BashCommandClass::BuildTest);
    let (_, test_class) =
        build_verifier_exit_zero_evidence_for_test(&test_outcome).expect("build_test evidence");
    assert_eq!(test_class, "build_test");
}
