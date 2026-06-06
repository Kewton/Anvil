//! Issue #1005: `RepairOperatorRegistry` — organize the deterministic repair
//! operators by **failure class / binding check / target role**.
//!
//! v0.6.5 grew repair-loop reach without converting failures into passes. The
//! deterministic operators that *do* convert a failure already exist
//! (`cargo_dependency_repair`, `rust_binding_repair`, `node_runner_manifest`),
//! but they are dispatched from hard-coded `if let … Applied` chains with no
//! catalogue. Adding a docs / data / research operator under that shape means a
//! new task-kind-specific repair job.
//!
//! This module is the table-driven registry SSOT that sits **over** the
//! existing operators (it does not re-implement them). It answers:
//! - "for this failure class, which operators are candidates?" (AC1)
//! - "which registered operator handled this failure — or is one missing?"
//!   (AC2 / AC4)
//!
//! A new operator (docs section, CSV column, …) is a new row in [`REGISTRY`];
//! it never requires a new job (AC3). When the registry routes a failure to an
//! `LlmSingleShot` operator, the existing bounded repair pass owns the patch —
//! still 1 failure / 1 target / 1 patch.
//!
//! `pub(super)` limited / no facade re-export (DR3-001). The failure-class
//! classifier reads `super::VerifierDiagnosticFailureKind` (loop_run-private,
//! visible to this child module) and reuses `super::task_contract::ArtifactRole`
//! as the target-role axis rather than minting a parallel enum.

use super::VerifierDiagnosticFailureKind;
use super::task_contract::ArtifactRole;
use crate::logging::log_llm_event;

/// Session-observation event recording the operator selected (or missing) for
/// an observed repair failure. Goes through `log_llm_event` so
/// `mask_payload_inplace` is the final defence line; the raw diagnostic text is
/// never placed on the payload.
pub(super) const EVENT_REPAIR_OPERATOR_SELECTED: &str = "agent.repair_operator.selected";

/// Target role an operator edits. Re-uses the artifact-ledger / repair-target
/// role SSOT instead of a parallel enum.
pub(super) type TargetRole = ArtifactRole;

/// Stable identifier for a registered repair operator. The string form is the
/// observation SSOT (`as_str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum OperatorId {
    /// Add a missing serde-family dependency to `Cargo.toml`
    /// ([`super::cargo_dependency_repair`]).
    CargoSerdeDependency,
    /// Rename `[lib] name` to match an integration-test import
    /// ([`super::rust_binding_repair`], `rust_fix_lib_name_for_integration_test`).
    RustLibNameBinding,
    /// Rebind a `CARGO_BIN_EXE_<name>` reference to the real binary
    /// ([`super::rust_binding_repair`], `rust_fix_cargo_bin_exe_test_env`).
    RustCargoBinExeBinding,
    /// Create / complete `package.json` `scripts.test`
    /// ([`super::node_runner_manifest`]).
    NodeTestRunnerManifest,
    /// Restore a Python CLI entrypoint (single-shot LLM repair).
    PythonCliEntrypoint,
    /// Restore the importable FastAPI `app` (single-shot LLM repair).
    FastApiAppImport,
    /// Add a missing required documentation section (single-shot LLM repair).
    DocsRequiredSection,
    /// Add a missing CSV schema column (single-shot LLM repair).
    CsvSchemaColumn,
}

impl OperatorId {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            OperatorId::CargoSerdeDependency => "cargo_serde_dependency",
            OperatorId::RustLibNameBinding => "rust_lib_name_binding",
            OperatorId::RustCargoBinExeBinding => "rust_cargo_bin_exe_binding",
            OperatorId::NodeTestRunnerManifest => "node_test_runner_manifest",
            OperatorId::PythonCliEntrypoint => "python_cli_entrypoint",
            OperatorId::FastApiAppImport => "fastapi_app_import",
            OperatorId::DocsRequiredSection => "docs_required_section",
            OperatorId::CsvSchemaColumn => "csv_schema_column",
        }
    }
}

