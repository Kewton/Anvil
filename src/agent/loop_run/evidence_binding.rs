//! Issue #988 umbrella plus child issues #989/#993: evidence binding lifecycle.
//!
//! Structures the binding between a deliverable's evidence runner and the
//! artifacts it depends on *before* the runner executes, so a binding gap is
//! classified as a structured `evidence_binding_failed` observation instead of
//! being rolled up into `safe_stop_verifier_missing` or a generic
//! `repair_exhausted`.
//!
//! The core types (`EvidenceBindingPlan` / `BindingCheck` /
//! `EvidenceBindingStatus` / `BindingCheckKind`) are runtime-neutral so the same
//! shape extends to Node manifest/test ordering, docs content, data schema, and
//! research citation binding. Runtime differences live in adapters (`rust_*` /
//! `node_*` / `docs_*` / `data_*` / `research_*` free functions) and the generic
//! [`BindingFailureCheck`] recovery routing axis, not in provider-specific
//! control flow.
//!
//! Issue #1004 adds the controller hook [`evidence_binding_plan_after_scaffold`]:
//! the lifecycle runs it *after scaffold materialization* and *before* the
//! EvidenceRunner so a binding gap is classified as a structured
//! `evidence_binding_failed` observation (routed to `EvidenceBindingFailedJob`
//! via [`EvidenceBindingPlan::recovery_job_kind`]) instead of a generic evidence
//! failure. The dispatch is runtime-neutral: it unions the per-runtime adapters
//! so the Rust/Node branches never leak into the lifecycle center. The structured
//! `BindingCheck` vocabulary covers manifest / public-API import / executable
//! handle / test script / required sections / schema / citation / file layout.
//!
//! Like `evidence_runner.rs`, this module is an extension seam: the focused
//! in-crate tests pin the shape before broad callers are wired, so the
//! production-facing functions are `#[allow(dead_code)]` for now. `pub(super)`
//! limited / no facade re-export (DR3-001).

#![allow(dead_code)] // Extension seam (parent #988); focused tests pin the shape.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::active_job_arbiter::RecoveryJobKind;
use super::evidence_runner::{EvidenceRunnerError, EvidenceRunnerKind};
use super::generated_test_guard::{parse_toml_string_value, strip_toml_comment};
use super::node_runner_manifest::complete_node_test_runner_manifest;
use super::task_contract::TaskKind;
use crate::terminal_outcome::GenericTerminalState;

/// Whether a single binding check (or the whole plan) resolved.
///
/// Runtime-neutral: a docs/data/research adapter reuses the same three states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceBindingStatus {
    /// The referenced symbol/handle/identity binds to a declared one.
    Bound,
    /// A required binding could not be resolved (a mismatch).
    Unbound,
    /// Not enough information to decide (no manifest reference, no checks).
    /// Neutral — never routes to a failure.
    Indeterminate,
}

impl EvidenceBindingStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            EvidenceBindingStatus::Bound => "bound",
            EvidenceBindingStatus::Unbound => "unbound",
            EvidenceBindingStatus::Indeterminate => "indeterminate",
        }
    }
}

/// The kind of binding a check covers. Generic vocabulary so non-coding
/// adapters can map their own bindings onto the same enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingCheckKind {
    /// The project descriptor declares a usable identity.
    /// Rust: `Cargo.toml` has a non-empty `[package] name`.
    /// Node: `package.json` exists.
    ManifestIdentity,
    /// A referenced import/symbol resolves to a declared identity.
    /// Rust: `use <crate>::…` ↔ `[lib] name` (default lib = package name).
    ImportSymbol,
    /// A referenced executable handle resolves to a declared target.
    /// Rust: `env!("CARGO_BIN_EXE_<name>")` / `cargo_bin("<name>")` ↔ `[[bin]] name`
    /// (default bin = package name).
    ExecutableHandle,
    /// A runnable test command binds.
    /// Node: `package.json` declares a usable `scripts.test`.
    TestScript,
    /// A required document section binds to the produced document.
    /// Docs: `## Setup` ↔ a section observed in the rendered document.
    RequiredSection,
    /// A declared schema column binds to the produced output.
    /// Data: a `[schema] columns` entry ↔ a column observed in the output.
    SchemaColumn,
    /// A citation binds to an available source.
    /// Research: a cited reference ↔ a fetched/declared source.
    Citation,
    /// A required file/path the evidence runner depends on exists.
    /// Generic: the document / output / notes the non-coding runner binds to.
    FileLayout,
}

impl BindingCheckKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingCheckKind::ManifestIdentity => "manifest_identity",
            BindingCheckKind::ImportSymbol => "import_symbol",
            BindingCheckKind::ExecutableHandle => "executable_handle",
            BindingCheckKind::TestScript => "test_script",
            BindingCheckKind::RequiredSection => "required_section",
            BindingCheckKind::SchemaColumn => "schema_column",
            BindingCheckKind::Citation => "citation",
            BindingCheckKind::FileLayout => "file_layout",
        }
    }
}

/// Which deliverable -> evidence-runner family cannot bind.
///
/// This is separate from [`BindingCheck`], which is the structured per-reference
/// observation used by [`EvidenceBindingPlan`]. `BindingFailureCheck` is the
/// generic recovery routing axis for Node/docs/data/research binding-order
/// failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingFailureCheck {
    /// Coding / Node: a test deliverable exists but no test runner manifest
    /// binds it.
    RunnerManifest,
    /// Docs: a document exists but the content check cannot bind to a target
    /// document / section.
    DocumentSection,
    /// Data: an output exists but the schema check cannot bind to an output
    /// file.
    SchemaOutput,
    /// Research: notes exist but the citation check cannot bind to source
    /// notes.
    SourceCitation,
}

