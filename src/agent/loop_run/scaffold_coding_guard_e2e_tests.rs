//! Issue #924 (P7) — scaffold coding-guard E2E suite (in-crate `#[cfg(test)]`).
//!
//! Pattern lifted from `data_capability_e2e_tests.rs` / `safe_stop_e2e_tests.rs`
//! (CB-001 / DR3-001 precedent): an in-crate `#[cfg(test)] mod` that drives the
//! PRODUCTION scaffold / manifest gate without Ollama or any process spawn.
//! Agent-backed cases use the same `commands::test_agent_with_config` harness as
//! the existing scaffold tests; manifest / truth-table cases drive the `verifier`
//! spine directly. The production binary excludes this module.
//!
//! The gate predicate `scaffold_allowed_for_active_task(agent)` is `true` for
//! coding tasks, so every gated path is byte-identical for coding (the positive
//! cases pin that) and demoted only for the non-coding `TaskKind`s (Docs / Data /
//! Research / Ops / Authoring).

use super::scaffold_pipeline::{
    maybe_apply_deterministic_edit_after_format_error, maybe_apply_deterministic_quality_fallback,
    maybe_materialize_task_contract_fallback, mode_deterministic_scaffold_spec,
    scaffold_allowed_for_active_task,
};
use super::task_contract::{ArtifactRole, CompletionDecision, DeliverableObligation, TaskKind};
use super::verifier::{Verifier, VerifierArtifact, VerifierDiagnosticCode, capability_for};
use crate::agent::loop_run::commands::test_agent_with_config;
use crate::config::{Config, DeterministicFallbackMode};
use crate::modes::plan_act::WorkMode;
use crate::ollama::xml_fallback::ToolCall;
use crate::session::store::ConversationMessage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a test agent whose deterministic fallback is fully enabled and whose
/// specialized (python/fastapi) fallback flag matches `specialized`.
fn agent_with_request(
    request: &str,
    work_mode: WorkMode,
    specialized: bool,
) -> (super::Agent, tempfile::TempDir) {
    let cfg = Config {
        deterministic_fallback: DeterministicFallbackMode::FullTemplate,
        experimental_specialized_fallback: specialized,
        ..Config::default()
    };
    let (mut agent, temp) = test_agent_with_config(cfg);
    agent.session.mode_state.work_mode = work_mode;
    agent
        .session
        .messages
        .push(ConversationMessage::user(request.to_string()));
    (agent, temp)
}

/// Read the memoized classification kind through the production authority.
fn classified_kind(agent: &super::Agent) -> TaskKind {
    super::task_classification::task_contract_authority(agent)
        .map(|contract| contract.task_kind)
        .expect("active request must classify")
}

// ---------------------------------------------------------------------------
// Case 7 — `AcceptancePolicy::is_coding()` truth table (Coding=true, other 5=false).
// (Driven first: it is the predicate the whole gate is built on.)
// ---------------------------------------------------------------------------

#[test]
fn is_coding_truth_table() {
    assert!(capability_for(TaskKind::Coding).is_coding());
    for kind in [
        TaskKind::Docs,
        TaskKind::Data,
        TaskKind::Research,
        TaskKind::Ops,
        TaskKind::Authoring,
    ] {
        assert!(
            !capability_for(kind).is_coding(),
            "{kind:?} must not be coding"
        );
    }
    // `is_coding` and `allows_process_exec` share today's truth table but are
    // independent decision axes (scaffold materialize vs verifier spawn). Pin the
    // current coincidence so an accidental divergence is caught.
    assert_eq!(
        capability_for(TaskKind::Coding).is_coding(),
        capability_for(TaskKind::Coding).allows_process_exec()
    );
}

// ---------------------------------------------------------------------------
// Case 1 — coding task reaches a coding-shaped scaffold spec (existing behavior).
// ---------------------------------------------------------------------------

#[test]
fn coding_task_reaches_coding_shaped_scaffold_spec() {
    let (agent, _temp) = agent_with_request(
        "Create a Rust CLI word counter with tests",
        WorkMode::GenericCode,
        false,
    );
    assert_eq!(classified_kind(&agent), TaskKind::Coding);
    assert!(
        scaffold_allowed_for_active_task(&agent),
        "coding task must allow scaffold"
    );

    let policy = agent.session.mode_state.policy();
    let spec = mode_deterministic_scaffold_spec(
        &agent,
        "Create a Rust CLI word counter with tests",
        &policy,
    )
    .expect("coding request must yield a project-skeleton scaffold spec");
    assert_eq!(spec.scaffold_kind, "Rust CLI");
}