/// Failure class — the binding-check axis the registry is organized by. Coarse
/// by design: the precise applicability check lives inside each operator; this
/// only routes the failure to the candidate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum FailureClass {
    /// An undeclared serde-family crate (`E0432` / `E0433`).
    CargoDependencyMissing,
    /// A Rust crate / bin / lib binding mismatch (lib name, `CARGO_BIN_EXE`).
    RustCrateBindingMismatch,
    /// A Node workspace with test artifacts but no bindable test runner.
    NodeTestRunnerUnbound,
    /// A missing Python CLI / `__main__` entrypoint.
    PythonEntrypointMissing,
    /// A FastAPI `app` that cannot be imported.
    FastApiImportMissing,
    /// A required documentation section is absent.
    DocsSectionMissing,
    /// A CSV / structured-data schema or column mismatch.
    DataSchemaMismatch,
}

impl FailureClass {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            FailureClass::CargoDependencyMissing => "cargo_dependency_missing",
            FailureClass::RustCrateBindingMismatch => "rust_crate_binding_mismatch",
            FailureClass::NodeTestRunnerUnbound => "node_test_runner_unbound",
            FailureClass::PythonEntrypointMissing => "python_entrypoint_missing",
            FailureClass::FastApiImportMissing => "fastapi_import_missing",
            FailureClass::DocsSectionMissing => "docs_section_missing",
            FailureClass::DataSchemaMismatch => "data_schema_mismatch",
        }
    }
}

/// Whether an operator deterministically edits / materializes the fix, or
/// defers to a single-shot LLM repair bounded to 1 failure / 1 target / 1
/// patch. The bound is declared at the type level so a new `LlmSingleShot` row
/// can never grow into a multi-patch job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperatorKind {
    Deterministic,
    LlmSingleShot,
}

impl OperatorKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            OperatorKind::Deterministic => "deterministic",
            OperatorKind::LlmSingleShot => "llm_single_shot",
        }
    }
}

/// A registered operator. Pure metadata — the edit itself lives in the
/// operator's own module so this registry never duplicates a repair.
#[derive(Debug, Clone, Copy)]
pub(super) struct RepairOperatorDescriptor {
    pub(super) id: OperatorId,
    pub(super) failure_class: FailureClass,
    pub(super) target_role: TargetRole,
    pub(super) kind: OperatorKind,
    pub(super) summary: &'static str,
}

/// The operator registry. **Table-driven**: a new operator (docs / data /
/// research, …) is a new row here and nothing else — it never requires a new
/// task-kind-specific repair job (AC3). Deterministic rows map to existing
/// operators; `LlmSingleShot` rows reserve the failure class for the bounded
/// repair pass (1 failure / 1 target / 1 patch).
const REGISTRY: &[RepairOperatorDescriptor] = &[
    RepairOperatorDescriptor {
        id: OperatorId::CargoSerdeDependency,
        failure_class: FailureClass::CargoDependencyMissing,
        target_role: ArtifactRole::Setup,
        kind: OperatorKind::Deterministic,
        summary: "Add a missing serde-family dependency to Cargo.toml.",
    },
    RepairOperatorDescriptor {
        id: OperatorId::RustLibNameBinding,
        failure_class: FailureClass::RustCrateBindingMismatch,
        target_role: ArtifactRole::Setup,
        kind: OperatorKind::Deterministic,
        summary: "Rename [lib] name to match the integration-test import.",
    },
    RepairOperatorDescriptor {
        id: OperatorId::RustCargoBinExeBinding,
        failure_class: FailureClass::RustCrateBindingMismatch,
        target_role: ArtifactRole::Test,
        kind: OperatorKind::Deterministic,
        summary: "Rebind a CARGO_BIN_EXE_<name> reference to the real binary.",
    },
    RepairOperatorDescriptor {
        id: OperatorId::NodeTestRunnerManifest,
        failure_class: FailureClass::NodeTestRunnerUnbound,
        target_role: ArtifactRole::Setup,
        kind: OperatorKind::Deterministic,
        summary: "Create/complete package.json scripts.test for the Node runner.",
    },
    RepairOperatorDescriptor {
        id: OperatorId::PythonCliEntrypoint,
        failure_class: FailureClass::PythonEntrypointMissing,
        target_role: ArtifactRole::Implementation,
        kind: OperatorKind::LlmSingleShot,
        summary: "Restore a Python CLI entrypoint (1 failure / 1 target / 1 patch).",
    },
    RepairOperatorDescriptor {
        id: OperatorId::FastApiAppImport,
        failure_class: FailureClass::FastApiImportMissing,
        target_role: ArtifactRole::Implementation,
        kind: OperatorKind::LlmSingleShot,
        summary: "Restore the importable FastAPI app (1 failure / 1 target / 1 patch).",
    },
    RepairOperatorDescriptor {
        id: OperatorId::DocsRequiredSection,
        failure_class: FailureClass::DocsSectionMissing,
        target_role: ArtifactRole::UsageDocs,
        kind: OperatorKind::LlmSingleShot,
        summary: "Add a missing required documentation section (1 failure / 1 target / 1 patch).",
    },
    RepairOperatorDescriptor {
        id: OperatorId::CsvSchemaColumn,
        failure_class: FailureClass::DataSchemaMismatch,
        target_role: ArtifactRole::DataOutput,
        kind: OperatorKind::LlmSingleShot,
        summary: "Add a missing CSV schema column (1 failure / 1 target / 1 patch).",
    },
];