impl BindingFailureCheck {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingFailureCheck::RunnerManifest => "runner_manifest",
            BindingFailureCheck::DocumentSection => "document_section",
            BindingFailureCheck::SchemaOutput => "schema_output",
            BindingFailureCheck::SourceCitation => "source_citation",
        }
    }

    pub(super) fn recovery(self) -> BindingRecovery {
        match self {
            BindingFailureCheck::RunnerManifest => BindingRecovery::MaterializeRunnerManifest,
            BindingFailureCheck::DocumentSection => BindingRecovery::RecoverDocumentSection,
            BindingFailureCheck::SchemaOutput => BindingRecovery::RecoverSchemaOutput,
            BindingFailureCheck::SourceCitation => BindingRecovery::RecoverSourceCitation,
        }
    }

    pub(super) fn for_task_kind(task_kind: TaskKind) -> Option<Self> {
        match task_kind {
            TaskKind::Coding => Some(BindingFailureCheck::RunnerManifest),
            TaskKind::Docs => Some(BindingFailureCheck::DocumentSection),
            TaskKind::Data => Some(BindingFailureCheck::SchemaOutput),
            TaskKind::Research => Some(BindingFailureCheck::SourceCitation),
            TaskKind::Ops | TaskKind::Authoring => None,
        }
    }

    pub(super) fn for_evidence_runner_kind(kind: EvidenceRunnerKind) -> Option<Self> {
        match kind {
            EvidenceRunnerKind::CodingBuildTest => Some(BindingFailureCheck::RunnerManifest),
            EvidenceRunnerKind::DocsContentCheck => Some(BindingFailureCheck::DocumentSection),
            EvidenceRunnerKind::DataSchemaCheck => Some(BindingFailureCheck::SchemaOutput),
            EvidenceRunnerKind::ResearchSourceFetch => Some(BindingFailureCheck::SourceCitation),
            EvidenceRunnerKind::OpsCommandObservation
            | EvidenceRunnerKind::AuthoringContentCheck => None,
        }
    }
}

/// The recovery that re-binds a deliverable to its evidence runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingRecovery {
    MaterializeRunnerManifest,
    RecoverDocumentSection,
    RecoverSchemaOutput,
    RecoverSourceCitation,
}

impl BindingRecovery {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingRecovery::MaterializeRunnerManifest => "materialize_runner_manifest",
            BindingRecovery::RecoverDocumentSection => "recover_document_section",
            BindingRecovery::RecoverSchemaOutput => "recover_schema_output",
            BindingRecovery::RecoverSourceCitation => "recover_source_citation",
        }
    }
}

/// A generic binding failure: a deliverable exists but its evidence runner
/// cannot be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EvidenceBindingFailedJob {
    pub(super) check: BindingFailureCheck,
    pub(super) recovery: BindingRecovery,
}

impl EvidenceBindingFailedJob {
    fn new(check: BindingFailureCheck) -> Self {
        Self {
            check,
            recovery: check.recovery(),
        }
    }

    pub(super) fn generic_terminal_state(self) -> GenericTerminalState {
        GenericTerminalState::EvidenceBindingFailed
    }

    pub(super) fn recovery_job_kind(self) -> RecoveryJobKind {
        RecoveryJobKind::EvidenceBindingFailedJob
    }
}

/// Result of a generic binding-order check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingState {
    Bound,
    Failed(EvidenceBindingFailedJob),
}

impl BindingState {
    pub(super) fn failed_job(self) -> Option<EvidenceBindingFailedJob> {
        match self {
            BindingState::Bound => None,
            BindingState::Failed(job) => Some(job),
        }
    }
}

pub(super) fn evaluate_binding(
    check: BindingFailureCheck,
    deliverable_present: bool,
    runner_bindable: bool,
) -> BindingState {
    if deliverable_present && !runner_bindable {
        BindingState::Failed(EvidenceBindingFailedJob::new(check))
    } else {
        BindingState::Bound
    }
}

/// One binding check: the reference the evidence runner depends on, the declared
/// identities it could bind to, and whether it resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BindingCheck {
    pub(super) kind: BindingCheckKind,
    pub(super) status: EvidenceBindingStatus,
    /// The symbol/handle/identity the test referenced (or the descriptor key
    /// that was expected for `ManifestIdentity`).
    pub(super) reference: String,
    /// The declared identities available to bind to (empty when none exist).
    pub(super) candidates: Vec<String>,
}

impl BindingCheck {
    fn bound(kind: BindingCheckKind, reference: String, candidates: Vec<String>) -> Self {
        Self {
            kind,
            status: EvidenceBindingStatus::Bound,
            reference,
            candidates,
        }
    }

    fn unbound(kind: BindingCheckKind, reference: String, candidates: Vec<String>) -> Self {
        Self {
            kind,
            status: EvidenceBindingStatus::Unbound,
            reference,
            candidates,
        }
    }

    fn resolved(
        kind: BindingCheckKind,
        bound: bool,
        reference: String,
        candidates: Vec<String>,
    ) -> Self {
        if bound {
            Self::bound(kind, reference, candidates)
        } else {
            Self::unbound(kind, reference, candidates)
        }
    }
}

/// The structured binding plan for a deliverable's evidence runner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct EvidenceBindingPlan {
    pub(super) checks: Vec<BindingCheck>,
}

impl EvidenceBindingPlan {
    /// Overall status: `Unbound` if any check is `Unbound`; `Indeterminate` if
    /// there are no decisive checks; otherwise `Bound`.
    pub(super) fn status(&self) -> EvidenceBindingStatus {
        if self
            .checks
            .iter()
            .any(|check| check.status == EvidenceBindingStatus::Unbound)
        {
            return EvidenceBindingStatus::Unbound;
        }
        if self
            .checks
            .iter()
            .any(|check| check.status == EvidenceBindingStatus::Bound)
        {
            return EvidenceBindingStatus::Bound;
        }
        EvidenceBindingStatus::Indeterminate
    }

    pub(super) fn is_bound(&self) -> bool {
        self.status() == EvidenceBindingStatus::Bound
    }

    /// The checks that failed to bind — the structured observation surface a
    /// caller logs / routes to recovery.
    pub(super) fn failed_checks(&self) -> impl Iterator<Item = &BindingCheck> {
        self.checks
            .iter()
            .filter(|check| check.status == EvidenceBindingStatus::Unbound)
    }

    /// Route a binding gap into the existing evidence-runner error vocabulary
    /// (`BindingFailed`). `Bound` / `Indeterminate` return `None`.
    pub(super) fn binding_error(&self) -> Option<EvidenceRunnerError> {
        (self.status() == EvidenceBindingStatus::Unbound)
            .then_some(EvidenceRunnerError::BindingFailed)
    }

    /// Project a binding gap onto the generic terminal state
    /// (`EvidenceBindingFailed`). Keeps the legacy terminal projection intact —
    /// no new terminal label is introduced.
    pub(super) fn generic_terminal_state(&self) -> Option<GenericTerminalState> {
        self.binding_error()
            .map(EvidenceRunnerError::generic_terminal_state)
    }

