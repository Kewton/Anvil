//! Issue #1005: in-crate `#[cfg(test)] mod` E2E suite for the
//! `RepairOperatorRegistry` (CB-001 / DR3-001 pattern shared with
//! `safe_stop_e2e_tests` / `job_report_e2e_tests`). Production binary excludes
//! this module.
//!
//! These drive the **production projection** `FailureContext::from_repair_job`
//! against realistic `RepairJob` fixtures — the same path the verifier-repair
//! slot in `repair_job_dispatch.rs` uses — and assert the registry routes each
//! representative failure to the right operator candidate set (AC1 / AC2), that
//! docs/data extension operators are reachable as plain rows without a new
//! deterministic job (AC3), and that an unroutable failure is observed as
//! `operator_missing` (AC4).

use super::repair_job::RepairJob;
use super::repair_operator::{
    FailureClass, FailureContext, OperatorId, OperatorKind, candidates_for, classify, registry,
    rust_binding_operator_id, select,
};
use super::task_contract::{ArtifactRole, RecoveryTargetHint};

/// A `RepairJob` carrying a realistic verifier failure diagnostic, mirroring
/// what the deterministic repair slot sees (`error_kind` + `output_excerpt`).
fn job_with_failure(error_kind: &str, output_excerpt: &str) -> RepairJob {
    RepairJob {
        error_kind: Some(error_kind.to_string()),
        output_excerpt: output_excerpt.to_string(),
        ..RepairJob::new_for_test()
    }
}

#[test]
fn rust_binding_failure_routes_through_registry() {
    // AC2: Rust crate/bin/lib mismatch handled via the registry.
    let job = job_with_failure(
        "compile_error",
        "error: environment variable `CARGO_BIN_EXE_app` not defined at compile time",
    );
    let ctx = FailureContext::from_repair_job(Some(&job));
    assert_eq!(classify(&ctx), Some(FailureClass::RustCrateBindingMismatch));

    let selection = select(classify(&ctx), ctx.target_role);
    assert!(!selection.operator_missing());
    assert!(
        selection
            .candidates
            .contains(&OperatorId::RustCargoBinExeBinding)
    );
    assert!(
        selection
            .candidates
            .contains(&OperatorId::RustLibNameBinding)
    );

    // The live dispatch maps the rust operator's `Applied { operator }` label to
    // the registry id it records.
    assert_eq!(
        rust_binding_operator_id("rust_fix_lib_name_for_integration_test"),
        Some(OperatorId::RustLibNameBinding)
    );
}

#[test]
fn node_missing_test_script_routes_through_registry() {
    // AC2: Node missing package/test script handled via the registry.
    let job = job_with_failure(
        "evidence_missing",
        "npm error Missing script: test — check the package.json scripts field",
    );
    let ctx = FailureContext::from_repair_job(Some(&job));
    assert_eq!(classify(&ctx), Some(FailureClass::NodeTestRunnerUnbound));

    // The scaffold path records `select(NodeTestRunnerUnbound, Setup)`; it must
    // resolve to the node operator and not be operator_missing.
    let scaffold_selection = select(
        Some(FailureClass::NodeTestRunnerUnbound),
        Some(ArtifactRole::Setup),
    );
    assert!(!scaffold_selection.operator_missing());
    assert_eq!(
        scaffold_selection.candidates,
        vec![OperatorId::NodeTestRunnerManifest]
    );
}

#[test]
fn cargo_serde_dependency_failure_routes_through_registry() {
    // AC1: failure class → operator candidate from a realistic diagnostic.
    let job = job_with_failure(
        "compile_error",
        "error[E0432]: unresolved import `serde_json`\n use of undeclared crate or module",
    );
    let ctx = FailureContext::from_repair_job(Some(&job));
    assert_eq!(classify(&ctx), Some(FailureClass::CargoDependencyMissing));
    let selection = select(classify(&ctx), ctx.target_role);
    assert_eq!(selection.candidates, vec![OperatorId::CargoSerdeDependency]);
}

#[test]
fn target_hint_role_narrows_candidates() {
    // The dispatch projects `target_hint.role` into the FailureContext; a
    // Test-role hint narrows the Rust binding candidates to the bin operator.
    let job = RepairJob {
        error_kind: Some("compile_error".to_string()),
        output_excerpt: "error: `CARGO_BIN_EXE_app` not defined".to_string(),
        target_hint: Some(RecoveryTargetHint {
            role: ArtifactRole::Test,
            path: "tests/cli.rs".to_string(),
            reason: "bin exe binding".to_string(),
        }),
        ..RepairJob::new_for_test()
    };
    let ctx = FailureContext::from_repair_job(Some(&job));
    assert_eq!(ctx.target_role, Some(ArtifactRole::Test));
    let selection = select(classify(&ctx), ctx.target_role);
    assert_eq!(
        selection.candidates,
        vec![OperatorId::RustCargoBinExeBinding]
    );
}

#[test]
fn unroutable_failure_is_operator_missing() {
    // AC4: a failure with no registered operator is observed as
    // `operator_missing` (the recorded payload sets the flag).
    let job = job_with_failure("runtime_error", "panicked at 'assertion failed: x == y'");
    let ctx = FailureContext::from_repair_job(Some(&job));
    assert_eq!(classify(&ctx), None);
    let selection = select(classify(&ctx), ctx.target_role);
    assert!(selection.operator_missing());
    assert!(selection.candidates.is_empty());
}

#[test]
fn docs_and_data_operators_add_no_deterministic_job() {
    // AC3: docs/data/research operators are plain registry rows reachable via
    // the same select() path, and they do NOT introduce a new deterministic
    // repair job — the deterministic operator set stays the existing ones
    // (cargo / rust lib / rust bin / node manifest). New rows are `LlmSingleShot`.
    let deterministic: Vec<OperatorId> = registry()
        .iter()
        .filter(|d| d.kind == OperatorKind::Deterministic)
        .map(|d| d.id)
        .collect();
    assert_eq!(
        deterministic,
        vec![
            OperatorId::CargoSerdeDependency,
            OperatorId::RustLibNameBinding,
            OperatorId::RustCargoBinExeBinding,
            OperatorId::NodeTestRunnerManifest,
        ]
    );

    for (class, id) in [
        (
            FailureClass::DocsSectionMissing,
            OperatorId::DocsRequiredSection,
        ),
        (
            FailureClass::DataSchemaMismatch,
            OperatorId::CsvSchemaColumn,
        ),
    ] {
        let selection = select(Some(class), None);
        assert!(!selection.operator_missing());
        assert!(selection.candidates.contains(&id));
        // Reachable as a row, but single-shot LLM — no deterministic job.
        let descriptor = candidates_for(class, None)
            .into_iter()
            .find(|d| d.id == id)
            .expect("operator registered");
        assert_eq!(descriptor.kind, OperatorKind::LlmSingleShot);
    }
}

#[test]
fn missing_repair_job_yields_no_operator() {
    // Defensive: a turn with no active RepairJob projects to an empty context
    // and classifies to no operator (the dispatch only observes inside the
    // repair slot, but the projection must be total).
    let ctx = FailureContext::from_repair_job(None);
    assert!(classify(&ctx).is_none());
    let selection = select(classify(&ctx), ctx.target_role);
    assert!(selection.operator_missing());
}