/// The full registry table (for callers that enumerate operators). Reserved
/// registry API — exercised by the totality tests and available to future
/// enumeration callers; the live dispatch routes through [`select`].
#[allow(dead_code)]
pub(super) fn registry() -> &'static [RepairOperatorDescriptor] {
    REGISTRY
}

/// The descriptor for an operator id, if registered.
pub(super) fn descriptor_for(id: OperatorId) -> Option<&'static RepairOperatorDescriptor> {
    REGISTRY.iter().find(|descriptor| descriptor.id == id)
}

/// AC1: candidate operators for a failure class, optionally narrowed by target
/// role. Registry order is preserved (deterministic rows precede the
/// single-shot LLM fallbacks within a class).
pub(super) fn candidates_for(
    failure_class: FailureClass,
    target_role: Option<TargetRole>,
) -> Vec<&'static RepairOperatorDescriptor> {
    REGISTRY
        .iter()
        .filter(|descriptor| descriptor.failure_class == failure_class)
        .filter(|descriptor| target_role.is_none_or(|role| descriptor.target_role == role))
        .collect()
}

/// The outcome of consulting the registry for an observed failure. A `None`
/// `failure_class` (or an empty candidate set) means the failure could not be
/// routed to any registered operator — `operator_missing` (AC4).
#[derive(Debug, Clone)]
pub(super) struct OperatorSelection {
    pub(super) failure_class: Option<FailureClass>,
    pub(super) target_role: Option<TargetRole>,
    pub(super) candidates: Vec<OperatorId>,
}

impl OperatorSelection {
    /// AC4: no registered operator candidate for the observed failure.
    pub(super) fn operator_missing(&self) -> bool {
        self.failure_class.is_none() || self.candidates.is_empty()
    }

    /// The first deterministic candidate, if any — the operator the
    /// deterministic repair slot would attempt for this class. Reserved
    /// registry API: the live slot runs the existing operators unconditionally
    /// (they self-gate); this accessor lets a future caller / the tests inspect
    /// the deterministic-first ordering.
    #[allow(dead_code)]
    pub(super) fn first_deterministic(&self) -> Option<OperatorId> {
        let class = self.failure_class?;
        candidates_for(class, self.target_role)
            .into_iter()
            .find(|descriptor| descriptor.kind == OperatorKind::Deterministic)
            .map(|descriptor| descriptor.id)
    }
}

/// Resolve a failure class (and optional target role) to its registered
/// operator candidates.
pub(super) fn select(
    failure_class: Option<FailureClass>,
    target_role: Option<TargetRole>,
) -> OperatorSelection {
    let candidates = failure_class
        .map(|class| {
            candidates_for(class, target_role)
                .into_iter()
                .map(|descriptor| descriptor.id)
                .collect()
        })
        .unwrap_or_default();
    OperatorSelection {
        failure_class,
        target_role,
        candidates,
    }
}