    /// Route a binding gap to the dedicated recovery job kind
    /// (`EvidenceBindingFailedJob`), keeping it distinct from a bound runner that
    /// failed (`EvidenceFailedJob`). `Bound` / `Indeterminate` return `None`.
    ///
    /// This is runtime-neutral: every adapter (Rust / Node / docs / data /
    /// research) routes through the same single job kind, so no new job type is
    /// introduced. The structured per-check detail (`failed_checks`) selects the
    /// concrete repair operator; this projection is only the job-kind routing.
    pub(super) fn recovery_job_kind(&self) -> Option<RecoveryJobKind> {
        (self.status() == EvidenceBindingStatus::Unbound)
            .then_some(RecoveryJobKind::EvidenceBindingFailedJob)
    }
}

// ---------------------------------------------------------------------------
// Rust adapter
// ---------------------------------------------------------------------------

/// Cargo manifest identities relevant to test binding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CargoBindingIdentities {
    pub(super) package_name: Option<String>,
    pub(super) lib_name: Option<String>,
    pub(super) bin_names: BTreeSet<String>,
    /// Normalized dependency import names (`[dependencies]` /
    /// `[dev-dependencies]` / `[build-dependencies]` keys). The key is the
    /// import name even under a `package = "…"` rename, so this is exactly the
    /// set to exclude from self-import detection.
    pub(super) dependencies: BTreeSet<String>,
}

impl CargoBindingIdentities {
    /// The crate names importable as `use <name>::…`. With an explicit
    /// `[lib] name` that is the only importable identity; otherwise the default
    /// lib name is the (normalized) package name.
    fn lib_identities(&self) -> BTreeSet<String> {
        let mut identities = BTreeSet::new();
        if let Some(lib) = &self.lib_name {
            identities.insert(normalize_crate_ident(lib));
        } else if let Some(pkg) = &self.package_name {
            identities.insert(normalize_crate_ident(pkg));
        }
        identities
    }

    /// The bin target names a `CARGO_BIN_EXE_*` handle can bind to: declared
    /// `[[bin]] name`s plus the default bin (the package name). Including the
    /// package name keeps lib-only crates conservative (false-negative, never a
    /// false-positive binding failure).
    fn bin_identities(&self) -> BTreeSet<String> {
        let mut identities: BTreeSet<String> = self
            .bin_names
            .iter()
            .map(|n| normalize_crate_ident(n))
            .collect();
        if let Some(pkg) = &self.package_name {
            identities.insert(normalize_crate_ident(pkg));
        }
        identities
    }
}

/// References a Rust integration test makes to the crate under test.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RustTestReferences {
    /// First path segments of `use …` / `extern crate …` statements.
    pub(super) crate_imports: BTreeSet<String>,
    /// `CARGO_BIN_EXE_*` / `cargo_bin("…")` binary handles.
    pub(super) binary_handles: BTreeSet<String>,
}

/// Normalize a crate/bin identifier the way cargo does for `use` idents and
/// `CARGO_BIN_EXE_*` env var names: `-` becomes `_`. Case is preserved.
fn normalize_crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

const BUILTIN_CRATES: &[&str] = &["std", "core", "alloc", "proc_macro", "test"];

/// Imports that never refer to an external crate and so must be excluded from
/// self-import detection.
fn is_self_or_builtin_import(name: &str) -> bool {
    matches!(name, "crate" | "self" | "super" | "Self") || BUILTIN_CRATES.contains(&name)
}

/// First path segment of a `use`/`extern crate` tail (e.g. `foo` from
/// `foo::bar::{…}` or `::foo as bar`). Rust crate idents are `[A-Za-z0-9_]`.
fn first_path_segment(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("::").unwrap_or(rest);
    let segment: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (!segment.is_empty()).then_some(segment)
}

/// Extract the crate imports and binary handles from a single Rust test source.
pub(super) fn extract_rust_test_references(source: &str) -> RustTestReferences {
    let mut crate_imports = BTreeSet::new();
    for raw_line in source.lines() {
        let line = strip_toml_comment_free(raw_line.trim());
        let line = line.strip_prefix("pub ").unwrap_or(line);
        let import_tail = line
            .strip_prefix("use ")
            .or_else(|| line.strip_prefix("extern crate "));
        if let Some(rest) = import_tail
            && let Some(segment) = first_path_segment(rest)
        {
            crate_imports.insert(segment);
        }
    }
    RustTestReferences {
        crate_imports,
        binary_handles: referenced_binary_handles(source),
    }
}

/// Drop a trailing `// …` line comment so `use foo; // note` parses cleanly.
/// (A `use`/`extern crate` line never contains a `//` before the statement, so
/// this only ever trims trailing comments.)
fn strip_toml_comment_free(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => line[..idx].trim_end(),
        None => line,
    }
}

