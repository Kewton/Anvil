//! Issue #988 (parent #974): EvidenceBinding / NoProgressRecovery lifecycle
//! boundary.
//!
//! v0.6.4 left `repair_exhausted` as the single most common terminal state
//! (7/20) and all three Rust cases failed, even though the iteration budget was
//! never exhausted. The bottleneck is not the model: it is that the controller
//! rounds distinct failure shapes into the same coarse terminals
//! (`safe_stop_verifier_missing` / `missing_repo_edits` / generic
//! `repair_exhausted`) and re-attacks the same target with the same diagnostic.
//!
//! This module owns the **Binding** and **Recovery-classification** stages of
//! the generic `Objective -> Deliverable -> Evidence -> Binding -> Observation
//! -> Recovery -> TerminalProjection` lifecycle (#974). It is the structural
//! boundary the child issues (#989-#994) build on:
//!
//! - [`EvidenceBindingPlan`] structures the deliverable<->evidence-runner
//!   binding for a task kind and classifies mismatches through one path for
//!   Rust *and* Node (and an extension axis for docs/data/research). It calls
//!   the existing deterministic operators' pure cores
//!   ([`super::cargo_dependency_repair`], [`super::node_runner_manifest`]) so
//!   the binding classification is genuinely unified, not a parallel copy.
//! - [`RepairOperatorId`] is the small operator-registry boundary that names
//!   the deterministic operators behind one enum (the implicit dispatch order
//!   in `repair_job_dispatch::handle_repair_job_patch_provider_step`).
//! - [`RepairExhaustionClass`] decomposes `repair_exhausted` into
//!   `same_target_exhausted` / `operator_missing` / `binding_failed_after_repair`
//!   / `contract_conflict`, each projecting back to the legacy label and the
//!   generic terminal vocabulary ([`GenericTerminalState`]) for eval
//!   compatibility.
//! - [`NoProgressRecoveryPolicy`] turns a same-target/same-role no-progress
//!   signal (the #987 `TargetReassessmentRequired` shape) into a structured ban
//!   payload plus a forced role switch, instead of a blind replan.
//!
//! Scope: this is the extension-seam foundation (mirrors `evidence_runner.rs`
//! #949 and `summary::GenericTerminalState` #947). It is pure and fully
//! unit-tested; the broad caller wiring into the live repair loop is owned by
//! the child issues. `pub(super)` limited / no facade re-export (DR3-001).

#![allow(dead_code)] // Extension seam; focused tests pin the shape before wiring broad callers.

use super::cargo_dependency_repair::{complete_cargo_dependencies, undeclared_known_crates};
use super::evidence_runner::EvidenceRunnerKind;
use super::node_runner_manifest::{NodeManifestAction, complete_node_test_runner_manifest};
use super::summary::GenericTerminalState;
use super::task_contract::{ArtifactRole, TaskKind};
use crate::session::feedback::mask_secrets;

/// Repeated no-progress occurrences before [`NoProgressRecoveryPolicy`] bans the
/// target/role and forces a role switch. Two consecutive same-target failures
/// (post-edit, same diagnostic) is the same threshold the #987 target
/// reassessment uses (`repair_attempt >= 2`).
const NO_PROGRESS_THRESHOLD: u32 = 2;

/// A single binding-check identity in the generic lifecycle. Extensible across
/// runtimes (Rust / Node) and task kinds (docs / data / research) so a new
/// runtime adds variants here rather than a bespoke lifecycle (#988 non-goal:
/// no Rust-only / Node-only lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingCheckKind {
    /// `Cargo.toml` exists for a Rust verifier run.
    CargoManifestPresent,
    /// Declared crates (serde-family allowlist) resolve in `[dependencies]`.
    CargoDependencyDeclared,
    /// `[lib] name` binds the expected crate name.
    CargoLibTargetName,
    /// `[[bin]] name` binds every `CARGO_BIN_EXE_*` an integration test uses.
    CargoBinTargetName,
    /// An expected `tests/*.rs` integration target is present.
    CargoIntegrationTestPresent,
    /// `package.json` exists for a Node verifier run.
    NodeManifestPresent,
    /// `package.json` declares a usable `scripts.test`.
    NodeTestScriptDeclared,
    /// Docs deliverable content is bound to its content check (#993).
    DocsContentBound,
    /// Data deliverable is bound to its schema check (#993).
    DataSchemaBound,
    /// Research deliverable is bound to its citation check (#993).
    ResearchCitationBound,
}