/// A cheap projection of a verifier failure used to route it to an operator.
/// Mirrors `cargo_dependency_repair::cargo_dependency_diagnostic_for_job` so
/// the registry sees the same diagnostic the operators self-gate on.
#[derive(Debug, Clone, Default)]
pub(super) struct FailureContext {
    pub(super) failure_kind: Option<VerifierDiagnosticFailureKind>,
    pub(super) diagnostic: String,
    pub(super) target_role: Option<TargetRole>,
}

impl FailureContext {
    pub(super) fn from_repair_job(job: Option<&super::repair_job::RepairJob>) -> Self {
        let Some(job) = job else {
            return Self::default();
        };
        let error_kind = job
            .error_kind
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(job.failure_signature.as_str());
        let diagnostic = if job.output_excerpt.trim().is_empty() {
            error_kind.to_string()
        } else {
            format!("{error_kind}\n{}", job.output_excerpt)
        };
        let failure_kind = job.semantic_plan.as_ref().map(|plan| plan.semantic_cause);
        let target_role = job.target_hint.as_ref().map(|hint| hint.role).or_else(|| {
            job.semantic_plan
                .as_ref()
                .map(|plan| plan.preferred_repair_role)
        });
        Self {
            failure_kind,
            diagnostic,
            target_role,
        }
    }
}

/// Route a failure to its [`FailureClass`]. Coarse keyword + semantic-kind
/// matching — good enough to pick the candidate *set*; each operator confirms
/// precise applicability before editing.
pub(super) fn classify(ctx: &FailureContext) -> Option<FailureClass> {
    classify_from_diagnostic(&ctx.diagnostic, ctx.failure_kind)
}

pub(super) fn classify_from_diagnostic(
    diagnostic: &str,
    failure_kind: Option<VerifierDiagnosticFailureKind>,
) -> Option<FailureClass> {
    let lower = diagnostic.to_ascii_lowercase();

    // 1. Cargo serde-family missing dependency (owned by cargo_dependency_repair).
    let names_serde = lower.contains("serde_json") || lower.contains("serde");
    let undeclared_crate =
        lower.contains("e0432") || lower.contains("e0433") || lower.contains("undeclared crate");
    if names_serde && undeclared_crate {
        return Some(FailureClass::CargoDependencyMissing);
    }

    // 2. Rust crate / bin / lib binding mismatch (owned by rust_binding_repair).
    if lower.contains("cargo_bin_exe")
        || lower.contains("[lib]")
        || lower.contains("can't find crate")
        || (lower.contains("unresolved import") && lower.contains("cargo"))
    {
        return Some(FailureClass::RustCrateBindingMismatch);
    }

    // 3. Node test-runner unbound (owned by node_runner_manifest).
    if lower.contains("package.json")
        || lower.contains("missing script: test")
        || lower.contains("npm test")
        || lower.contains("node --test")
    {
        return Some(FailureClass::NodeTestRunnerUnbound);
    }

    // 4. FastAPI app import — checked before the generic Python entrypoint so a
    //    FastAPI import failure is not swallowed by `no module named`.
    if lower.contains("fastapi") || lower.contains("uvicorn") {
        return Some(FailureClass::FastApiImportMissing);
    }

    // 5. Python CLI / __main__ entrypoint.
    if lower.contains("no module named")
        || lower.contains("modulenotfounderror")
        || lower.contains("__main__")
    {
        return Some(FailureClass::PythonEntrypointMissing);
    }

    // 6. CSV / structured-data schema or column mismatch.
    let column_problem =
        lower.contains("column") && (lower.contains("missing") || lower.contains("schema"));
    if lower.contains("csv") || column_problem {
        return Some(FailureClass::DataSchemaMismatch);
    }

    // 7. Docs required section.
    if lower.contains("required section") || lower.contains("missing section") {
        return Some(FailureClass::DocsSectionMissing);
    }

    // Coarse fallback on the semantic failure kind for routing a schema-shaped
    // failure that didn't carry an explicit keyword.
    match failure_kind {
        Some(VerifierDiagnosticFailureKind::SchemaMismatch) => {
            Some(FailureClass::DataSchemaMismatch)
        }
        _ => None,
    }
}