fn referenced_binary_handles(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for tail in source.split("CARGO_BIN_EXE_").skip(1) {
        let name: String = tail
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    for marker in ["cargo_bin(\"", "cargo_bin!(\""] {
        let mut rest = source;
        while let Some(idx) = rest.find(marker) {
            let after = &rest[idx + marker.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            let name = &after[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            {
                names.insert(name.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    names
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestSection {
    Package,
    Lib,
    Bin,
    Dependencies,
    Other,
}

/// Parse the binding-relevant identities from a `Cargo.toml` source string.
///
/// Reuses the shared `strip_toml_comment` / `parse_toml_string_value` helpers
/// (no `toml` crate dependency, no ad hoc string mutation) and only tracks the
/// `[package]` / `[lib]` / `[[bin]]` / dependency sections.
pub(super) fn parse_cargo_binding_identities(manifest_source: &str) -> CargoBindingIdentities {
    let mut identities = CargoBindingIdentities::default();
    let mut section = ManifestSection::Other;
    for raw_line in manifest_source.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(inner) = line.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
            section = if inner.trim() == "bin" {
                ManifestSection::Bin
            } else {
                ManifestSection::Other
            };
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = classify_table_header(inner.trim(), &mut identities);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        match section {
            ManifestSection::Package if key == "name" => {
                identities.package_name =
                    parse_toml_string_value(value.trim()).filter(|s| !s.is_empty());
            }
            ManifestSection::Lib if key == "name" => {
                identities.lib_name =
                    parse_toml_string_value(value.trim()).filter(|s| !s.is_empty());
            }
            ManifestSection::Bin if key == "name" => {
                if let Some(name) = parse_toml_string_value(value.trim()).filter(|s| !s.is_empty())
                {
                    identities.bin_names.insert(name);
                }
            }
            ManifestSection::Dependencies if !key.is_empty() => {
                identities
                    .dependencies
                    .insert(normalize_crate_ident(strip_quotes(key)));
            }
            _ => {}
        }
    }
    identities
}

/// Classify a single-bracket table header and record `[<table>.<dep>]`
/// sub-table dependency keys as a side effect.
fn classify_table_header(header: &str, identities: &mut CargoBindingIdentities) -> ManifestSection {
    match header {
        "package" => ManifestSection::Package,
        "lib" => ManifestSection::Lib,
        "dependencies" | "dev-dependencies" | "build-dependencies" => ManifestSection::Dependencies,
        other => {
            if let Some(dep) = dependency_subtable_key(other) {
                identities
                    .dependencies
                    .insert(normalize_crate_ident(strip_quotes(dep)));
                ManifestSection::Other
            } else if other.ends_with(".dependencies") {
                // e.g. `target.'cfg(unix)'.dependencies`
                ManifestSection::Dependencies
            } else {
                ManifestSection::Other
            }
        }
    }
}

/// For a `[dependencies.foo]` / `[dev-dependencies.foo]` / `[build-dependencies.foo]`
/// sub-table header, return the dependency key (`foo`).
fn dependency_subtable_key(header: &str) -> Option<&str> {
    for prefix in ["dependencies.", "dev-dependencies.", "build-dependencies."] {
        if let Some(rest) = header.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn strip_quotes(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
}

/// Build the Rust evidence binding plan from a manifest source and a set of test
/// sources. Pure (no filesystem) so focused tests can drive it directly.
///
/// - No test references at all → an empty (`Indeterminate`) plan, so non-Rust
///   evidence paths (Python/Node/docs) are never affected.
/// - Missing manifest with Rust references → a single `ManifestIdentity`
///   `Unbound` check (the runner cannot bind without a manifest).
pub(super) fn rust_evidence_binding_plan(
    manifest_source: Option<&str>,
    test_sources: &[&str],
) -> EvidenceBindingPlan {
    let mut imports = BTreeSet::new();
    let mut handles = BTreeSet::new();
    for source in test_sources {
        let references = extract_rust_test_references(source);
        imports.extend(references.crate_imports);
        handles.extend(references.binary_handles);
    }

    let mut plan = EvidenceBindingPlan::default();
    if imports.is_empty() && handles.is_empty() {
        return plan; // Indeterminate: nothing to bind.
    }

    let identities = match manifest_source {
        Some(source) => parse_cargo_binding_identities(source),
        None => {
            plan.checks.push(BindingCheck::unbound(
                BindingCheckKind::ManifestIdentity,
                "Cargo.toml".to_string(),
                Vec::new(),
            ));
            return plan;
        }
    };

    // ManifestIdentity: is there a buildable package identity at all?
    match &identities.package_name {
        Some(name) => plan.checks.push(BindingCheck::bound(
            BindingCheckKind::ManifestIdentity,
            name.clone(),
            vec![name.clone()],
        )),
        None => plan.checks.push(BindingCheck::unbound(
            BindingCheckKind::ManifestIdentity,
            "package.name".to_string(),
            Vec::new(),
        )),
    }

    // ImportSymbol: crate-under-test imports must resolve to the lib identity.
    let lib_identities = identities.lib_identities();
    let lib_candidates: Vec<String> = lib_identities.iter().cloned().collect();
    for import in &imports {
        if is_self_or_builtin_import(import) {
            continue;
        }
        if identities
            .dependencies
            .contains(&normalize_crate_ident(import))
        {
            continue; // declared dependency, not a crate-under-test reference
        }
        let bound = lib_identities.contains(&normalize_crate_ident(import));
        plan.checks.push(BindingCheck::resolved(
            BindingCheckKind::ImportSymbol,
            bound,
            import.clone(),
            lib_candidates.clone(),
        ));
    }

    // ExecutableHandle: CARGO_BIN_EXE_* handles must resolve to a bin identity.
    let bin_identities = identities.bin_identities();
    let bin_candidates: Vec<String> = bin_identities.iter().cloned().collect();
    for handle in &handles {
        let bound = bin_identities.contains(&normalize_crate_ident(handle));
        plan.checks.push(BindingCheck::resolved(
            BindingCheckKind::ExecutableHandle,
            bound,
            handle.clone(),
            bin_candidates.clone(),
        ));
    }

    plan
}

/// Per-file read cap so a pathological workspace cannot make the binding scan
/// allocate unbounded memory.
const MAX_BINDING_SOURCE_BYTES: u64 = 1024 * 1024;

fn read_capped(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_BINDING_SOURCE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Build the Rust evidence binding plan by reading `Cargo.toml` and the direct
/// `tests/*.rs` children under `work_root`. Read-only; future production wiring
/// (#988) calls this before invoking the coding evidence runner.
pub(super) fn rust_evidence_binding_plan_from_work_root(work_root: &Path) -> EvidenceBindingPlan {
    let manifest = read_capped(&work_root.join("Cargo.toml"));

    let mut test_sources: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(work_root.join("tests")) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs")
            })
            .collect();
        paths.sort();
        for path in &paths {
            if let Some(source) = read_capped(path) {
                test_sources.push(source);
            }
        }
    }

    let references: Vec<&str> = test_sources.iter().map(String::as_str).collect();
    rust_evidence_binding_plan(manifest.as_deref(), &references)
}

// ---------------------------------------------------------------------------
// Node adapter
// ---------------------------------------------------------------------------

/// Build the Node evidence binding plan from a `package.json` source and whether
/// a Node test deliverable exists. Pure (no filesystem) so focused tests drive
/// it directly.
///
/// - No Node test file → an empty (`Indeterminate`) plan, so non-Node evidence
///   paths (Rust/Python/docs) are never affected.
/// - Test file present + missing `package.json` → a `ManifestIdentity` `Unbound`
///   check (the runner cannot bind without a manifest).
/// - Test file present + manifest without a usable `scripts.test` → a
///   `TestScript` `Unbound` check. Bindability reuses
///   [`complete_node_test_runner_manifest`] so the plan and the deterministic
///   recovery operator agree (a malformed manifest is conservatively treated as
///   bound — a false negative, never a false positive).
pub(super) fn node_evidence_binding_plan(
    manifest_source: Option<&str>,
    test_file_present: bool,
) -> EvidenceBindingPlan {
    let mut plan = EvidenceBindingPlan::default();
    if !test_file_present {
        return plan; // Indeterminate: no test deliverable to bind.
    }
    match manifest_source {
        None => plan.checks.push(BindingCheck::unbound(
            BindingCheckKind::ManifestIdentity,
            "package.json".to_string(),
            Vec::new(),
        )),
        Some(source) => {
            plan.checks.push(BindingCheck::bound(
                BindingCheckKind::ManifestIdentity,
                "package.json".to_string(),
                vec!["package.json".to_string()],
            ));
            // A `Some(completion)` means the runner cannot bind a usable
            // `scripts.test` yet; `None` means it is already bindable (or the
            // manifest is malformed and must not be clobbered → bound,
            // conservatively).
            let bound = complete_node_test_runner_manifest(Some(source)).is_none();
            let candidates = if bound {
                vec!["scripts.test".to_string()]
            } else {
                Vec::new()
            };
            plan.checks.push(BindingCheck::resolved(
                BindingCheckKind::TestScript,
                bound,
                "scripts.test".to_string(),
                candidates,
            ));
        }
    }
    plan
}

/// Build the Node evidence binding plan by reading `package.json` and probing for
/// a conventional Node test file under `work_root`. Read-only. Reuses
/// `node_request_helpers::workspace_has_node_test_file` so the Node test-file
/// suffix list stays a single source of truth.
pub(super) fn node_evidence_binding_plan_from_work_root(work_root: &Path) -> EvidenceBindingPlan {
    let test_file_present = super::node_request_helpers::workspace_has_node_test_file(work_root);
    let manifest = read_capped(&work_root.join("package.json"));
    node_evidence_binding_plan(manifest.as_deref(), test_file_present)
}

// ---------------------------------------------------------------------------
// Non-coding adapters (docs / data / research)
// ---------------------------------------------------------------------------

/// Generic membership binding plan shared by the docs/data/research adapters:
/// each `reference` must resolve to one of `candidates`, expressing the binding
/// through the same `BindingCheck` vocabulary as the Rust/Node adapters.
///
/// - No references → an empty (`Indeterminate`) plan (nothing to bind).
/// - Deliverable absent → a single `FileLayout` `Unbound` check (the document /
///   output / notes the runner binds to does not exist).
/// - Otherwise each reference resolves iff present in the candidate set.
fn membership_binding_plan(
    kind: BindingCheckKind,
    deliverable_present: bool,
    deliverable_label: &str,
    references: &[String],
    candidates: &[String],
) -> EvidenceBindingPlan {
    let mut plan = EvidenceBindingPlan::default();
    if references.is_empty() {
        return plan; // Indeterminate: nothing to bind.
    }
    if !deliverable_present {
        plan.checks.push(BindingCheck::unbound(
            BindingCheckKind::FileLayout,
            deliverable_label.to_string(),
            Vec::new(),
        ));
        return plan;
    }
    let available: BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    let candidate_list: Vec<String> = candidates.to_vec();
    for reference in references {
        let bound = available.contains(reference.as_str());
        plan.checks.push(BindingCheck::resolved(
            kind,
            bound,
            reference.clone(),
            candidate_list.clone(),
        ));
    }
    plan
}

/// Docs adapter: each required section must bind to the produced document.
pub(super) fn docs_evidence_binding_plan(
    document_present: bool,
    required_sections: &[String],
    present_sections: &[String],
) -> EvidenceBindingPlan {
    membership_binding_plan(
        BindingCheckKind::RequiredSection,
        document_present,
        "document",
        required_sections,
        present_sections,
    )
}

/// Data adapter: each declared schema column must bind to the produced output.
pub(super) fn data_evidence_binding_plan(
    output_present: bool,
    required_columns: &[String],
    observed_columns: &[String],
) -> EvidenceBindingPlan {
    membership_binding_plan(
        BindingCheckKind::SchemaColumn,
        output_present,
        "output",
        required_columns,
        observed_columns,
    )
}

/// Research adapter: each citation must bind to an available source.
pub(super) fn research_evidence_binding_plan(
    notes_present: bool,
    citations: &[String],
    available_sources: &[String],
) -> EvidenceBindingPlan {
    membership_binding_plan(
        BindingCheckKind::Citation,
        notes_present,
        "notes",
        citations,
        available_sources,
    )
}

// ---------------------------------------------------------------------------
// Controller hook
// ---------------------------------------------------------------------------

/// The controller hook the lifecycle runs *after scaffold materialization* and
/// *before* the EvidenceRunner executes: build the structured
/// `EvidenceBindingPlan` for the active task kind so a binding gap is classified
/// as a structured `evidence_binding_failed` observation
/// ([`EvidenceBindingPlan::recovery_job_kind`] → `EvidenceBindingFailedJob`)
/// instead of being rolled into a generic evidence failure or `repair_exhausted`.
///
/// The dispatch stays runtime-neutral; the Rust/Node specifics live in the
/// per-runtime adapters so they never leak into the lifecycle center (Issue #1004
/// maintainability requirement).
///
/// - `Coding` → the union of the Rust (cargo/test) and Node (manifest/test-script)
///   work-root plans. A coding workspace is usually one runtime or the other; the
///   union is `Unbound` if either runtime is unbound and `Indeterminate` when
///   neither declares references (so Python-only / non-coding workspaces are
///   never flagged).
/// - `Docs | Data | Research | Ops | Authoring` → `Indeterminate`. Their binding
///   is driven by contract evidence (required sections / schema columns /
///   citations) via [`docs_evidence_binding_plan`] / [`data_evidence_binding_plan`]
///   / [`research_evidence_binding_plan`], not by a post-scaffold workspace scan.
pub(super) fn evidence_binding_plan_after_scaffold(
    task_kind: TaskKind,
    work_root: &Path,
) -> EvidenceBindingPlan {
    match task_kind {
        TaskKind::Coding => {
            let mut plan = rust_evidence_binding_plan_from_work_root(work_root);
            let node = node_evidence_binding_plan_from_work_root(work_root);
            plan.checks.extend(node.checks);
            plan
        }
        TaskKind::Docs
        | TaskKind::Data
        | TaskKind::Research
        | TaskKind::Ops
        | TaskKind::Authoring => EvidenceBindingPlan::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("evidence_binding")
            .join("rust")
            .join(name)
    }

    #[test]
    fn label_strings_are_stable() {
        assert_eq!(EvidenceBindingStatus::Bound.as_str(), "bound");
        assert_eq!(EvidenceBindingStatus::Unbound.as_str(), "unbound");
        assert_eq!(
            EvidenceBindingStatus::Indeterminate.as_str(),
            "indeterminate"
        );
        assert_eq!(
            BindingCheckKind::ManifestIdentity.as_str(),
            "manifest_identity"
        );
        assert_eq!(BindingCheckKind::ImportSymbol.as_str(), "import_symbol");
        assert_eq!(
            BindingCheckKind::ExecutableHandle.as_str(),
            "executable_handle"
        );
        // Issue #1004: generic vocabulary additions (labels must stay stable).
        assert_eq!(BindingCheckKind::TestScript.as_str(), "test_script");
        assert_eq!(
            BindingCheckKind::RequiredSection.as_str(),
            "required_section"
        );
        assert_eq!(BindingCheckKind::SchemaColumn.as_str(), "schema_column");
        assert_eq!(BindingCheckKind::Citation.as_str(), "citation");
        assert_eq!(BindingCheckKind::FileLayout.as_str(), "file_layout");
    }

    #[test]
    fn parses_package_lib_and_bin_names() {
        let manifest = "\
[package]
name = \"ndjson-tool\"
version = \"0.1.0\"

[lib]
name = \"nd_json\"

[[bin]]
name = \"ndjson\"

[dev-dependencies]
tempfile = \"3\"
serde = { version = \"1\", features = [\"derive\"] }

[dependencies.regex]
version = \"1\"
";
        let identities = parse_cargo_binding_identities(manifest);
        assert_eq!(identities.package_name.as_deref(), Some("ndjson-tool"));
        assert_eq!(identities.lib_name.as_deref(), Some("nd_json"));
        assert!(identities.bin_names.contains("ndjson"));
        assert!(identities.dependencies.contains("tempfile"));
        assert!(identities.dependencies.contains("serde"));
        assert!(identities.dependencies.contains("regex"));
        // `[lib] name` overrides the package name as the importable identity.
        assert_eq!(
            identities.lib_identities().into_iter().collect::<Vec<_>>(),
            vec!["nd_json".to_string()]
        );
    }

    #[test]
    fn extracts_imports_and_binary_handles() {
        let source = "\
use std::process::Command;
use slug::slugify;
pub use tempfile::TempDir;
extern crate libc;

#[test]
fn runs() {
    let bin = env!(\"CARGO_BIN_EXE_word_counter\");
    let other = Command::cargo_bin(\"slug-cli\").unwrap();
}
";
        let references = extract_rust_test_references(source);
        assert!(references.crate_imports.contains("std"));
        assert!(references.crate_imports.contains("slug"));
        assert!(references.crate_imports.contains("tempfile"));
        assert!(references.crate_imports.contains("libc"));
        assert!(references.binary_handles.contains("word_counter"));
        assert!(references.binary_handles.contains("slug-cli"));
    }

    // --- Representative v0.6.4 failures (slug / word / ndjson) -------------

    #[test]
    fn slug_crate_import_mismatch_is_binding_failure() {
        // package is `slugify`; the test imports `slug` -> unresolved self-import.
        let plan = rust_evidence_binding_plan_from_work_root(&fixture_root("slug"));
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.binding_error(),
            Some(EvidenceRunnerError::BindingFailed)
        );
        assert_eq!(
            plan.generic_terminal_state(),
            Some(GenericTerminalState::EvidenceBindingFailed)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::ImportSymbol)
            .expect("import symbol binding failure");
        assert_eq!(failure.reference, "slug");
        assert_eq!(failure.candidates, vec!["slugify".to_string()]);
    }

    #[test]
    fn word_executable_handle_mismatch_is_binding_failure() {
        // package/bin is `wordcount`; the test references CARGO_BIN_EXE_word_counter.
        let plan = rust_evidence_binding_plan_from_work_root(&fixture_root("word"));
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.generic_terminal_state(),
            Some(GenericTerminalState::EvidenceBindingFailed)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::ExecutableHandle)
            .expect("executable handle binding failure");
        assert_eq!(failure.reference, "word_counter");
        assert!(failure.candidates.contains(&"wordcount".to_string()));
    }

    #[test]
    fn ndjson_lib_name_mismatch_is_binding_failure() {
        // `[lib] name = "nd_json"`; the test imports `ndjson`.
        let plan = rust_evidence_binding_plan_from_work_root(&fixture_root("ndjson"));
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.binding_error(),
            Some(EvidenceRunnerError::BindingFailed)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::ImportSymbol)
            .expect("import symbol binding failure");
        assert_eq!(failure.reference, "ndjson");
        assert_eq!(failure.candidates, vec!["nd_json".to_string()]);
    }

    #[test]
    fn correctly_bound_rust_project_has_no_false_positive() {
        // Correct self-import + matching bin handle + a declared dev-dependency
        // import (which must NOT be flagged).
        let plan = rust_evidence_binding_plan_from_work_root(&fixture_root("bound"));
        assert_eq!(plan.status(), EvidenceBindingStatus::Bound);
        assert!(plan.is_bound());
        assert_eq!(plan.binding_error(), None);
        assert_eq!(plan.generic_terminal_state(), None);
        assert!(plan.failed_checks().next().is_none());
    }

    // --- Boundary cases (no false positives on non-coding / dep-only) -----

    #[test]
    fn importing_only_a_declared_dependency_is_not_flagged() {
        let manifest = "\
[package]
name = \"app\"

[dev-dependencies]
tempfile = \"3\"
";
        let test = "use tempfile::TempDir;\n#[test] fn t() { let _ = TempDir::new(); }\n";
        let plan = rust_evidence_binding_plan(Some(manifest), &[test]);
        // ManifestIdentity is bound; the dependency import yields no check.
        assert_eq!(plan.status(), EvidenceBindingStatus::Bound);
        assert!(
            plan.checks
                .iter()
                .all(|check| check.kind != BindingCheckKind::ImportSymbol)
        );
        assert_eq!(plan.binding_error(), None);
    }

    #[test]
    fn no_rust_references_yields_indeterminate_plan() {
        // Mimics a Python/Node/docs evidence path: no Rust references at all.
        let manifest = "[package]\nname = \"app\"\n";
        let plan = rust_evidence_binding_plan(Some(manifest), &["print('hello')\n"]);
        assert!(plan.checks.is_empty());
        assert_eq!(plan.status(), EvidenceBindingStatus::Indeterminate);
        assert_eq!(plan.binding_error(), None);
        assert_eq!(plan.generic_terminal_state(), None);
    }

    #[test]
    fn missing_manifest_with_rust_references_is_manifest_binding_failure() {
        let test = "use slug::slugify;\n#[test] fn t() {}\n";
        let plan = rust_evidence_binding_plan(None, &[test]);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        let failure = plan
            .failed_checks()
            .next()
            .expect("manifest binding failure");
        assert_eq!(failure.kind, BindingCheckKind::ManifestIdentity);
        assert_eq!(failure.reference, "Cargo.toml");
    }

    #[test]
    fn hyphen_underscore_normalization_binds() {
        // package `word-counter`; bin handle CARGO_BIN_EXE_word_counter binds via
        // `-`/`_` normalization, and `use word_counter::` binds to the lib.
        let manifest = "[package]\nname = \"word-counter\"\n";
        let test = "\
use word_counter::count;
#[test]
fn t() {
    let bin = env!(\"CARGO_BIN_EXE_word_counter\");
    let _ = bin;
}
";
        let plan = rust_evidence_binding_plan(Some(manifest), &[test]);
        assert_eq!(plan.status(), EvidenceBindingStatus::Bound);
        assert_eq!(plan.binding_error(), None);
    }
}

#[cfg(test)]
mod generic_binding_failure_tests {
    use super::*;

    const ALL_FAILURE_CHECKS: [BindingFailureCheck; 4] = [
        BindingFailureCheck::RunnerManifest,
        BindingFailureCheck::DocumentSection,
        BindingFailureCheck::SchemaOutput,
        BindingFailureCheck::SourceCitation,
    ];

    #[test]
    fn deliverable_present_but_unbound_is_a_binding_failure() {
        for check in ALL_FAILURE_CHECKS {
            let state = evaluate_binding(check, true, false);
            let job = state
                .failed_job()
                .unwrap_or_else(|| panic!("{} should be a binding failure", check.as_str()));
            assert_eq!(job.check, check);
            assert_eq!(job.recovery, check.recovery());
            assert_eq!(
                job.generic_terminal_state(),
                GenericTerminalState::EvidenceBindingFailed
            );
            assert_eq!(
                job.recovery_job_kind(),
                RecoveryJobKind::EvidenceBindingFailedJob
            );
        }
    }

    #[test]
    fn bound_or_absent_deliverable_is_not_a_binding_failure() {
        for check in ALL_FAILURE_CHECKS {
            assert_eq!(evaluate_binding(check, true, true), BindingState::Bound);
            assert_eq!(evaluate_binding(check, false, false), BindingState::Bound);
            assert_eq!(evaluate_binding(check, false, true), BindingState::Bound);
        }
    }

    #[test]
    fn binding_failure_check_for_task_kind_mirrors_evidence_runner_selection() {
        assert_eq!(
            BindingFailureCheck::for_task_kind(TaskKind::Coding),
            Some(BindingFailureCheck::RunnerManifest)
        );
        assert_eq!(
            BindingFailureCheck::for_task_kind(TaskKind::Docs),
            Some(BindingFailureCheck::DocumentSection)
        );
        assert_eq!(
            BindingFailureCheck::for_task_kind(TaskKind::Data),
            Some(BindingFailureCheck::SchemaOutput)
        );
        assert_eq!(
            BindingFailureCheck::for_task_kind(TaskKind::Research),
            Some(BindingFailureCheck::SourceCitation)
        );
        assert_eq!(BindingFailureCheck::for_task_kind(TaskKind::Ops), None);
        assert_eq!(
            BindingFailureCheck::for_task_kind(TaskKind::Authoring),
            None
        );
    }

    #[test]
    fn binding_failure_check_for_evidence_runner_kind_matches_task_kind_mapping() {
        let cases = [
            (
                EvidenceRunnerKind::CodingBuildTest,
                Some(BindingFailureCheck::RunnerManifest),
            ),
            (
                EvidenceRunnerKind::DocsContentCheck,
                Some(BindingFailureCheck::DocumentSection),
            ),
            (
                EvidenceRunnerKind::DataSchemaCheck,
                Some(BindingFailureCheck::SchemaOutput),
            ),
            (
                EvidenceRunnerKind::ResearchSourceFetch,
                Some(BindingFailureCheck::SourceCitation),
            ),
            (EvidenceRunnerKind::OpsCommandObservation, None),
            (EvidenceRunnerKind::AuthoringContentCheck, None),
        ];
        for (runner_kind, expected) in cases {
            assert_eq!(
                BindingFailureCheck::for_evidence_runner_kind(runner_kind),
                expected
            );
        }
    }

    #[test]
    fn binding_recovery_labels_are_stable() {
        assert_eq!(
            BindingFailureCheck::RunnerManifest.as_str(),
            "runner_manifest"
        );
        assert_eq!(
            BindingFailureCheck::DocumentSection.as_str(),
            "document_section"
        );
        assert_eq!(BindingFailureCheck::SchemaOutput.as_str(), "schema_output");
        assert_eq!(
            BindingFailureCheck::SourceCitation.as_str(),
            "source_citation"
        );
        assert_eq!(
            BindingRecovery::MaterializeRunnerManifest.as_str(),
            "materialize_runner_manifest"
        );
        assert_eq!(
            BindingRecovery::RecoverDocumentSection.as_str(),
            "recover_document_section"
        );
        assert_eq!(
            BindingRecovery::RecoverSchemaOutput.as_str(),
            "recover_schema_output"
        );
        assert_eq!(
            BindingRecovery::RecoverSourceCitation.as_str(),
            "recover_source_citation"
        );
    }
}

/// Issue #1004: the controller hook that validates the `EvidenceBindingPlan`
/// after scaffold materialization, plus the Node / docs / data / research
/// adapters that express their bindings through the same `EvidenceBindingPlan`
/// abstraction.
#[cfg(test)]
mod after_scaffold_tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    // --- Node adapter (AC1) -------------------------------------------------

    #[test]
    fn node_test_without_manifest_is_a_binding_failure() {
        // A Node test deliverable exists but `package.json` is missing — a
        // binding-order failure routed to `EvidenceBindingFailedJob`, not a
        // missing/failed-evidence terminal.
        let plan = node_evidence_binding_plan(None, true);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        assert_eq!(
            plan.generic_terminal_state(),
            Some(GenericTerminalState::EvidenceBindingFailed)
        );
        let failure = plan
            .failed_checks()
            .next()
            .expect("manifest binding failure");
        assert_eq!(failure.kind, BindingCheckKind::ManifestIdentity);
        assert_eq!(failure.reference, "package.json");
    }

    #[test]
    fn node_manifest_without_test_script_is_a_test_script_binding_failure() {
        let manifest = r#"{"name":"app","version":"1.0.0","type":"module"}"#;
        let plan = node_evidence_binding_plan(Some(manifest), true);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::TestScript)
            .expect("test script binding failure");
        assert_eq!(failure.reference, "scripts.test");
        // The manifest itself binds; only the runnable test command does not.
        assert!(
            plan.checks
                .iter()
                .any(|check| check.kind == BindingCheckKind::ManifestIdentity
                    && check.status == EvidenceBindingStatus::Bound)
        );
    }

    #[test]
    fn node_manifest_with_bound_test_script_is_not_a_binding_failure() {
        let manifest = r#"{"name":"app","scripts":{"test":"node --test"}}"#;
        let plan = node_evidence_binding_plan(Some(manifest), true);
        assert_eq!(plan.status(), EvidenceBindingStatus::Bound);
        assert!(plan.is_bound());
        assert_eq!(plan.recovery_job_kind(), None);
        assert!(plan.failed_checks().next().is_none());
    }

    #[test]
    fn node_without_test_file_is_indeterminate() {
        // No Node test deliverable -> nothing to bind, so the plan never routes
        // a failure (a Rust/Python workspace is unaffected).
        let plan = node_evidence_binding_plan(None, false);
        assert!(plan.checks.is_empty());
        assert_eq!(plan.status(), EvidenceBindingStatus::Indeterminate);
        assert_eq!(plan.recovery_job_kind(), None);
    }

    #[test]
    fn node_malformed_manifest_is_conservatively_bound() {
        // A malformed manifest must not be clobbered; the binding scan treats it
        // as bound (false negative, never a false positive).
        let plan = node_evidence_binding_plan(Some("{not json"), true);
        assert_eq!(plan.status(), EvidenceBindingStatus::Bound);
        assert_eq!(plan.recovery_job_kind(), None);
    }

    // --- Controller hook (AC1 / AC2 end-to-end) -----------------------------

    #[test]
    fn after_scaffold_node_missing_manifest_routes_to_binding_failed_job() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(
            root.join("tests").join("main.test.js"),
            "import { test } from 'node:test';\ntest('ok', () => {});\n",
        )
        .unwrap();

        let plan = evidence_binding_plan_after_scaffold(TaskKind::Coding, root);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        assert!(
            plan.failed_checks()
                .any(|check| check.kind == BindingCheckKind::ManifestIdentity)
        );
    }

    #[test]
    fn after_scaffold_rust_crate_mismatch_routes_to_binding_failed_job() {
        // AC2: a Rust integration test crate/bin/lib mismatch surfaces through
        // the controller hook as a binding failure.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("evidence_binding")
            .join("rust")
            .join("slug");
        let plan = evidence_binding_plan_after_scaffold(TaskKind::Coding, &root);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        assert!(
            plan.failed_checks()
                .any(|check| check.kind == BindingCheckKind::ImportSymbol)
        );
    }

    #[test]
    fn after_scaffold_empty_workspace_is_indeterminate() {
        let temp = tempfile::tempdir().unwrap();
        let plan = evidence_binding_plan_after_scaffold(TaskKind::Coding, temp.path());
        assert_eq!(plan.status(), EvidenceBindingStatus::Indeterminate);
        assert_eq!(plan.recovery_job_kind(), None);
    }

    #[test]
    fn after_scaffold_non_coding_kinds_are_indeterminate() {
        let temp = tempfile::tempdir().unwrap();
        for kind in [
            TaskKind::Docs,
            TaskKind::Data,
            TaskKind::Research,
            TaskKind::Ops,
            TaskKind::Authoring,
        ] {
            let plan = evidence_binding_plan_after_scaffold(kind, temp.path());
            assert!(plan.checks.is_empty());
            assert_eq!(plan.status(), EvidenceBindingStatus::Indeterminate);
            assert_eq!(plan.recovery_job_kind(), None);
        }
    }

    // --- docs / data / research adapters (AC3) ------------------------------

    #[test]
    fn docs_missing_required_section_is_a_binding_failure() {
        let plan =
            docs_evidence_binding_plan(true, &strings(&["Setup", "Usage"]), &strings(&["Setup"]));
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::RequiredSection)
            .expect("required section binding failure");
        assert_eq!(failure.reference, "Usage");

        // All sections present -> bound.
        let bound = docs_evidence_binding_plan(
            true,
            &strings(&["Setup", "Usage"]),
            &strings(&["Setup", "Usage"]),
        );
        assert_eq!(bound.status(), EvidenceBindingStatus::Bound);
        assert_eq!(bound.recovery_job_kind(), None);
    }

    #[test]
    fn docs_missing_document_is_a_file_layout_binding_failure() {
        let plan = docs_evidence_binding_plan(false, &strings(&["Setup"]), &[]);
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        let failure = plan.failed_checks().next().expect("file layout failure");
        assert_eq!(failure.kind, BindingCheckKind::FileLayout);
        assert_eq!(failure.reference, "document");
    }

    #[test]
    fn data_schema_output_mismatch_is_a_binding_failure() {
        let plan = data_evidence_binding_plan(
            true,
            &strings(&["Category", "Total"]),
            &strings(&["Category"]),
        );
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::SchemaColumn)
            .expect("schema column binding failure");
        assert_eq!(failure.reference, "Total");
    }

    #[test]
    fn research_citation_source_mismatch_is_a_binding_failure() {
        let plan = research_evidence_binding_plan(
            true,
            &strings(&["https://example.test/a", "https://example.test/missing"]),
            &strings(&["https://example.test/a"]),
        );
        assert_eq!(plan.status(), EvidenceBindingStatus::Unbound);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        let failure = plan
            .failed_checks()
            .find(|check| check.kind == BindingCheckKind::Citation)
            .expect("citation binding failure");
        assert_eq!(failure.reference, "https://example.test/missing");
    }

    #[test]
    fn non_coding_adapters_with_no_references_are_indeterminate() {
        // Empty reference sets never route a failure (no false positives on a
        // task with no declared sections / columns / citations).
        assert_eq!(
            docs_evidence_binding_plan(true, &[], &[]).status(),
            EvidenceBindingStatus::Indeterminate
        );
        assert_eq!(
            data_evidence_binding_plan(true, &[], &[]).status(),
            EvidenceBindingStatus::Indeterminate
        );
        assert_eq!(
            research_evidence_binding_plan(true, &[], &[]).status(),
            EvidenceBindingStatus::Indeterminate
        );
    }

    // --- Legacy projection compatibility (AC4) ------------------------------

    #[test]
    fn unbound_plan_routes_to_binding_failed_not_evidence_failed() {
        // Every adapter's binding gap routes to the dedicated
        // `EvidenceBindingFailedJob`, never the generic `EvidenceFailedJob`.
        let plan = node_evidence_binding_plan(None, true);
        assert_eq!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        assert_ne!(
            plan.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceFailedJob)
        );
        assert_eq!(
            plan.binding_error(),
            Some(EvidenceRunnerError::BindingFailed)
        );
    }
}