impl BindingCheckKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingCheckKind::CargoManifestPresent => "cargo_manifest_present",
            BindingCheckKind::CargoDependencyDeclared => "cargo_dependency_declared",
            BindingCheckKind::CargoLibTargetName => "cargo_lib_target_name",
            BindingCheckKind::CargoBinTargetName => "cargo_bin_target_name",
            BindingCheckKind::CargoIntegrationTestPresent => "cargo_integration_test_present",
            BindingCheckKind::NodeManifestPresent => "node_manifest_present",
            BindingCheckKind::NodeTestScriptDeclared => "node_test_script_declared",
            BindingCheckKind::DocsContentBound => "docs_content_bound",
            BindingCheckKind::DataSchemaBound => "data_schema_bound",
            BindingCheckKind::ResearchCitationBound => "research_citation_bound",
        }
    }

    /// The evidence runner this check gates. Projects onto the existing runner
    /// vocabulary (#949) so the binding stage and the runner stage share one
    /// language rather than drifting.
    pub(super) fn evidence_runner_kind(self) -> EvidenceRunnerKind {
        match self {
            BindingCheckKind::CargoManifestPresent
            | BindingCheckKind::CargoDependencyDeclared
            | BindingCheckKind::CargoLibTargetName
            | BindingCheckKind::CargoBinTargetName
            | BindingCheckKind::CargoIntegrationTestPresent
            | BindingCheckKind::NodeManifestPresent
            | BindingCheckKind::NodeTestScriptDeclared => EvidenceRunnerKind::CodingBuildTest,
            BindingCheckKind::DocsContentBound => EvidenceRunnerKind::DocsContentCheck,
            BindingCheckKind::DataSchemaBound => EvidenceRunnerKind::DataSchemaCheck,
            BindingCheckKind::ResearchCitationBound => EvidenceRunnerKind::ResearchSourceFetch,
        }
    }

    /// The deterministic operator that can repair this binding mismatch, if any.
    /// SSOT for the check->operator mapping used by [`EvidenceBindingPlan::bind`];
    /// checks without a deterministic operator defer to the LLM repair pass but
    /// are still classified as binding mismatches (not generic
    /// `repair_exhausted`).
    pub(super) fn recommended_operator(self) -> Option<RepairOperatorId> {
        match self {
            BindingCheckKind::CargoDependencyDeclared => Some(RepairOperatorId::CargoDependency),
            BindingCheckKind::NodeManifestPresent | BindingCheckKind::NodeTestScriptDeclared => {
                Some(RepairOperatorId::NodeManifest)
            }
            BindingCheckKind::CargoManifestPresent
            | BindingCheckKind::CargoLibTargetName
            | BindingCheckKind::CargoBinTargetName
            | BindingCheckKind::CargoIntegrationTestPresent
            | BindingCheckKind::DocsContentBound
            | BindingCheckKind::DataSchemaBound
            | BindingCheckKind::ResearchCitationBound => None,
        }
    }
}

/// Registry of deterministic repair operators. Names the operators behind one
/// enum (vs. one giant runtime job), mirroring the dispatch order in
/// `repair_job_dispatch::handle_repair_job_patch_provider_step`
/// (cargo dependency -> mechanical compile -> LLM). docs/data/research
/// operators are future registry entries (#993), not new lifecycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairOperatorId {
    /// [`super::cargo_dependency_repair`] (#986): completes missing
    /// serde-family `[dependencies]`.
    CargoDependency,
    /// [`super::mechanical_compile_repair`]: deterministic compile-error edit.
    MechanicalCompile,
    /// [`super::node_runner_manifest`] (#984): completes the Node test-runner
    /// manifest.
    NodeManifest,
}

impl RepairOperatorId {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            RepairOperatorId::CargoDependency => "cargo_dependency",
            RepairOperatorId::MechanicalCompile => "mechanical_compile",
            RepairOperatorId::NodeManifest => "node_manifest",
        }
    }

    /// Deterministic dispatch precedence. The cargo dependency operator runs
    /// before the mechanical compile operator (a missing dep masquerades as a
    /// compile error), which runs before any LLM pass; the Node manifest
    /// operator is the MissingEvidence counterpart on the Node path.
    pub(super) fn registry_order() -> [RepairOperatorId; 3] {
        [
            RepairOperatorId::CargoDependency,
            RepairOperatorId::MechanicalCompile,
            RepairOperatorId::NodeManifest,
        ]
    }

    /// The binding check this operator satisfies, if it is binding-shaped. The
    /// mechanical compile operator repairs a compile error rather than a
    /// binding, so it has no representative check.
    pub(super) fn binding_check(self) -> Option<BindingCheckKind> {
        match self {
            RepairOperatorId::CargoDependency => Some(BindingCheckKind::CargoDependencyDeclared),
            RepairOperatorId::NodeManifest => Some(BindingCheckKind::NodeManifestPresent),
            RepairOperatorId::MechanicalCompile => None,
        }
    }
}

/// Pure, runtime-agnostic facts about the current workspace/diagnostic, fed to
/// [`EvidenceBindingPlan::bind`]. No `Agent` / filesystem access; the live
/// dispatch caller fills these from disk (child issue wiring).
#[derive(Debug, Clone, Copy)]
pub(super) struct BindingProbe<'a> {
    /// The latest verifier diagnostic text.
    pub(super) verifier_diagnostic: &'a str,
    /// `Cargo.toml` contents, if present.
    pub(super) cargo_manifest: Option<&'a str>,
    /// `package.json` contents, if present.
    pub(super) node_manifest: Option<&'a str>,
    /// Workspace-relative test file paths observed this run.
    pub(super) present_test_files: &'a [&'a str],
    /// Bin names referenced via `env!("CARGO_BIN_EXE_<name>")` in tests.
    pub(super) referenced_bin_exe_names: &'a [&'a str],
    /// The crate name the task expects `[lib] name` to bind, if known.
    pub(super) expected_crate_name: Option<&'a str>,
    /// Non-coding artifact excerpt (docs/data/research content binding).
    pub(super) artifact_excerpt: Option<&'a str>,
}

impl<'a> BindingProbe<'a> {
    /// Construct a probe carrying only the diagnostic; all other facts empty.
    pub(super) fn for_diagnostic(verifier_diagnostic: &'a str) -> Self {
        Self {
            verifier_diagnostic,
            cargo_manifest: None,
            node_manifest: None,
            present_test_files: &[],
            referenced_bin_exe_names: &[],
            expected_crate_name: None,
            artifact_excerpt: None,
        }
    }

    fn rust_signal(&self) -> bool {
        if self.cargo_manifest.is_some() {
            return true;
        }
        if self
            .present_test_files
            .iter()
            .any(|p| p.to_ascii_lowercase().ends_with(".rs"))
        {
            return true;
        }
        let lower = self.verifier_diagnostic.to_ascii_lowercase();
        lower.contains("cargo") || lower.contains("error[e0") || lower.contains("rustc")
    }