/// Map a `rust_binding_repair` operator label to its registry id. The label
/// strings mirror `rust_binding_repair::OPERATOR_LIB_NAME` /
/// `OPERATOR_CARGO_BIN_EXE_ENV` (log SSOT); a drift here is caught by
/// `rust_binding_operator_id_matches_labels` in tests.
pub(super) fn rust_binding_operator_id(operator_label: &str) -> Option<OperatorId> {
    match operator_label {
        "rust_fix_lib_name_for_integration_test" => Some(OperatorId::RustLibNameBinding),
        "rust_fix_cargo_bin_exe_test_env" => Some(OperatorId::RustCargoBinExeBinding),
        _ => None,
    }
}

/// AC4: emit the operator-selection observation. `applied = Some(id)` when a
/// deterministic operator from the registry just produced an edit; `None` when
/// the failure fell through the deterministic slot (a routed class is handled
/// by the bounded LLM repair pass, or the failure was unroutable →
/// `operator_missing`). `log_llm_event` masks the payload; the diagnostic text
/// is never placed on it.
pub(super) fn record_operator_selection(
    session_id: &str,
    selection: &OperatorSelection,
    applied: Option<OperatorId>,
) {
    let candidates: Vec<&'static str> = selection.candidates.iter().map(|id| id.as_str()).collect();
    let applied_descriptor = applied.and_then(descriptor_for);
    log_llm_event(
        EVENT_REPAIR_OPERATOR_SELECTED,
        serde_json::json!({
            "session_id": session_id,
            "failure_class": selection.failure_class.map(FailureClass::as_str),
            "target_role": selection.target_role.map(ArtifactRole::label),
            "candidates": candidates,
            "applied": applied.map(OperatorId::as_str),
            "applied_kind": applied_descriptor.map(|descriptor| descriptor.kind.as_str()),
            "applied_summary": applied_descriptor.map(|descriptor| descriptor.summary),
            "operator_missing": applied.is_none() && selection.operator_missing(),
        }),
    );
}

