//! Issue #606 Phase α-1 — integration smoke tests for the post-hoc
//! completion-evidence pipeline.
//!
//! These tests reach into the `loop_run` facade via two `#[doc(hidden)] pub`
//! seams (`classify_repo_edit_path_for_test` / `is_completion_verifier_command_for_test`)
//! that re-export the SSOT helpers exactly as the production hooks see
//! them. The intent is to pin the externally-observable contract:
//!
//!   * Path classification follows the DR1-001 ordering (`.mdx` resolves to
//!     `docs`, not `impl`).
//!   * The DR4-002 security gate rejects shell-control-laundered exit codes
//!     before they can be promoted to `VerifierExitZero` evidence.
//!
//! Combined with the rich module-level unit suites in
//! `src/agent/loop_run/completion_evidence.rs` (13 tests),
//! `src/agent/loop_run/protocol.rs` (6 new tests for the accept/satisfy/missing
//! matrix), and `src/logging.rs` (1 new test for payload key-set preservation),
//! these integration tests cover AP-01〜AP-07 + VR-01〜VR-06 from the
//! work-plan (`dev-reports/issue/606/work-plan.md`).

use anvil::agent::loop_run::{
    classify_repo_edit_path_for_test, is_completion_verifier_command_for_test,
};
use std::path::Path;

// ----------------------- AP-01 / AP-02 / AP-03 / AP-04 (verifier accept) ----

/// AP-01 / AP-02 / AP-03 / AP-04 surface: a canonical build/test invocation
/// flows through the DR4-002 security gate cleanly so the post-hoc Bash
/// hook can promote it to `VerifierExitZero` evidence.
#[test]
fn verifier_invocations_pass_the_security_gate() {
    // AP-01 — cargo test
    assert!(is_completion_verifier_command_for_test("cargo test"));
    assert!(is_completion_verifier_command_for_test(
        "cargo test --workspace --all-targets"
    ));
    // AP-02 — pytest
    assert!(is_completion_verifier_command_for_test("pytest"));
    assert!(is_completion_verifier_command_for_test("pytest -q"));
    // AP-03 — npm test
    assert!(is_completion_verifier_command_for_test("npm test"));
    // AP-04 — cargo build
    assert!(is_completion_verifier_command_for_test("cargo build"));
    assert!(is_completion_verifier_command_for_test(
        "cargo build --release"
    ));
}

// ----------------------- AP-06 (DR4-002 security gate) ----------------------

/// AP-06 surface (combined with DR4-002): a verifier command containing
/// shell-control operators is rejected at the gate so the bash hook never
/// pushes a poisoned `VerifierExitZero` into `EvidenceSet`. Covers `||`,
/// `&&`, `;`, pipe, backtick, `$()`, redirect, newline.
#[test]
fn verifier_command_with_shell_control_operator_is_rejected() {
    // `cargo test || true` would exit 0 even on test failure — this is the
    // classic evidence-poisoning shape Issue #606 §12 calls out.
    assert!(!is_completion_verifier_command_for_test(
        "cargo test || true"
    ));
    // `pytest && false` masks the inner success.
    assert!(!is_completion_verifier_command_for_test("pytest && false"));
    // Sequential `;` lets a follow-on command produce the final `$?`.
    assert!(!is_completion_verifier_command_for_test("npm test ; true"));
    // Pipe / redirect can swallow the inner exit when `pipefail` is unset.
    assert!(!is_completion_verifier_command_for_test(
        "cargo test | tee out.log"
    ));
    assert!(!is_completion_verifier_command_for_test(
        "cargo test > out.log"
    ));
    // Command substitution can launder exits via the outer pipeline.
    assert!(!is_completion_verifier_command_for_test(
        "cargo test --test $(whoami)"
    ));
    assert!(!is_completion_verifier_command_for_test(
        "cargo test --test `whoami`"
    ));
    // Embedded newline / CR.
    assert!(!is_completion_verifier_command_for_test(
        "cargo test\necho bypass"
    ));
    // Background — `&` lets the foreground exit before the test finishes.
    assert!(!is_completion_verifier_command_for_test(
        "cargo test & echo bg"
    ));
}

// ----------------------- AP-05 (RepoEdit categories) ------------------------

/// AP-05 surface (and DR1-001): Edit/Write success paths flow through the
/// SSOT classifier exactly. `.mdx` resolves to `docs`, not `impl`, because
/// the Docs predicate runs before `is_implementation_file`.
#[test]
fn edit_write_paths_classify_via_ssot_predicates() {
    // Impl
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("src/main.rs")),
        "impl"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("src/components/Button.tsx")),
        "impl"
    );
    // Test
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("src/__tests__/foo.ts")),
        "test"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("tests/integration.rs")),
        "test"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("test_calc.py")),
        "test"
    );
    // Docs — `.mdx` must resolve to `docs` even though it lives in the impl
    // SSOT extension list. Pins DR1-001 ordering invariant.
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("docs/intro.mdx")),
        "docs"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("README.md")),
        "docs"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("CHANGELOG.rst")),
        "docs"
    );
    // Setup
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("package.json")),
        "setup"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("tsconfig.json")),
        "setup"
    );
    // Other — `Cargo.toml` is intentionally out of the setup SSOT (DR1-002).
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("Cargo.toml")),
        "other"
    );
    assert_eq!(
        classify_repo_edit_path_for_test(Path::new("Makefile")),
        "other"
    );
}