    fn node_signal(&self) -> bool {
        if self.node_manifest.is_some() {
            return true;
        }
        if self.present_test_files.iter().any(|p| is_node_test_path(p)) {
            return true;
        }
        let lower = self.verifier_diagnostic.to_ascii_lowercase();
        lower.contains("npm")
            || lower.contains("node --test")
            || lower.contains("jest")
            || lower.contains("vitest")
    }
}

/// One detected binding mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BindingMismatch {
    pub(super) check: BindingCheckKind,
    /// The deterministic operator that can repair it, if applicable here.
    pub(super) operator: Option<RepairOperatorId>,
    pub(super) detail: &'static str,
}

impl BindingMismatch {
    fn new(
        check: BindingCheckKind,
        operator: Option<RepairOperatorId>,
        detail: &'static str,
    ) -> Self {
        Self {
            check,
            operator,
            detail,
        }
    }
}

/// The outcome of binding the deliverable to its evidence runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BindingResolution {
    /// Every check binds; the evidence runner can execute.
    Bound,
    /// One or more checks failed to bind (in plan order).
    Mismatched(Vec<BindingMismatch>),
}

impl BindingResolution {
    pub(super) fn is_bound(&self) -> bool {
        matches!(self, BindingResolution::Bound)
    }

    pub(super) fn mismatches(&self) -> &[BindingMismatch] {
        match self {
            BindingResolution::Bound => &[],
            BindingResolution::Mismatched(mismatches) => mismatches,
        }
    }

    /// The deterministic operator to run first, by [`RepairOperatorId::registry_order`]
    /// precedence (not plan order), so the resolution matches the live dispatch.
    pub(super) fn recoverable_operator(&self) -> Option<RepairOperatorId> {
        RepairOperatorId::registry_order().into_iter().find(|op| {
            self.mismatches()
                .iter()
                .any(|mismatch| mismatch.operator == Some(*op))
        })
    }

    /// Terminal projection: an unbound deliverable is `evidence_binding_failed`,
    /// not `evidence_runner_missing` / generic `repair_exhausted` (#988
    /// expectation 1: Rust `safe_stop_verifier_missing` moves to
    /// `evidence_binding_failed`). A bound deliverable has no terminal — the
    /// runner proceeds.
    pub(super) fn generic_terminal_state(&self) -> Option<GenericTerminalState> {
        match self {
            BindingResolution::Bound => None,
            BindingResolution::Mismatched(_) => Some(GenericTerminalState::EvidenceBindingFailed),
        }
    }
}

/// The front stage of the evidence lifecycle: the ordered set of binding checks
/// a task kind's evidence runner depends on, plus the classifier that resolves
/// them against a [`BindingProbe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvidenceBindingPlan {
    task_kind: TaskKind,
    checks: Vec<BindingCheckKind>,
}

impl EvidenceBindingPlan {
    /// The binding plan for a task kind. Coding carries the union of Rust and
    /// Node checks; [`BindingProbe`] runtime signals gate which actually fire,
    /// so a pure-Rust run never reports a Node mismatch and vice versa.
    pub(super) fn for_task_kind(task_kind: TaskKind) -> Self {
        let checks = match task_kind {
            TaskKind::Coding => vec![
                BindingCheckKind::CargoManifestPresent,
                BindingCheckKind::CargoDependencyDeclared,
                BindingCheckKind::CargoLibTargetName,
                BindingCheckKind::CargoBinTargetName,
                BindingCheckKind::CargoIntegrationTestPresent,
                BindingCheckKind::NodeManifestPresent,
                BindingCheckKind::NodeTestScriptDeclared,
            ],
            TaskKind::Docs | TaskKind::Authoring => vec![BindingCheckKind::DocsContentBound],
            TaskKind::Data => vec![BindingCheckKind::DataSchemaBound],
            TaskKind::Research => vec![BindingCheckKind::ResearchCitationBound],
            // Ops evidence is a command observation with no static binding check.
            TaskKind::Ops => Vec::new(),
        };
        Self { task_kind, checks }
    }

    pub(super) fn task_kind(&self) -> TaskKind {
        self.task_kind
    }

    pub(super) fn checks(&self) -> &[BindingCheckKind] {
        &self.checks
    }

    /// Resolve every check against the probe, collecting mismatches in plan
    /// order. Rust and Node share this single path (#988 acceptance: Rust
    /// binding mismatch and Node manifest/test ordering on one lifecycle).
    pub(super) fn bind(&self, probe: &BindingProbe<'_>) -> BindingResolution {
        let mismatches: Vec<BindingMismatch> = self
            .checks
            .iter()
            .filter_map(|check| evaluate_check(*check, probe))
            .collect();
        if mismatches.is_empty() {
            BindingResolution::Bound
        } else {
            BindingResolution::Mismatched(mismatches)
        }
    }
}

