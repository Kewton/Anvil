//! Issue #459 Tester Skill v1 — Phase 2 E2E smoke suite.
//!
//! Drives `run_tester_with_strategy` (the closure-DI orchestrator from
//! `src/agent/loop_run/tester.rs`) end-to-end for the three v1 stacks
//! (Rust / Node / Python) plus the disable / cap regressions called out in
//! the design policy § 12-1 and Issue #459 acceptance criteria. The LLM call
//! and the bash runner are both injected via closures so the suite never
//! depends on Ollama, `cargo`, `node`, or `python` being installed in CI —
//! the entire run completes in well under 5 seconds.
//!
//! The fixture layout lives under `tests/fixtures/stacks/{rust,node,python}/`.
//! The root `Cargo.toml` `[workspace] exclude = ["tests/fixtures"]` keeps
//! the Rust fixture from being pulled into the Anvil workspace at build
//! time. Each test clones the relevant fixture into a TempDir before
//! exercising any I/O so the on-disk fixture stays read-only.
//!
//! NOTE: tests follow the `tests/tmp_tests_e2e.rs` convention — top-level
//! integration test (no `tests/integration/` subdirectory) and they exercise
//! the public API surface only.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anvil::agent::loop_run::{
    MAX_TESTER_LLM_REPLY_BYTES, TESTER_MAX_GENERATED_TESTS_PER_TURN, TesterAbortReason,
    TesterApprovalMode, TesterCandidate, TesterLlmError, TesterNotInvokedReason, TesterOutcome,
    TesterPrompt, TesterRun, run_tester_with_strategy, tester_disabled, tester_gate,
};
use anvil::session::feedback::FeedbackKind;
use anvil::tools::bash::BashExecutionOutcome;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("stacks")
}

/// Recursively copy a fixture tree into a TempDir-backed `dst`. Read-only on
/// the source side: every test clones rather than mutating the fixture.
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst dir");
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let rel = entry.file_name();
        let target = dst.join(&rel);
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).expect("copy fixture file");
        }
    }
}

struct StackFixture {
    _state_root_keep: TempDir,
    _workspace_keep: TempDir,
    workspace_root: PathBuf,
    tmp_tests_root: PathBuf,
    tester_runs_root: PathBuf,
}

fn make_stack_fixture(stack_name: &str) -> StackFixture {
    let state = TempDir::new().expect("temp state");
    let workspace = TempDir::new().expect("temp workspace");
    let session_dir = state.path().join("sessions").join("session-test-id");
    let tmp_tests_root = session_dir.join("tmp-tests");
    let tester_runs_root = session_dir.join("tester-runs");
    fs::create_dir_all(&tmp_tests_root).unwrap();
    fs::create_dir_all(&tester_runs_root).unwrap();

    let src = fixtures_dir().join(stack_name);
    copy_tree(&src, workspace.path());

    StackFixture {
        workspace_root: workspace.path().to_path_buf(),
        tmp_tests_root,
        tester_runs_root,
        _state_root_keep: state,
        _workspace_keep: workspace,
    }
}

fn run_for<'a>(fx: &'a StackFixture) -> TesterRun<'a> {
    TesterRun {
        work_root: &fx.workspace_root,
        tmp_tests_root: &fx.tmp_tests_root,
        tester_runs_root: &fx.tester_runs_root,
        approval_mode: TesterApprovalMode::Auto,
        plan_mode: false,
        no_tester_env: false,
        session_id: std::borrow::Cow::Borrowed("session-test-id"),
    }
}

fn ok_bash_outcome(stdout: &str) -> BashExecutionOutcome {
    BashExecutionOutcome {
        command: "fake".to_string(),
        exit_code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
        blocked_reason: None,
        interrupted: false,
    }
}

fn auto_approver() -> impl FnOnce(TesterApprovalMode, &[String]) -> Result<(), TesterAbortReason> {
    |_m, _c| Ok(())
}