// ----------------------- VR-06 (Agent construction non-regression) -----------

/// VR-04 surface: the strict CB-001 gate accepts the common verifier
/// shapes that don't rely on shell control characters and rejects
/// quoted-operator forms (the old quote-skipping scanner accepted those,
/// but it could not be made safe against backslash escapes — see
/// `contains_evidence_poisoning_shell_control` doc and the
/// `cb_001_*` unit tests in `completion_evidence.rs`).
#[test]
fn verifier_gate_accepts_typical_verifier_invocations() {
    // Common option-only invocations have no shell-meta characters and
    // must be accepted so a successful `cargo test` / `pytest` still
    // contributes `VerifierExitZero` evidence.
    assert!(is_completion_verifier_command_for_test(
        "cargo test --workspace"
    ));
    assert!(is_completion_verifier_command_for_test(
        "cargo test foo::bar"
    ));
    assert!(is_completion_verifier_command_for_test(
        "pytest -k some_name"
    ));
    // Plain quoted filters with no shell-meta characters are still fine
    // because the deny list only triggers on actual control characters
    // (the quotes themselves are not banned, just no longer trusted to
    // shield interior `|` / `;` / etc. from the gate).
    assert!(is_completion_verifier_command_for_test(
        "pytest -k 'foo and not bar'"
    ));
    // Conversely, any operator inside quotes now disqualifies — the
    // gate is intentionally conservative because the cost of a false
    // positive (poisoned evidence) is higher than the cost of a false
    // negative (skipping an evidence push; the command itself still
    // executes through Bash).
    assert!(!is_completion_verifier_command_for_test(
        "cargo test -- --filter 'a | b'"
    ));
}

/// VR-06 surface: constructing a real `Agent` after wiring in the
/// per-turn `evidence_set_this_turn` field still succeeds and the
/// `current_turn_index` accessor / session field plumbing remains
/// functional. Smoke check: a no-Ollama `Agent::new` + immediate drop
/// must not panic.
#[test]
fn agent_construction_after_evidence_set_wiring_does_not_panic() {
    use anvil::agent::Agent;
    use anvil::agent::loop_run::FooterHandle;
    use anvil::config::Config;
    use anvil::model_registry::RuntimeModels;
    use anvil::ollama::client::OllamaClient;
    use anvil::session::store::{SessionSnapshot, SessionStore};
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join("test-606-agent-smoke")).unwrap();

    let mut config = Config::default();
    config.cwd = dir.path().to_path_buf();
    config.photon_enabled = false;
    config.requested_model = Some("test-model".to_string());
    config.state_dir_override = Some(state_root.clone());
    config.yes_mode = true;
    config.max_iterations = 1;

    let session = SessionSnapshot {
        id: "test-606-agent-smoke".to_string(),
        workspace_key: "anvil-606-smoke".to_string(),
        ..Default::default()
    };

    let _agent = Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:19999".to_string()).unwrap(),
        SessionStore::new(&state_root, "test-606-agent-smoke", "anvil-606-smoke"),
        session,
        FooterHandle::disabled(),
    );
    // Drop the agent; the test passes if the constructor + Drop pipeline
    // doesn't panic. The new `evidence_set_this_turn` field default-inits.
}

// ----------------------- Issue #608 Phase 2C (AP-08 hook regressions) ------

/// AP-08 hook regression: Bash tool output for a BuildTest class command
/// flows through the `test_output` formatter (FAILED summary + tail trim)
/// before the byte cap. Verifies the FAILED summary header is exposed at the
/// head of the body (after `exit_code=` metadata).
#[test]
fn test_output_formatter_pipeline_renders_failed_summary_head() {
    use anvil::tools::test_output::format_for_tool_result;
    // Synthetic pytest output where the FAILED line is buried in noise.
    let mut s = String::new();
    for i in 0..200 {
        s.push_str(&format!("noise line {i}\n"));
    }
    s.push_str("FAILED tests/test_a.py::test_x - boom\n");
    let formatted = format_for_tool_result(&s);
    assert!(
        formatted.starts_with("FAILED: 1 test(s)"),
        "FAILED header must be at head of formatted body: {:?}",
        &formatted[..50.min(formatted.len())]
    );
    assert!(formatted.contains("test_x"));
}