fn evaluate_check(check: BindingCheckKind, probe: &BindingProbe<'_>) -> Option<BindingMismatch> {
    match check {
        BindingCheckKind::CargoManifestPresent => {
            (probe.rust_signal() && probe.cargo_manifest.is_none()).then(|| {
                BindingMismatch::new(
                    check,
                    None,
                    "rust verifier signal but no Cargo.toml manifest",
                )
            })
        }
        BindingCheckKind::CargoDependencyDeclared => {
            let crates = undeclared_known_crates(probe.verifier_diagnostic);
            if crates.is_empty() {
                return None;
            }
            match probe.cargo_manifest {
                // The operator can complete the manifest deterministically.
                Some(raw) if complete_cargo_dependencies(raw, &crates).is_some() => {
                    Some(BindingMismatch::new(
                        check,
                        check.recommended_operator(),
                        "declared serde-family crate is undeclared in [dependencies]",
                    ))
                }
                // Already declared -> not a mismatch.
                Some(_) => None,
                // No manifest to complete: still a binding mismatch, but no
                // deterministic operator applies here.
                None => Some(BindingMismatch::new(
                    check,
                    None,
                    "serde-family crate undeclared and no manifest to complete",
                )),
            }
        }
        BindingCheckKind::CargoLibTargetName => {
            match (probe.cargo_manifest, probe.expected_crate_name) {
                (Some(raw), Some(expected)) => match cargo_lib_target_name(raw) {
                    Some(actual) if actual != expected => Some(BindingMismatch::new(
                        check,
                        None,
                        "[lib] name does not bind the expected crate name",
                    )),
                    _ => None,
                },
                _ => None,
            }
        }
        BindingCheckKind::CargoBinTargetName => {
            if probe.referenced_bin_exe_names.is_empty() {
                return None;
            }
            match probe.cargo_manifest {
                Some(raw) => {
                    let declared = cargo_bin_target_names(raw);
                    let missing = probe
                        .referenced_bin_exe_names
                        .iter()
                        .any(|name| !declared.iter().any(|d| d == name));
                    missing.then(|| {
                        BindingMismatch::new(
                            check,
                            None,
                            "CARGO_BIN_EXE_* has no matching [[bin]] name",
                        )
                    })
                }
                None => Some(BindingMismatch::new(
                    check,
                    None,
                    "CARGO_BIN_EXE_* referenced but no manifest declares [[bin]]",
                )),
            }
        }
        BindingCheckKind::CargoIntegrationTestPresent => {
            let needs = probe.rust_signal()
                && diagnostic_mentions_integration_test(probe.verifier_diagnostic);
            let has = probe
                .present_test_files
                .iter()
                .any(|path| is_rust_integration_test_path(path));
            (needs && !has).then(|| {
                BindingMismatch::new(
                    check,
                    None,
                    "integration test target expected but no tests/*.rs present",
                )
            })
        }
        BindingCheckKind::NodeManifestPresent => {
            (probe.node_signal() && probe.node_manifest.is_none()).then(|| {
                BindingMismatch::new(
                    check,
                    check.recommended_operator(),
                    "node test signal but no package.json manifest",
                )
            })
        }
        BindingCheckKind::NodeTestScriptDeclared => match probe.node_manifest {
            Some(raw) if probe.node_signal() => {
                match complete_node_test_runner_manifest(Some(raw)) {
                    Some(completion) if completion.action == NodeManifestAction::AddTestScript => {
                        Some(BindingMismatch::new(
                            check,
                            check.recommended_operator(),
                            "package.json lacks a usable scripts.test",
                        ))
                    }
                    // Already bindable, or malformed (operator declines) -> no
                    // deterministic mismatch here.
                    _ => None,
                }
            }
            _ => None,
        },
        BindingCheckKind::DocsContentBound
        | BindingCheckKind::DataSchemaBound
        | BindingCheckKind::ResearchCitationBound => {
            // Issue #993 owns the detailed content/schema/citation binding. The
            // boundary already classifies an absent/empty artifact excerpt as a
            // binding mismatch (deferred to the LLM pass); present content binds.
            let bound = probe
                .artifact_excerpt
                .is_some_and(|excerpt| !excerpt.trim().is_empty());
            (!bound).then(|| {
                BindingMismatch::new(check, None, "non-coding artifact evidence is not yet bound")
            })
        }
    }
}

/// Extract `[lib] name = "..."` from a Cargo manifest. Dependency-free
/// line scanner (no `toml` crate; local-first). Returns `None` when there is no
/// `[lib]` table or it declares no `name`.
fn cargo_lib_target_name(manifest: &str) -> Option<String> {
    section_string_value(manifest, "[lib]", "name")
        .into_iter()
        .next()
}

/// Extract every `[[bin]] name = "..."` from a Cargo manifest, in declaration
/// order. Used to bind `CARGO_BIN_EXE_*` references.
fn cargo_bin_target_names(manifest: &str) -> Vec<String> {
    section_string_value(manifest, "[[bin]]", "name")
}

/// Collect the string value of `key` inside every table whose header line
/// (trimmed, comments stripped) equals `header`. One result per matching table.
fn section_string_value(manifest: &str, header: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut in_section = false;
    for line in manifest.lines() {
        let trimmed = strip_inline_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_table_header(trimmed) {
            in_section = trimmed == header;
            continue;
        }
        if in_section && let Some(value) = parse_string_assignment(trimmed, key) {
            values.push(value);
        }
    }
    values
}

fn is_table_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

/// Drop a trailing `# comment`. A `#` inside a quoted string is not a comment;
/// the manifests this scanner reads do not put `#` inside `name`/`version`
/// strings, so a simple first-`#` split is sufficient and conservative.
fn strip_inline_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Parse `key = "value"` (TOML basic string). Returns the unquoted value.
fn parse_string_assignment(trimmed: &str, key: &str) -> Option<String> {
    let (lhs, rhs) = trimmed.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    let rhs = rhs.trim();
    let inner = rhs.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_string())
}

fn diagnostic_mentions_integration_test(diagnostic: &str) -> bool {
    let lower = diagnostic.to_ascii_lowercase();
    lower.contains("tests/") || lower.contains("integration test")
}

fn is_rust_integration_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/") && lower.ends_with(".rs")
}

fn is_node_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    const SUFFIXES: &[&str] = &[
        ".test.js",
        ".test.mjs",
        ".test.cjs",
        ".test.ts",
        ".spec.js",
        ".spec.mjs",
        ".spec.ts",
    ];
    if SUFFIXES.iter().any(|suffix| lower.ends_with(suffix)) {
        return true;
    }
    (lower.starts_with("test/") || lower.contains("/test/"))
        && (lower.ends_with(".js") || lower.ends_with(".mjs"))
}