fn rust_smoke_reply() -> String {
    // Note: do not embed `#[test]` literally in a raw string here — Rust 2021
    // reserves the `r#"#"` prefix space awkwardly. Use double-escaped JSON.
    "{\"test_files\":[{\"relative_path\":\"smoke_basic.rs\",\
        \"content\":\"fn smoke() { assert_eq!(1, 1); }\\n\"}]}"
        .to_string()
}

fn node_smoke_reply() -> String {
    r#"{"test_files":[{"relative_path":"smoke_basic.js","content":"console.log('ok');\n"}]}"#
        .to_string()
}

fn python_smoke_reply() -> String {
    r#"{"test_files":[{"relative_path":"smoke_basic.py","content":"print('ok')\n"}]}"#.to_string()
}

// ---------------------------------------------------------------------------
// Case 1: Rust smoke E2E (tmp-tests/files/<rel> + tester-runs/<id>/ harness +
//         TestPass FeedbackFrame + tester-runs cleanup)
// ---------------------------------------------------------------------------

#[test]
fn rust_smoke_records_test_pass_and_cleans_up_harness() {
    let fx = make_stack_fixture("rust");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &[]).expect("rust candidate");
    assert!(matches!(candidate, TesterCandidate::Rust { .. }));

    let mut bash_invocations = 0usize;
    let mut captured_cmd = String::new();
    let mut captured_timeout: Option<Duration> = None;
    let run_bash = |cmd: &str, _cwd: &Path, t: Option<Duration>| {
        bash_invocations += 1;
        captured_cmd = cmd.to_string();
        captured_timeout = t;
        Ok(ok_bash_outcome(
            "running 1 test\ntest smoke ... ok\n\ntest result: ok. 1 passed; 0 failed\n",
        ))
    };

    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p: &TesterPrompt| Ok(rust_smoke_reply()),
        run_bash,
        auto_approver(),
    );

    match outcome {
        TesterOutcome::Recorded(frame) => {
            assert_eq!(
                frame.kind,
                FeedbackKind::TestPass,
                "kind should be TestPass"
            );
        }
        other => panic!("expected Recorded(TestPass), got {other:?}"),
    }
    assert_eq!(bash_invocations, 1, "smoke runner invoked exactly once");
    // CB-005 (Issue #459): the Rust template now wraps `cargo test ...` with
    // `env CARGO_TARGET_DIR=<run_dir>/target` so build artefacts never leak
    // into the workspace `target/`.
    assert!(
        captured_cmd.starts_with("env CARGO_TARGET_DIR="),
        "Rust smoke command should start with env CARGO_TARGET_DIR=, got: {captured_cmd}"
    );
    assert!(
        captured_cmd.contains(" cargo test --manifest-path"),
        "Rust smoke command should still invoke cargo test, got: {captured_cmd}"
    );
    assert!(
        captured_cmd.contains("tester-runs/") && captured_cmd.contains("/target"),
        "CARGO_TARGET_DIR must be inside tester-runs/<run_id>/target, got: {captured_cmd}"
    );
    assert!(
        !captured_cmd.contains("tmp-tests/files/"),
        "Rust command must not reference tmp-tests/files literal (DR4 security)"
    );
    assert_eq!(
        captured_timeout,
        Some(Duration::from_secs(30)),
        "explicit_timeout must be the canonical TESTER_SMOKE_TIMEOUT_SECS"
    );

    // tmp-tests/files/<rel> body landed under the session-scoped root.
    assert!(
        fx.tmp_tests_root.join("files/smoke_basic.rs").is_file(),
        "smoke body should be saved under tmp-tests/files/"
    );

    // tester-runs/<run_id>/ best-effort cleanup happened.
    let entries: Vec<_> = fs::read_dir(&fx.tester_runs_root)
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    assert!(
        entries.is_empty(),
        "tester-runs/ should be empty after Recorded; leftover entries: {entries:?}"
    );
}