// ---------------------------------------------------------------------------
// Case 2 — non-coding kinds do NOT reach a coding-shaped scaffold spec.
//
// We pick a request that would otherwise yield a Rust project skeleton, but
// pair it with a non-coding classification (Data / Docs / Research / Ops /
// Authoring). The branch-local gate in `mode_deterministic_scaffold_spec` must
// skip the project-skeleton (and python) branches and fall through (to the Docs
// branch if the policy allows it, else `None`).
// ---------------------------------------------------------------------------

#[test]
fn non_coding_kinds_do_not_reach_coding_shaped_scaffold() {
    // (request, work_mode, expected non-coding kind)
    let fixtures: &[(&str, WorkMode, TaskKind)] = &[
        (
            "Generate report output.csv from the input data",
            WorkMode::Python,
            TaskKind::Data,
        ),
        (
            "Translate README.ja.md into English and write README.md",
            WorkMode::Docs,
            TaskKind::Authoring,
        ),
        (
            "Research and compare local LLM options, include sources and a recommendation",
            WorkMode::Docs,
            TaskKind::Research,
        ),
        (
            "Prepare a deployment runbook checklist with rollback steps",
            WorkMode::Docs,
            TaskKind::Ops,
        ),
    ];

    for (request, work_mode, expected) in fixtures {
        let (agent, _temp) = agent_with_request(request, *work_mode, true);
        assert_eq!(
            classified_kind(&agent),
            *expected,
            "fixture must classify as {expected:?}: {request}"
        );
        assert!(
            !scaffold_allowed_for_active_task(&agent),
            "non-coding {expected:?} must be demoted out of scaffold: {request}"
        );

        let policy = agent.session.mode_state.policy();
        // A Rust-skeleton-shaped request must NOT yield a coding scaffold for a
        // non-coding kind. (`Python` policy may still fall through to None; `Docs`
        // policy may fall through to a Docs spec — never a coding-shaped one.)
        let spec = mode_deterministic_scaffold_spec(
            &agent,
            "Create a Rust CLI word counter with tests",
            &policy,
        );
        if let Some(spec) = spec {
            assert_eq!(
                spec.scaffold_kind, "Docs",
                "non-coding {expected:?} must only ever fall through to a Docs scaffold, got {:?}",
                spec.scaffold_kind
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Case 3 — Docs + WorkMode::Docs still reaches the Docs deterministic scaffold
// (the fall-through pin: the gate demotes coding-shaped branches, not Docs).
// ---------------------------------------------------------------------------

#[test]
fn docs_kind_still_reaches_docs_scaffold_fall_through() {
    let request = "READMEを作成してください。";
    let (agent, _temp) = agent_with_request(request, WorkMode::Docs, false);
    assert_eq!(
        classified_kind(&agent),
        TaskKind::Docs,
        "fixture must classify as Docs"
    );
    assert!(
        !scaffold_allowed_for_active_task(&agent),
        "Docs is non-coding, so the coding-shaped branches are gated off"
    );

    let policy = agent.session.mode_state.policy();
    assert!(
        policy.allow_docs_deterministic_fallback,
        "WorkMode::Docs must enable the docs deterministic fallback"
    );
    let spec = mode_deterministic_scaffold_spec(&agent, request, &policy)
        .expect("Docs branch must still fire (fall-through pin)");
    assert_eq!(
        spec.scaffold_kind, "Docs",
        "Docs deterministic scaffold must remain reachable for Docs kind"
    );
}

// ---------------------------------------------------------------------------
// Case 4 — task-contract FastAPI positive regression: a coding FastAPI build
// with `WorkMode::Python` + specialized fallback + `Continue` still materializes.
// ---------------------------------------------------------------------------

#[test]
fn coding_fastapi_task_contract_fallback_materializes() {
    let request = "Build a FastAPI backend service with endpoints";
    let (mut agent, temp) = agent_with_request(request, WorkMode::Python, true);
    assert_eq!(
        classified_kind(&agent),
        TaskKind::Coding,
        "FastAPI build must classify as Coding so the gate admits it"
    );
    assert!(scaffold_allowed_for_active_task(&agent));

    let decision = CompletionDecision::Continue {
        missing: vec![ArtifactRole::Implementation],
    };
    let fired = maybe_materialize_task_contract_fallback(&mut agent, &decision, 0);
    assert!(
        fired,
        "coding FastAPI task-contract fallback must still materialize"
    );
    assert!(
        temp.path().join("pyproject.toml").is_file(),
        "FastAPI scaffold must write pyproject.toml"
    );
}

// ---------------------------------------------------------------------------
// Case 4b — a non-coding (Data) request does NOT materialize via the
// task-contract fallback even with `WorkMode::Python` + specialized fallback +
// `Continue`: the fn-entry gate fires before the python/fastapi materializer.
// ---------------------------------------------------------------------------

#[test]
fn non_coding_data_request_does_not_materialize_task_contract_fallback() {
    // A pure CSV/data request that classifies as Data; the gate must short-circuit
    // before `policy_allows_python_specialized_fallback` / `fastapi_scaffold_files`.
    let request = "Generate report output.csv from the input data";
    let (mut agent, temp) = agent_with_request(request, WorkMode::Python, true);
    assert_eq!(
        classified_kind(&agent),
        TaskKind::Data,
        "data CSV request must classify as Data"
    );
    assert!(!scaffold_allowed_for_active_task(&agent));

    let decision = CompletionDecision::Continue {
        missing: vec![ArtifactRole::DataOutput],
    };
    let fired = maybe_materialize_task_contract_fallback(&mut agent, &decision, 0);
    assert!(
        !fired,
        "non-coding Data request must not materialize a deterministic scaffold"
    );
    assert!(
        !temp.path().join("pyproject.toml").is_file(),
        "no scaffold files may be written for a non-coding kind"
    );
}

// ---------------------------------------------------------------------------
// Case 5 — manifest ARTIFACT path: a malformed package.json yields InvalidManifest
// for a coding task but is NOT diagnosed for a non-coding task (the `is_coding`
// gate on the artifact path). Driven through the production `Verifier::diagnostic`.
// ---------------------------------------------------------------------------

#[test]
fn manifest_artifact_gate_is_coding_only() {
    use super::verifier::{CodingVerifier, DataVerifier};

    let malformed = VerifierArtifact {
        path: Some("package.json"),
        excerpt: r#"{"scripts":"#,
        required_columns: &[],
        required_sections: &[],
    };

    // Coding: the manifest gate fires → InvalidManifest (unchanged).
    let coding = CodingVerifier
        .diagnostic(VerifierArtifact { ..malformed })
        .expect("coding malformed manifest must still diagnose");
    assert_eq!(coding.code, VerifierDiagnosticCode::InvalidManifest);
    assert_eq!(coding.role, ArtifactRole::Setup);

    // Non-coding (Data): the manifest gate is skipped. The non-empty excerpt then
    // flows to the Data arm, which can never emit InvalidManifest.
    let data = DataVerifier.diagnostic(VerifierArtifact { ..malformed });
    if let Some(diagnostic) = data {
        assert_ne!(
            diagnostic.code,
            VerifierDiagnosticCode::InvalidManifest,
            "non-coding artifact path must never produce InvalidManifest"
        );
    }
}

// ---------------------------------------------------------------------------
// Case 6 — manifest OBLIGATION path is UNCHANGED: a non-coding install task with
// an explicit Setup package.json obligation still gets InvalidManifest when the
// manifest is malformed (the obligation path is role==Setup scoped, not gated).
// ---------------------------------------------------------------------------

#[test]
fn manifest_obligation_path_still_fires_for_non_coding_setup() {
    use super::verifier::verifier_diagnostic_for_obligation;

    // An explicit Setup package.json obligation (format inferred = Json).
    let obligation = DeliverableObligation::file(ArtifactRole::Setup, "package.json");
    assert_eq!(obligation.role, ArtifactRole::Setup);

    // Drive the obligation path for a non-coding kind (Ops install task). The
    // malformed manifest must still be flagged InvalidManifest (S3-003).
    let diagnostic = verifier_diagnostic_for_obligation(
        TaskKind::Ops,
        &obligation,
        Some(r#"{"scripts":"#),
        true,
    )
    .expect("malformed Setup manifest obligation must still diagnose for non-coding");
    assert_eq!(
        diagnostic.code,
        VerifierDiagnosticCode::InvalidManifest,
        "the role==Setup obligation manifest gate must remain ungated"
    );
}

// ---------------------------------------------------------------------------
// Case 8 — format-error small-edit gate.
//   (a) non-coding → Ok(None) (the fn-entry gate fires before any write).
//   (b) coding + format-error + Rust arithmetic pattern → Ok(Some) and the file
//       is actually patched (exercises the write path that had no positive test).
// ---------------------------------------------------------------------------

/// Push an assistant message carrying a `Read` tool call for `path` so
/// `last_read_tool_path` resolves the small-edit target.
fn push_read_tool_call(agent: &mut super::Agent, path: &str) {
    agent.session.messages.push(ConversationMessage::assistant(
        String::new(),
        vec![ToolCall {
            id: "read-1".to_string(),
            name: "Read".to_string(),
            arguments: serde_json::json!({ "path": path }),
        }],
    ));
}

#[test]
fn format_error_small_edit_skips_non_coding() {
    // A Data request whose latest user message classifies non-coding.
    let request = "Generate output.csv from the input data";
    let (mut agent, temp) = agent_with_request(request, WorkMode::Python, true);
    assert_eq!(classified_kind(&agent), TaskKind::Data);

    // Stage a Rust file matching the arithmetic pattern + a Read tool call. The
    // gate must short-circuit BEFORE the write path, leaving the file untouched.
    let src = "pub fn multiply(left: i64, right: i64) -> i64 { left + right }\n";
    std::fs::write(temp.path().join("lib.rs"), src).unwrap();
    push_read_tool_call(&mut agent, "lib.rs");

    let err = "tool call parser failed: malformed payload";
    let result =
        maybe_apply_deterministic_edit_after_format_error(&mut agent, err).expect("must not error");
    assert!(
        result.is_none(),
        "non-coding format-error small edit must be skipped (Ok(None))"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("lib.rs")).unwrap(),
        src,
        "non-coding gate must not write the arithmetic patch"
    );
}

#[test]
fn format_error_small_edit_applies_for_coding() {
    // A coding request so the gate admits the write path.
    let request = "Implement a multiply function in Rust";
    let (mut agent, temp) = agent_with_request(request, WorkMode::GenericCode, true);
    assert_eq!(
        classified_kind(&agent),
        TaskKind::Coding,
        "coding request must classify as Coding for the positive write path"
    );
    assert!(scaffold_allowed_for_active_task(&agent));

    // The model must advertise the deterministic-edit-after-format-error
    // capability (qwen3.5 family does).
    agent.models.main = "qwen3.5:7b".to_string();

    let src = "pub fn multiply(left: i64, right: i64) -> i64 { left + right }\n";
    std::fs::write(temp.path().join("lib.rs"), src).unwrap();
    push_read_tool_call(&mut agent, "lib.rs");

    let err = "tool call parser failed: malformed payload";
    let result =
        maybe_apply_deterministic_edit_after_format_error(&mut agent, err).expect("must not error");
    assert!(
        result.is_some(),
        "coding format-error small edit must apply (Ok(Some))"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("lib.rs")).unwrap(),
        "pub fn multiply(left: i64, right: i64) -> i64 { left * right }\n",
        "coding write path must patch `left + right` -> `left * right`"
    );
}

// ---------------------------------------------------------------------------
// Case 8c — UI quality fallback (a live `&Agent` dispatch fn) is gated for
// non-coding and admitted for coding, exercising the `Ok(false)` not-applicable
// return type at the fn entry.
// ---------------------------------------------------------------------------

#[test]
fn quality_fallback_gate_respects_coding() {
    // Non-coding: the gate returns Ok(false) before touching the filesystem, so a
    // missing target does not surface a read error.
    let (agent, _temp) = agent_with_request(
        "Generate output.csv from the input data",
        WorkMode::Python,
        true,
    );
    assert_eq!(classified_kind(&agent), TaskKind::Data);
    let gated =
        maybe_apply_deterministic_quality_fallback(&agent, "improve the UI", "does-not-exist.html")
            .expect("non-coding gate must return Ok(false), not a read error");
    assert!(!gated, "non-coding quality fallback must be Ok(false)");

    // Coding: the gate admits the fn, so the missing target now surfaces the read
    // error (proving the gate did not short-circuit).
    let (agent, _temp) = agent_with_request(
        "Implement a multiply function in Rust",
        WorkMode::GenericCode,
        true,
    );
    assert_eq!(classified_kind(&agent), TaskKind::Coding);
    let result =
        maybe_apply_deterministic_quality_fallback(&agent, "improve the UI", "does-not-exist.html");
    assert!(
        result.is_err(),
        "coding path must proceed past the gate and hit the missing-target read error"
    );
}