/// Decomposition of the coarse `repair_exhausted` terminal into its actual
/// sub-cause (#988 acceptance 1 / expectation 3). The legacy label is kept as a
/// compatibility projection; the generic terminal vocabulary carries the richer
/// classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairExhaustionClass {
    /// The same target kept producing the same diagnostic after repair.
    SameTargetSameDiagnostic,
    /// No deterministic operator applied and the LLM pass could not converge.
    OperatorMissing,
    /// Evidence binding still failed after the repair edits applied.
    BindingFailedAfterRepair,
    /// The impl/test/evidence contract is ambiguous and unresolved (#994).
    ContractConflictUnresolved,
}

impl RepairExhaustionClass {
    /// The decomposed label (#988 expectation 3).
    pub(super) fn label(self) -> &'static str {
        match self {
            RepairExhaustionClass::SameTargetSameDiagnostic => "same_target_exhausted",
            RepairExhaustionClass::OperatorMissing => "operator_missing",
            RepairExhaustionClass::BindingFailedAfterRepair => "binding_failed_after_repair",
            RepairExhaustionClass::ContractConflictUnresolved => "contract_conflict",
        }
    }

    /// The compatibility projection: every decomposed class still came from the
    /// `repair_exhausted` terminal, so the legacy eval label is preserved
    /// (#988 non-functional: legacy terminal label kept as compat projection).
    pub(super) fn legacy_label(self) -> &'static str {
        "repair_exhausted"
    }

    /// Project onto the generic terminal vocabulary (#947). A binding failure
    /// surfaces as `evidence_binding_failed`; a contract conflict is a
    /// controlled safe stop; the others remain repair exhaustion.
    pub(super) fn generic_terminal_state(self) -> GenericTerminalState {
        match self {
            RepairExhaustionClass::SameTargetSameDiagnostic
            | RepairExhaustionClass::OperatorMissing => {
                GenericTerminalState::EvidenceRepairExhausted
            }
            RepairExhaustionClass::BindingFailedAfterRepair => {
                GenericTerminalState::EvidenceBindingFailed
            }
            RepairExhaustionClass::ContractConflictUnresolved => {
                GenericTerminalState::EvidenceRepairSafeStop
            }
        }
    }
}

/// Structured inputs for [`classify_repair_exhaustion`]. Each flag is an
/// observation the repair controller already has (post-rerun delta, operator
/// applicability, contract ambiguity); the classifier is a pure decision tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct RepairExhaustionSignal {
    /// The post-edit rerun reproduced the same diagnostic on the same target
    /// (the #987 `TargetReassessmentRequired` shape).
    pub(super) same_target_same_diagnostic: bool,
    /// Evidence binding was still unbound after the repair edits applied.
    pub(super) binding_failed_after_repair: bool,
    /// A deterministic operator was available for this failure.
    pub(super) operator_available: bool,
    /// The impl/test/evidence contract is ambiguous (no authority to arbitrate).
    pub(super) contract_conflict: bool,
}

/// Classify a `repair_exhausted` terminal into its sub-cause. Most-specific
/// cause wins: contract conflict > binding-failed-after-repair >
/// same-target-same-diagnostic > operator-missing. When a deterministic
/// operator was available but the run still exhausted (the operator did not
/// resolve it), the cause is treated as same-target stagnation.
pub(super) fn classify_repair_exhaustion(signal: &RepairExhaustionSignal) -> RepairExhaustionClass {
    if signal.contract_conflict {
        RepairExhaustionClass::ContractConflictUnresolved
    } else if signal.binding_failed_after_repair {
        RepairExhaustionClass::BindingFailedAfterRepair
    } else if signal.same_target_same_diagnostic {
        RepairExhaustionClass::SameTargetSameDiagnostic
    } else if !signal.operator_available {
        RepairExhaustionClass::OperatorMissing
    } else {
        RepairExhaustionClass::SameTargetSameDiagnostic
    }
}

/// A repeated no-progress observation: the same target/role failed `repeated_count`
/// times with no diagnostic movement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoProgressSignal {
    pub(super) role: ArtifactRole,
    pub(super) path: String,
    pub(super) repeated_count: u32,
}

/// The structured recovery for a no-progress signal: ban the offending
/// target/role and force a switch to a different role on the next diagnostic
/// (#988 acceptance 2). Replaces the #987 blind `Replan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoProgressRecovery {
    /// The role to ban on the next diagnostic pass.
    pub(super) banned_role: ArtifactRole,
    /// The (masked) target path to ban. Masked at construction because it is an
    /// external-origin string that may flow into later payloads (Security
    /// Invariant: every external string passes `mask_secrets`).
    pub(super) banned_target_path: String,
    /// The role the diagnostic is forced onto instead. Always differs from the
    /// banned role (the whole point of the switch).
    pub(super) forced_role_switch: ArtifactRole,
}

/// Policy that converts a same-target/same-role no-progress signal into a ban
/// payload plus a forced role switch.
#[derive(Debug, Clone, Copy)]
pub(super) struct NoProgressRecoveryPolicy;

impl NoProgressRecoveryPolicy {
    /// Returns a recovery once the no-progress threshold is reached, otherwise
    /// `None` (let the normal repair flow continue).
    pub(super) fn recover(signal: &NoProgressSignal) -> Option<NoProgressRecovery> {
        if signal.repeated_count < NO_PROGRESS_THRESHOLD {
            return None;
        }
        Some(NoProgressRecovery {
            banned_role: signal.role,
            banned_target_path: mask_secrets(&signal.path),
            forced_role_switch: forced_role_switch_for(signal.role),
        })
    }
}