// ---------------------------------------------------------------------------
// Case 2: Node smoke E2E (tmp-tests/files/<rel> + node --check exit 0 →
//         BuildPass FeedbackFrame)
// ---------------------------------------------------------------------------

#[test]
fn node_smoke_records_build_pass() {
    let fx = make_stack_fixture("node");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &[]).expect("node candidate");
    assert!(matches!(candidate, TesterCandidate::Node { .. }));

    let mut captured_cmd = String::new();
    let run_bash = |cmd: &str, _cwd: &Path, _t: Option<Duration>| {
        captured_cmd = cmd.to_string();
        Ok(ok_bash_outcome(""))
    };

    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p: &TesterPrompt| Ok(node_smoke_reply()),
        run_bash,
        auto_approver(),
    );

    match outcome {
        TesterOutcome::Recorded(frame) => {
            assert_eq!(frame.kind, FeedbackKind::BuildPass);
        }
        other => panic!("expected Recorded(BuildPass), got {other:?}"),
    }
    assert!(
        captured_cmd.starts_with("node --check"),
        "Node smoke command should be `node --check ...`, got: {captured_cmd}"
    );
    // No npx / install / network in the Node template (AC20).
    assert!(
        !captured_cmd.contains("npx") && !captured_cmd.contains("install"),
        "Node template must not invoke npx / install"
    );
    assert!(
        fx.tmp_tests_root.join("files/smoke_basic.js").is_file(),
        "smoke body should be saved under tmp-tests/files/"
    );
}

// ---------------------------------------------------------------------------
// Case 3: Python smoke E2E (tmp-tests/files/<rel> + py_compile exit 0 →
//         TestPass FeedbackFrame, NOT BuildPass — DR1-010)
// ---------------------------------------------------------------------------

#[test]
fn python_smoke_records_test_pass() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    assert!(matches!(candidate, TesterCandidate::Python { .. }));

    let mut captured_cmd = String::new();
    let run_bash = |cmd: &str, _cwd: &Path, _t: Option<Duration>| {
        captured_cmd = cmd.to_string();
        Ok(ok_bash_outcome(""))
    };

    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p: &TesterPrompt| Ok(python_smoke_reply()),
        run_bash,
        auto_approver(),
    );

    match outcome {
        TesterOutcome::Recorded(frame) => {
            assert_eq!(frame.kind, FeedbackKind::TestPass);
        }
        other => panic!("expected Recorded(TestPass), got {other:?}"),
    }
    assert!(
        captured_cmd.starts_with("python3 -m py_compile"),
        "Python smoke command should be `python3 -m py_compile ...`, got: {captured_cmd}"
    );
    assert!(
        fx.tmp_tests_root.join("files/smoke_basic.py").is_file(),
        "smoke body should be saved under tmp-tests/files/"
    );
}

// ---------------------------------------------------------------------------
// Case 4: Plan-mode regression — Tester must not invoke (NotInvoked(PlanMode))
// ---------------------------------------------------------------------------

#[test]
fn plan_mode_blocks_tester_invocation() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");

    let mut run = run_for(&fx);
    run.plan_mode = true;
    // The closures should NOT be reached; assert via panic if they are.
    let outcome = run_tester_with_strategy(
        run,
        candidate,
        |_p| -> Result<String, TesterLlmError> { panic!("LLM closure must not fire in Plan mode") },
        |_cmd, _cwd, _t| -> Result<BashExecutionOutcome, String> {
            panic!("Bash closure must not fire in Plan mode")
        },
        |_m, _c| -> Result<(), TesterAbortReason> { panic!("Approver must not fire in Plan mode") },
    );

    assert!(matches!(
        outcome,
        TesterOutcome::NotInvoked(TesterNotInvokedReason::PlanMode)
    ));
    // tmp-tests/files/ must remain empty.
    assert!(
        !fx.tmp_tests_root.join("files/smoke_basic.py").exists(),
        "no smoke body should land on disk in Plan mode"
    );
}