/// AP-08 leak regression: a verifier command that printed an Authorization
/// header to stderr must not surface that header through the formatter
/// output. mask_secrets (token + URL credential) is applied to the trimmed
/// body in `format_for_tool_result`.
#[test]
fn test_output_formatter_strips_url_credentials_from_body() {
    use anvil::tools::test_output::format_for_tool_result;
    let mut s = String::new();
    s.push_str("FAILED tests/test_a.py::test_x\n");
    for i in 0..50 {
        s.push_str(&format!("frame {i}\n"));
    }
    s.push_str("calling https://user:hunter2@api.example.com/x\n");
    let formatted = format_for_tool_result(&s);
    assert!(
        !formatted.contains("hunter2"),
        "URL userinfo must be redacted by formatter body mask: {:?}",
        formatted
    );
    assert!(
        formatted.starts_with("FAILED: 1 test(s)"),
        "summary header missing: {:?}",
        &formatted[..50.min(formatted.len())]
    );
}

/// CB-001 regression: header-family credentials (Authorization / Cookie /
/// X-API-Key / X-Auth-Token) printed in stderr of a failing verifier must
/// not surface through the formatter pipeline. The body redaction stacks
/// `mask_secrets` + `mask_header_family` so these header lines are
/// rewritten to `Header: <REDACTED>` before reaching the tool result.
#[test]
fn test_output_formatter_strips_header_family_credentials_from_body() {
    use anvil::tools::test_output::format_for_tool_result;
    let mut s = String::new();
    s.push_str("FAILED tests/test_a.py::test_x\n");
    for i in 0..50 {
        s.push_str(&format!("frame {i}\n"));
    }
    s.push_str("> Authorization: Bearer cb001_bearer_leak_value_AAA\n");
    s.push_str("> Cookie: session=cb001_cookie_leak_value_BBB; theme=dark\n");
    s.push_str("> X-API-Key: cb001_apikey_leak_value_CCC\n");
    s.push_str("> X-Auth-Token: cb001_xauthtoken_leak_value_DDD\n");
    let formatted = format_for_tool_result(&s);
    for leak in [
        "cb001_bearer_leak_value_AAA",
        "cb001_cookie_leak_value_BBB",
        "cb001_apikey_leak_value_CCC",
        "cb001_xauthtoken_leak_value_DDD",
    ] {
        assert!(
            !formatted.contains(leak),
            "raw header credential {leak} leaked into formatter output: {:?}",
            formatted
        );
    }
    assert!(
        formatted.contains("<REDACTED>"),
        "header redaction marker missing: {:?}",
        formatted
    );
    assert!(
        formatted.starts_with("FAILED: 1 test(s)"),
        "summary header missing: {:?}",
        &formatted[..50.min(formatted.len())]
    );
}

/// CB-001 regression: header-family credentials embedded INSIDE the FAILED
/// test name (parametrized test that captures a header value) must also
/// be redacted in the FAILED summary bullet line.
#[test]
fn test_output_formatter_strips_header_family_credentials_from_failed_name() {
    use anvil::tools::test_output::format_for_tool_result;
    let s = "FAILED tests/test_a.py::test[hdr=Authorization: Bearer cb001_in_name_value] - boom\n";
    let formatted = format_for_tool_result(s);
    assert!(
        !formatted.contains("cb001_in_name_value"),
        "raw bearer in failed-name leaked into formatter output: {:?}",
        formatted
    );
    assert!(formatted.contains("<REDACTED>"));
    assert!(formatted.starts_with("FAILED: 1 test(s)"));
}

// ----------------------- Issue #608 Phase 2H VR-coverage smoke -------------

/// Phase 2H VR-06: legacy session.json (no AP-09 fields) still
/// deserializes cleanly. Light cross-crate smoke that the SessionSnapshot
/// schema growth is forward-compatible.
#[test]
fn legacy_session_json_without_ap09_fields_deserializes_via_default() {
    use anvil::session::store::SessionSnapshot;
    let snap = SessionSnapshot::default();
    assert!(snap.last_verifier_command.is_none());
    assert!(snap.last_verifier_invocation.is_none());
    let json = serde_json::to_string(&snap).unwrap();
    // skip_serializing_if = "Option::is_none" — keys absent for None.
    assert!(!json.contains("\"last_verifier_command\""));
    assert!(!json.contains("\"last_verifier_invocation\""));
    let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert!(back.last_verifier_command.is_none());
}

/// Phase 2H VR-04: secret-bearing verifier command is redacted on save,
/// re-redacted on load (defense-in-depth). Smoke check at the
/// integration layer.
#[test]
fn verifier_command_persisted_with_secrets_is_redacted_round_trip() {
    use anvil::session::store::SessionSnapshot;
    let snap = SessionSnapshot {
        last_verifier_command: Some(
            "cargo test --env api_key=ghp_supersecretvalueABCDEFGHIJKLMNOP".to_string(),
        ),
        ..Default::default()
    };
    let json = serde_json::to_string(&snap).unwrap();
    assert!(!json.contains("ghp_supersecretvalueABCDEFGHIJKLMNOP"));
    let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
    let stored = back.last_verifier_command.unwrap();
    assert!(!stored.contains("ghp_supersecretvalueABCDEFGHIJKLMNOP"));
    assert!(stored.contains("***"));
}