/// Deterministic forced role switch. When the same role keeps stalling, the
/// spec authority is likely the *other* side of the impl/test pair, so the
/// diagnostic is forced onto a different role. Always returns a role distinct
/// from the input.
fn forced_role_switch_for(role: ArtifactRole) -> ArtifactRole {
    match role {
        ArtifactRole::Implementation => ArtifactRole::Test,
        ArtifactRole::Test => ArtifactRole::Implementation,
        ArtifactRole::UsageDocs | ArtifactRole::Setup | ArtifactRole::DataOutput => {
            ArtifactRole::Implementation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_TASK_KINDS: [TaskKind; 6] = [
        TaskKind::Coding,
        TaskKind::Docs,
        TaskKind::Data,
        TaskKind::Research,
        TaskKind::Ops,
        TaskKind::Authoring,
    ];

    const ALL_BINDING_CHECKS: [BindingCheckKind; 10] = [
        BindingCheckKind::CargoManifestPresent,
        BindingCheckKind::CargoDependencyDeclared,
        BindingCheckKind::CargoLibTargetName,
        BindingCheckKind::CargoBinTargetName,
        BindingCheckKind::CargoIntegrationTestPresent,
        BindingCheckKind::NodeManifestPresent,
        BindingCheckKind::NodeTestScriptDeclared,
        BindingCheckKind::DocsContentBound,
        BindingCheckKind::DataSchemaBound,
        BindingCheckKind::ResearchCitationBound,
    ];

    // ----- Replay fixture 052: Rust serde dependency undeclared -----

    #[test]
    fn replay_052_rust_serde_dependency_binds_to_cargo_operator() {
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
        let diagnostic = "error[E0432]: unresolved import `serde`\n  --> src/lib.rs:1:5\n   use of undeclared crate or module `serde`";
        let probe = BindingProbe {
            cargo_manifest: Some(manifest),
            present_test_files: &["tests/lib.rs"],
            ..BindingProbe::for_diagnostic(diagnostic)
        };

        let plan = EvidenceBindingPlan::for_task_kind(TaskKind::Coding);
        let resolution = plan.bind(&probe);

        assert!(!resolution.is_bound());
        let dep_mismatch = resolution
            .mismatches()
            .iter()
            .find(|m| m.check == BindingCheckKind::CargoDependencyDeclared)
            .expect("serde dependency mismatch is detected");
        assert_eq!(
            dep_mismatch.operator,
            Some(RepairOperatorId::CargoDependency)
        );
        assert_eq!(
            resolution.recoverable_operator(),
            Some(RepairOperatorId::CargoDependency)
        );
        // Reclassified as a binding failure, not generic repair exhaustion.
        assert_eq!(
            resolution.generic_terminal_state(),
            Some(GenericTerminalState::EvidenceBindingFailed)
        );
        // Pure-Rust probe never reports a Node mismatch.
        assert!(
            resolution
                .mismatches()
                .iter()
                .all(|m| m.check.evidence_runner_kind() == EvidenceRunnerKind::CodingBuildTest)
        );
        assert!(!resolution.mismatches().iter().any(|m| matches!(
            m.check,
            BindingCheckKind::NodeManifestPresent | BindingCheckKind::NodeTestScriptDeclared
        )));
    }

    #[test]
    fn rust_serde_already_declared_binds_clean() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n";
        let diagnostic =
            "error[E0432]: unresolved import `serde`\nuse of undeclared crate or module `serde`";
        let probe = BindingProbe {
            cargo_manifest: Some(manifest),
            ..BindingProbe::for_diagnostic(diagnostic)
        };
        let plan = EvidenceBindingPlan::for_task_kind(TaskKind::Coding);
        // The dep is declared, so the dependency check does not mismatch.
        let resolution = plan.bind(&probe);
        assert!(
            !resolution
                .mismatches()
                .iter()
                .any(|m| m.check == BindingCheckKind::CargoDependencyDeclared)
        );
    }

    // ----- Rust [lib]/[[bin]] / CARGO_BIN_EXE binding mismatch -----

    #[test]
    fn rust_lib_name_mismatch_is_classified_binding() {
        let manifest = "[package]\nname = \"app\"\n\n[lib]\nname = \"wrong_crate\"\n";
        let probe = BindingProbe {
            cargo_manifest: Some(manifest),
            expected_crate_name: Some("app_core"),
            present_test_files: &["tests/it.rs"],
            ..BindingProbe::for_diagnostic("cargo test failed")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        let mismatch = resolution
            .mismatches()
            .iter()
            .find(|m| m.check == BindingCheckKind::CargoLibTargetName)
            .expect("lib name mismatch detected");
        // No deterministic operator yet (#991), but it is classified as binding.
        assert_eq!(mismatch.operator, None);
        assert_eq!(
            resolution.generic_terminal_state(),
            Some(GenericTerminalState::EvidenceBindingFailed)
        );
    }

    #[test]
    fn cargo_bin_exe_without_matching_bin_target_is_binding_mismatch() {
        let manifest = "[package]\nname = \"app\"\n\n[[bin]]\nname = \"server\"\n";
        let probe = BindingProbe {
            cargo_manifest: Some(manifest),
            referenced_bin_exe_names: &["cli"],
            present_test_files: &["tests/cli.rs"],
            ..BindingProbe::for_diagnostic("cargo test failed")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        assert!(
            resolution
                .mismatches()
                .iter()
                .any(|m| m.check == BindingCheckKind::CargoBinTargetName)
        );
    }

    #[test]
    fn cargo_bin_exe_with_matching_bin_target_binds() {
        let manifest =
            "[package]\nname = \"app\"\n\n[[bin]]\nname = \"cli\"\n\n[[bin]]\nname = \"server\"\n";
        let probe = BindingProbe {
            cargo_manifest: Some(manifest),
            referenced_bin_exe_names: &["cli"],
            ..BindingProbe::for_diagnostic("cargo test failed")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        assert!(
            !resolution
                .mismatches()
                .iter()
                .any(|m| m.check == BindingCheckKind::CargoBinTargetName)
        );
    }

    #[test]
    fn cargo_target_name_parsers_extract_declared_names() {
        let manifest = "[package]\nname = \"app\" # crate\n\n[lib]\nname = \"app_core\"\n\n[[bin]]\nname = \"cli\"\n\n[[bin]]\nname = \"server\"\n";
        assert_eq!(cargo_lib_target_name(manifest).as_deref(), Some("app_core"));
        assert_eq!(cargo_bin_target_names(manifest), vec!["cli", "server"]);
        assert_eq!(cargo_lib_target_name("[package]\nname = \"x\"\n"), None);
    }

    // ----- Replay fixtures 043 / 048 / 058: Node manifest/test ordering -----

    #[test]
    fn replay_048_node_missing_manifest_binds_to_node_operator() {
        // Tests exist but package.json is missing (fixtures 048 / 058).
        let probe = BindingProbe {
            present_test_files: &["app.test.mjs"],
            ..BindingProbe::for_diagnostic("npm test could not find package.json")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        let mismatch = resolution
            .mismatches()
            .iter()
            .find(|m| m.check == BindingCheckKind::NodeManifestPresent)
            .expect("node manifest mismatch detected");
        assert_eq!(mismatch.operator, Some(RepairOperatorId::NodeManifest));
        assert_eq!(
            resolution.recoverable_operator(),
            Some(RepairOperatorId::NodeManifest)
        );
        // Node-only probe never reports a Rust mismatch.
        assert!(!resolution.mismatches().iter().any(|m| matches!(
            m.check,
            BindingCheckKind::CargoManifestPresent | BindingCheckKind::CargoDependencyDeclared
        )));
    }

    #[test]
    fn replay_043_node_manifest_without_test_script_binds_to_node_operator() {
        // package.json exists but scripts.test is missing (fixture 043).
        let manifest = r#"{"name":"app","version":"1.0.0","type":"module"}"#;
        let probe = BindingProbe {
            node_manifest: Some(manifest),
            present_test_files: &["app.test.mjs"],
            ..BindingProbe::for_diagnostic("npm test: missing script: test")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        let mismatch = resolution
            .mismatches()
            .iter()
            .find(|m| m.check == BindingCheckKind::NodeTestScriptDeclared)
            .expect("node test-script mismatch detected");
        assert_eq!(mismatch.operator, Some(RepairOperatorId::NodeManifest));
    }

    #[test]
    fn node_manifest_with_test_script_binds_clean() {
        let manifest = r#"{"name":"app","scripts":{"test":"node --test"}}"#;
        let probe = BindingProbe {
            node_manifest: Some(manifest),
            present_test_files: &["app.test.mjs"],
            ..BindingProbe::for_diagnostic("npm test")
        };
        let resolution = EvidenceBindingPlan::for_task_kind(TaskKind::Coding).bind(&probe);
        assert!(resolution.is_bound());
        assert_eq!(resolution.generic_terminal_state(), None);
    }

    #[test]
    fn rust_and_node_share_one_binding_plan_lifecycle() {
        // Same plan object resolves both runtimes (#988 acceptance 3).
        let plan = EvidenceBindingPlan::for_task_kind(TaskKind::Coding);

        let rust = plan.bind(&BindingProbe {
            cargo_manifest: Some("[package]\nname = \"app\"\n"),
            ..BindingProbe::for_diagnostic(
                "error[E0433]: failed to resolve: use of undeclared crate or module `serde_json`",
            )
        });
        assert_eq!(
            rust.recoverable_operator(),
            Some(RepairOperatorId::CargoDependency)
        );

        let node = plan.bind(&BindingProbe {
            present_test_files: &["app.test.mjs"],
            ..BindingProbe::for_diagnostic("npm test could not find package.json")
        });
        assert_eq!(
            node.recoverable_operator(),
            Some(RepairOperatorId::NodeManifest)
        );
    }

    // ----- Generic enum / registry boundary (AC4) -----

    #[test]
    fn binding_plan_covers_every_task_kind() {
        for kind in ALL_TASK_KINDS {
            let plan = EvidenceBindingPlan::for_task_kind(kind);
            assert_eq!(plan.task_kind(), kind);
            // Ops has a command observation, not a static binding check.
            if kind == TaskKind::Ops {
                assert!(plan.checks().is_empty());
            } else {
                assert!(!plan.checks().is_empty());
            }
        }
    }

    #[test]
    fn binding_checks_project_onto_runner_vocabulary() {
        for check in ALL_BINDING_CHECKS {
            // Every check maps to a real runner kind and a stable label.
            let _ = check.evidence_runner_kind();
            assert!(!check.as_str().is_empty());
        }
        assert_eq!(
            BindingCheckKind::DocsContentBound.evidence_runner_kind(),
            EvidenceRunnerKind::DocsContentCheck
        );
        assert_eq!(
            BindingCheckKind::DataSchemaBound.evidence_runner_kind(),
            EvidenceRunnerKind::DataSchemaCheck
        );
        assert_eq!(
            BindingCheckKind::ResearchCitationBound.evidence_runner_kind(),
            EvidenceRunnerKind::ResearchSourceFetch
        );
    }

    #[test]
    fn docs_data_research_extension_axis_classifies_unbound_content() {
        for (kind, check) in [
            (TaskKind::Docs, BindingCheckKind::DocsContentBound),
            (TaskKind::Data, BindingCheckKind::DataSchemaBound),
            (TaskKind::Research, BindingCheckKind::ResearchCitationBound),
        ] {
            let plan = EvidenceBindingPlan::for_task_kind(kind);
            // Empty excerpt -> binding mismatch.
            let unbound = plan.bind(&BindingProbe::for_diagnostic(""));
            assert!(
                unbound.mismatches().iter().any(|m| m.check == check),
                "{kind:?} reports an unbound content mismatch"
            );
            // Present excerpt -> bound (detailed logic deferred to #993).
            let bound = plan.bind(&BindingProbe {
                artifact_excerpt: Some("## Section\nReal content."),
                ..BindingProbe::for_diagnostic("")
            });
            assert!(bound.is_bound(), "{kind:?} binds with present content");
        }
    }

    #[test]
    fn operator_registry_and_binding_check_mapping_are_consistent() {
        // registry_order is the dispatch precedence and has no duplicates.
        let order = RepairOperatorId::registry_order();
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], RepairOperatorId::CargoDependency);

        // recommended_operator and binding_check are inverse for binding-shaped
        // operators (single source of truth for the check<->operator mapping).
        assert_eq!(
            BindingCheckKind::CargoDependencyDeclared.recommended_operator(),
            Some(RepairOperatorId::CargoDependency)
        );
        assert_eq!(
            RepairOperatorId::CargoDependency.binding_check(),
            Some(BindingCheckKind::CargoDependencyDeclared)
        );
        // The compile operator is not binding-shaped.
        assert_eq!(RepairOperatorId::MechanicalCompile.binding_check(), None);
        for op in order {
            assert!(!op.as_str().is_empty());
        }
    }

    // ----- repair_exhausted decomposition (AC1 / expectation 3) -----

    #[test]
    fn repair_exhaustion_decomposes_by_most_specific_cause() {
        let cases = [
            (
                RepairExhaustionSignal {
                    contract_conflict: true,
                    binding_failed_after_repair: true,
                    same_target_same_diagnostic: true,
                    operator_available: true,
                },
                RepairExhaustionClass::ContractConflictUnresolved,
                "contract_conflict",
            ),
            (
                RepairExhaustionSignal {
                    binding_failed_after_repair: true,
                    same_target_same_diagnostic: true,
                    ..Default::default()
                },
                RepairExhaustionClass::BindingFailedAfterRepair,
                "binding_failed_after_repair",
            ),
            (
                RepairExhaustionSignal {
                    same_target_same_diagnostic: true,
                    operator_available: true,
                    ..Default::default()
                },
                RepairExhaustionClass::SameTargetSameDiagnostic,
                "same_target_exhausted",
            ),
            (
                RepairExhaustionSignal::default(),
                RepairExhaustionClass::OperatorMissing,
                "operator_missing",
            ),
        ];
        for (signal, expected, label) in cases {
            let class = classify_repair_exhaustion(&signal);
            assert_eq!(class, expected);
            assert_eq!(class.label(), label);
            // Legacy eval label is always preserved.
            assert_eq!(class.legacy_label(), "repair_exhausted");
        }
    }

    #[test]
    fn repair_exhaustion_projects_onto_generic_terminal_states() {
        assert_eq!(
            RepairExhaustionClass::SameTargetSameDiagnostic.generic_terminal_state(),
            GenericTerminalState::EvidenceRepairExhausted
        );
        assert_eq!(
            RepairExhaustionClass::OperatorMissing.generic_terminal_state(),
            GenericTerminalState::EvidenceRepairExhausted
        );
        assert_eq!(
            RepairExhaustionClass::BindingFailedAfterRepair.generic_terminal_state(),
            GenericTerminalState::EvidenceBindingFailed
        );
        assert_eq!(
            RepairExhaustionClass::ContractConflictUnresolved.generic_terminal_state(),
            GenericTerminalState::EvidenceRepairSafeStop
        );
    }

    // ----- NoProgressRecoveryPolicy: ban payload + forced role switch (AC2) ----

    #[test]
    fn no_progress_below_threshold_does_not_ban() {
        let signal = NoProgressSignal {
            role: ArtifactRole::Implementation,
            path: "src/lib.rs".to_string(),
            repeated_count: 1,
        };
        assert_eq!(NoProgressRecoveryPolicy::recover(&signal), None);
    }

    #[test]
    fn no_progress_at_threshold_bans_target_and_forces_role_switch() {
        let signal = NoProgressSignal {
            role: ArtifactRole::Implementation,
            path: "src/lib.rs".to_string(),
            repeated_count: 2,
        };
        let recovery =
            NoProgressRecoveryPolicy::recover(&signal).expect("threshold reached -> recovery");
        assert_eq!(recovery.banned_role, ArtifactRole::Implementation);
        assert_eq!(recovery.banned_target_path, "src/lib.rs");
        // Forced switch is always a different role.
        assert_ne!(recovery.forced_role_switch, recovery.banned_role);
        assert_eq!(recovery.forced_role_switch, ArtifactRole::Test);
    }

    #[test]
    fn forced_role_switch_is_total_and_always_changes_role() {
        for role in ArtifactRole::all() {
            let switched = forced_role_switch_for(role);
            assert_ne!(switched, role, "{role:?} switches to a different role");
        }
        assert_eq!(
            forced_role_switch_for(ArtifactRole::Test),
            ArtifactRole::Implementation
        );
        assert_eq!(
            forced_role_switch_for(ArtifactRole::UsageDocs),
            ArtifactRole::Implementation
        );
        assert_eq!(
            forced_role_switch_for(ArtifactRole::DataOutput),
            ArtifactRole::Implementation
        );
    }

    #[test]
    fn no_progress_masks_external_target_path() {
        // A secret-looking path token is masked at construction (Security
        // Invariant); ordinary paths pass through unchanged.
        let signal = NoProgressSignal {
            role: ArtifactRole::Setup,
            path: "config/api_key=sk-ABCDEFGHIJKLMNOPQRSTUVWX".to_string(),
            repeated_count: 3,
        };
        let recovery = NoProgressRecoveryPolicy::recover(&signal).expect("recovery");
        assert_eq!(
            recovery.banned_target_path,
            mask_secrets("config/api_key=sk-ABCDEFGHIJKLMNOPQRSTUVWX")
        );
    }
}