// ---------------------------------------------------------------------------
// Case 5: ANVIL_NO_TESTER opt-out (NotInvoked(Disabled))
// ---------------------------------------------------------------------------

#[test]
fn anvil_no_tester_env_disables_tester() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");

    // Drive the run with `no_tester_env: true` to mirror the production
    // wiring (env-read happens in turn.rs::try_invoke_tester via
    // `tester_disabled`). This avoids mutating the process env, which would
    // be racy against parallel tests.
    let mut run = run_for(&fx);
    run.no_tester_env = true;
    let outcome = run_tester_with_strategy(
        run,
        candidate,
        |_p| -> Result<String, TesterLlmError> {
            panic!("LLM closure must not fire when disabled")
        },
        |_cmd, _cwd, _t| -> Result<BashExecutionOutcome, String> {
            panic!("Bash closure must not fire when disabled")
        },
        |_m, _c| -> Result<(), TesterAbortReason> {
            panic!("Approver must not fire when disabled")
        },
    );

    assert!(matches!(
        outcome,
        TesterOutcome::NotInvoked(TesterNotInvokedReason::Disabled)
    ));

    // Cross-check the env-helper itself (DR2-017).
    assert!(!tester_disabled(|_| None));
    assert!(!tester_disabled(|_| Some(String::new())));
    assert!(tester_disabled(|_| Some("1".to_string())));
}

// ---------------------------------------------------------------------------
// Case 6: per-turn cap — second invocation in the same turn returns
//         NotInvoked(PerTurnCapHit) (DR1-004)
// ---------------------------------------------------------------------------

#[test]
fn per_turn_cap_blocks_second_invocation() {
    // First call: gate is open, so dispatch proceeds and consumes the cap.
    assert_eq!(
        tester_gate(false, false, false),
        None,
        "fresh turn must not be gated"
    );

    // After the orchestrator records / aborts, the caller flips
    // `tester_called_this_turn = true`. Mirror that and re-check the gate.
    let second = tester_gate(true, false, false);
    assert_eq!(
        second,
        Some(TesterNotInvokedReason::PerTurnCapHit),
        "second invocation in the same turn must be capped"
    );

    // Verify ordering: per-turn cap dominates Plan mode and ANVIL_NO_TESTER.
    assert_eq!(
        tester_gate(true, true, true),
        Some(TesterNotInvokedReason::PerTurnCapHit),
        "per-turn cap must take precedence over Plan / Disabled"
    );
    assert_eq!(
        tester_gate(false, true, true),
        Some(TesterNotInvokedReason::PlanMode),
        "Plan mode must take precedence over Disabled when not capped"
    );
    assert_eq!(
        tester_gate(false, false, true),
        Some(TesterNotInvokedReason::Disabled),
        "Disabled fires when neither cap nor Plan mode is set"
    );
}

// ---------------------------------------------------------------------------
// Case 7: malformed LLM replies are rejected and never write to tmp-tests
// ---------------------------------------------------------------------------

#[test]
fn empty_llm_reply_is_aborted() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p| Ok(String::new()),
        |_c, _cwd, _t| panic!("Bash must not fire on malformed reply"),
        auto_approver(),
    );
    assert!(matches!(
        outcome,
        TesterOutcome::Aborted(TesterAbortReason::LlmMalformed(_))
    ));
    assert!(
        !fx.tmp_tests_root
            .join("files")
            .join("smoke_basic.py")
            .exists(),
        "no smoke body on empty reply"
    );
}

#[test]
fn think_only_llm_reply_is_aborted() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p| Ok("<think>only thinking, no output</think>".to_string()),
        |_c, _cwd, _t| panic!("Bash must not fire"),
        auto_approver(),
    );
    assert!(matches!(
        outcome,
        TesterOutcome::Aborted(TesterAbortReason::LlmMalformed(_))
    ));
}