/// Classify the failure, select operators, and emit the observation in one
/// call. Returns the selection so the deterministic repair slot can branch on
/// it if needed.
pub(super) fn observe_operator_selection(
    session_id: &str,
    ctx: &FailureContext,
    applied: Option<OperatorId>,
) -> OperatorSelection {
    let selection = select(classify(ctx), ctx.target_role);
    record_operator_selection(session_id, &selection, applied);
    selection
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registry row's `failure_class` resolves the row back via
    /// `candidates_for`, and every `OperatorId` appears exactly once.
    #[test]
    fn registry_is_self_consistent() {
        let mut seen = std::collections::HashSet::new();
        for descriptor in registry() {
            assert!(
                seen.insert(descriptor.id),
                "duplicate operator id {:?}",
                descriptor.id
            );
            let candidates = candidates_for(descriptor.failure_class, None);
            assert!(
                candidates.iter().any(|c| c.id == descriptor.id),
                "{:?} not selectable by its own failure class",
                descriptor.id
            );
        }
        // The initial operator set named in the issue is all present.
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn candidates_for_returns_operators_of_a_failure_class() {
        // AC1: failure class → operator candidates.
        let rust = candidates_for(FailureClass::RustCrateBindingMismatch, None);
        let ids: Vec<OperatorId> = rust.iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            vec![
                OperatorId::RustLibNameBinding,
                OperatorId::RustCargoBinExeBinding
            ]
        );
    }

    #[test]
    fn candidates_for_narrows_by_target_role() {
        let test_role = candidates_for(
            FailureClass::RustCrateBindingMismatch,
            Some(ArtifactRole::Test),
        );
        assert_eq!(test_role.len(), 1);
        assert_eq!(test_role[0].id, OperatorId::RustCargoBinExeBinding);

        let setup_role = candidates_for(
            FailureClass::RustCrateBindingMismatch,
            Some(ArtifactRole::Setup),
        );
        assert_eq!(setup_role.len(), 1);
        assert_eq!(setup_role[0].id, OperatorId::RustLibNameBinding);
    }

    #[test]
    fn select_marks_unroutable_failure_as_operator_missing() {
        // AC4: unclassified failure → operator_missing.
        let selection = select(None, None);
        assert!(selection.operator_missing());
        assert!(selection.candidates.is_empty());

        // A routed class is not "missing".
        let routed = select(Some(FailureClass::CargoDependencyMissing), None);
        assert!(!routed.operator_missing());
        assert_eq!(routed.candidates, vec![OperatorId::CargoSerdeDependency]);
    }

    #[test]
    fn first_deterministic_prefers_deterministic_rows() {
        let cargo = select(Some(FailureClass::CargoDependencyMissing), None);
        assert_eq!(
            cargo.first_deterministic(),
            Some(OperatorId::CargoSerdeDependency)
        );
        // Docs is single-shot LLM only — no deterministic operator.
        let docs = select(Some(FailureClass::DocsSectionMissing), None);
        assert_eq!(docs.first_deterministic(), None);
        assert!(!docs.operator_missing());
    }

    #[test]
    fn classify_routes_each_failure_class() {
        let cases = [
            (
                "error[E0432]: unresolved import `serde_json`",
                FailureClass::CargoDependencyMissing,
            ),
            (
                "error: environment variable `CARGO_BIN_EXE_app` not defined",
                FailureClass::RustCrateBindingMismatch,
            ),
            (
                "npm error Missing script: test\ncheck package.json",
                FailureClass::NodeTestRunnerUnbound,
            ),
            (
                "ImportError: cannot import name app from fastapi project",
                FailureClass::FastApiImportMissing,
            ),
            (
                "ModuleNotFoundError: No module named 'mypkg.__main__'",
                FailureClass::PythonEntrypointMissing,
            ),
            (
                "AssertionError: output.csv missing column `total`",
                FailureClass::DataSchemaMismatch,
            ),
            (
                "verifier: required section `Usage` missing from README.md",
                FailureClass::DocsSectionMissing,
            ),
        ];
        for (diagnostic, expected) in cases {
            assert_eq!(
                classify_from_diagnostic(diagnostic, None),
                Some(expected),
                "diagnostic did not route: {diagnostic}"
            );
        }
    }

    #[test]
    fn classify_falls_back_to_schema_mismatch_kind() {
        assert_eq!(
            classify_from_diagnostic(
                "values differ from the declared shape",
                Some(VerifierDiagnosticFailureKind::SchemaMismatch),
            ),
            Some(FailureClass::DataSchemaMismatch)
        );
        assert_eq!(
            classify_from_diagnostic("totally opaque failure", None),
            None
        );
    }

    #[test]
    fn fastapi_is_checked_before_generic_python() {
        // A FastAPI import failure that also mentions a module must route to
        // FastApiImportMissing, not PythonEntrypointMissing.
        assert_eq!(
            classify_from_diagnostic(
                "No module named 'app'; uvicorn could not import the fastapi app",
                None,
            ),
            Some(FailureClass::FastApiImportMissing)
        );
    }

    #[test]
    fn rust_binding_operator_id_matches_labels() {
        assert_eq!(
            rust_binding_operator_id("rust_fix_lib_name_for_integration_test"),
            Some(OperatorId::RustLibNameBinding)
        );
        assert_eq!(
            rust_binding_operator_id("rust_fix_cargo_bin_exe_test_env"),
            Some(OperatorId::RustCargoBinExeBinding)
        );
        assert_eq!(rust_binding_operator_id("unknown"), None);
    }

    #[test]
    fn failure_context_from_missing_job_is_empty() {
        let ctx = FailureContext::from_repair_job(None);
        assert!(ctx.diagnostic.is_empty());
        assert!(ctx.failure_kind.is_none());
        assert!(ctx.target_role.is_none());
        assert!(classify(&ctx).is_none());
    }

    #[test]
    fn docs_data_research_rows_exist_without_new_jobs() {
        // AC3: the docs/data extension operators are plain table rows reachable
        // through the same select() path — no task-kind-specific job.
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
            assert!(selection.candidates.contains(&id));
            assert_eq!(
                candidates_for(class, None)
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.kind),
                Some(OperatorKind::LlmSingleShot)
            );
        }
    }
}