#[test]
fn oversized_llm_reply_is_aborted() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    let big = "x".repeat(MAX_TESTER_LLM_REPLY_BYTES + 1);
    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p| Ok(big),
        |_c, _cwd, _t| panic!("Bash must not fire"),
        auto_approver(),
    );
    assert!(matches!(
        outcome,
        TesterOutcome::Aborted(TesterAbortReason::LlmMalformed(_))
    ));
}

#[test]
fn multi_file_llm_reply_is_aborted() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    let body = r#"{"test_files":[
        {"relative_path":"a.py","content":"print('a')\n"},
        {"relative_path":"b.py","content":"print('b')\n"}
    ]}"#;
    assert_eq!(
        TESTER_MAX_GENERATED_TESTS_PER_TURN, 1,
        "this test pins the per-turn 1-file cap"
    );
    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p| Ok(body.to_string()),
        |_c, _cwd, _t| panic!("Bash must not fire on multi-file reply"),
        auto_approver(),
    );
    assert!(matches!(
        outcome,
        TesterOutcome::Aborted(TesterAbortReason::LlmMalformed(_))
    ));
    assert!(
        !fx.tmp_tests_root.join("files/a.py").exists()
            && !fx.tmp_tests_root.join("files/b.py").exists(),
        "no smoke body should land when the reply is rejected"
    );
}

#[test]
fn out_of_tmp_tests_path_is_aborted() {
    let fx = make_stack_fixture("python");
    let candidate = TesterCandidate::detect(&fx.workspace_root, &["main.py".to_string()])
        .expect("python candidate");
    let body = r#"{"test_files":[{"relative_path":"../escape.py","content":"x"}]}"#;
    let outcome = run_tester_with_strategy(
        run_for(&fx),
        candidate,
        |_p| Ok(body.to_string()),
        |_c, _cwd, _t| panic!("Bash must not fire on path-confinement violation"),
        auto_approver(),
    );
    assert!(matches!(
        outcome,
        TesterOutcome::Aborted(TesterAbortReason::PathConfinementViolation(_))
    ));
    // Nothing should land outside tmp-tests/files/.
    assert!(
        !fx.tmp_tests_root
            .parent()
            .unwrap()
            .join("escape.py")
            .exists()
    );
}

// ---------------------------------------------------------------------------
// Case 8: full-suite runtime under the 5-second budget called out by the
// design policy / work plan. We measure across the cases we have already
// validated above; the timer is generous so it still passes on slow CI.
// ---------------------------------------------------------------------------

#[test]
fn full_e2e_runtime_is_well_under_five_seconds() {
    let started = Instant::now();
    // Re-drive the three happy-path cases inline to bound the per-test
    // budget without depending on the test harness's parallel scheduler.
    for (stack_name, reply, expected_kind) in [
        ("rust", rust_smoke_reply(), FeedbackKind::TestPass),
        ("node", node_smoke_reply(), FeedbackKind::BuildPass),
        ("python", python_smoke_reply(), FeedbackKind::TestPass),
    ] {
        let fx = make_stack_fixture(stack_name);
        let changed = if stack_name == "python" {
            vec!["main.py".to_string()]
        } else {
            vec![]
        };
        let candidate = TesterCandidate::detect(&fx.workspace_root, &changed).expect("candidate");
        let bash_stdout = if stack_name == "rust" {
            "test result: ok. 1 passed; 0 failed\n"
        } else {
            ""
        };
        let outcome = run_tester_with_strategy(
            run_for(&fx),
            candidate,
            move |_p| Ok(reply.clone()),
            move |_c, _cwd, _t| Ok(ok_bash_outcome(bash_stdout)),
            auto_approver(),
        );
        match outcome {
            TesterOutcome::Recorded(frame) => assert_eq!(frame.kind, expected_kind),
            other => panic!("expected Recorded({expected_kind:?}), got {other:?}"),
        }
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "Tester E2E suite should complete well under 5s, took {elapsed:?}"
    );
}
