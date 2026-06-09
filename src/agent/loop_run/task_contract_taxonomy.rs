//! Task contract taxonomy.
//!
//! This module owns the closed vocabulary shared by objective contracts,
//! deliverable planning, evidence binding, and repair targeting. It is kept
//! separate from request inference and completion evaluation so new task kinds
//! or artifact roles do not force edits in the largest task-contract module.

/// The role an artifact plays in satisfying a task contract.
///
/// Issue #920: `#[non_exhaustive]` here is a **forward-marker only**.
/// `ArtifactRole` is `pub(super)` and never crosses the crate boundary, so the
/// attribute has **no effect on in-crate `match` exhaustiveness** — it does NOT
/// provide the extensibility this issue targets. Cascade-freedom comes from the
/// documented `_ =>` default arms at the Tier-A sites (e.g.
/// `default_deliverable_path`, `synthesized_missing_role_target_hint`,
/// `role_score`); the genuine 1:1 decision points (`label`/`from_label`,
/// `deliverable_kind_for_role`, `suggested_next_action`, the security guards
/// `file_matches_role`/`role_matches_path`, and the repair-priority ranks)
/// deliberately stay exhaustive so a new role compile-errors into a decision.
///
/// New variants MUST be **APPENDED after `DataOutput`** (never inserted): the
/// `Ord` derive drives `BTreeMap`/`BTreeSet`/`Vec<ArtifactRole>.sort()` ordering
/// and the `required_artifacts_completed_returns_btreemap_ordered` golden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
#[non_exhaustive]
pub(super) enum ArtifactRole {
    Implementation,
    Test,
    UsageDocs,
    Setup,
    DataOutput,
}

impl ArtifactRole {
    /// All variants in declaration (`Ord`) order. Issue #920: this is the
    /// totality SSOT so round-trip / Tier-A totality tests need no hand-listed
    /// variant set — adding a role updates only this one arm and the tests
    /// auto-cover it. New variants MUST be APPENDED after `DataOutput` (the
    /// `Ord` derive drives `BTreeMap`/`BTreeSet`/`Vec<ArtifactRole>.sort()`
    /// ordering and the `required_artifacts_completed_returns_btreemap_ordered`
    /// golden in `artifact_ledger.rs`).
    ///
    /// Test-only infrastructure (`#[cfg(test)]`): the production paths match on
    /// `ArtifactRole` directly; `all()` exists so the round-trip / Tier-A
    /// totality tests enumerate variants from a single source.
    #[cfg(test)]
    pub(super) const fn all() -> [ArtifactRole; 5] {
        [
            ArtifactRole::Implementation,
            ArtifactRole::Test,
            ArtifactRole::UsageDocs,
            ArtifactRole::Setup,
            ArtifactRole::DataOutput,
        ]
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ArtifactRole::Implementation => "implementation",
            ArtifactRole::Test => "test",
            ArtifactRole::UsageDocs => "usage_docs",
            ArtifactRole::Setup => "setup",
            ArtifactRole::DataOutput => "data_output",
        }
    }

    /// Strict reverse of [`ArtifactRole::label`] — accepts ONLY the canonical
    /// label strings that `label()` emits. Round-trip symmetric:
    /// `from_label(r.label()) == Some(r)` for every variant (incl.
    /// `data_output`); any other input returns `None` (Issue #920, Tier B: no
    /// `_ =>` default — a new role must declare its canonical string here).
    ///
    /// SRP boundary: LLM-origin *aliases* (`impl`/`code`/`docs`/`readme`/`data`
    /// /`output`, …) are NOT this function's responsibility. Alias-rich callers
    /// layer `from_label(normalized).or_else(|| <alias arm>)` so only the
    /// canonical vocabulary flows through this SSOT.
    ///
    /// SYNC: this canonical vocabulary must stay in sync with the LLM prompt
    /// allowed-values in `verifier_orchestration.rs` (string literals, so the
    /// link is manual; a drift golden test pins it).
    pub(super) fn from_label(s: &str) -> Option<ArtifactRole> {
        Some(match s {
            "implementation" => ArtifactRole::Implementation,
            "test" => ArtifactRole::Test,
            "usage_docs" => ArtifactRole::UsageDocs,
            "setup" => ArtifactRole::Setup,
            "data_output" => ArtifactRole::DataOutput,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskKind {
    Coding,
    Docs,
    Data,
    Research,
    Ops,
    /// Issue #919 (P2): translation / content writing / rewriting that
    /// *produces an artifact file* (`summarize`/`要約` deliberately excluded).
    /// Non-coding (verifier-free via `capability_for`), accept-tier completion
    /// (artifact present + non-empty + `AUTHORING_MIN_CONTENT_CHARS` + softened
    /// `required_sections_present`), routed through the thin `AuthoringVerifier`.
    Authoring,
}

impl TaskKind {
    /// Stable lowercase identifier for this task kind.
    ///
    /// Issue #918 (P1): promoted from the former `#[cfg(test)]`-only `label()`
    /// to a production accessor so the collapsed `VerifierDiagnostic.reason()`
    /// can format `task_kind={as_str}` directly off `TaskKind` (the per-verifier
    /// task-kind enum was folded into this one). The returned strings are
    /// byte-stable (`coding/docs/data/research/ops`) so reason/label goldens
    /// stay green.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            TaskKind::Coding => "coding",
            TaskKind::Docs => "docs",
            TaskKind::Data => "data",
            TaskKind::Research => "research",
            TaskKind::Ops => "ops",
            TaskKind::Authoring => "authoring",
        }
    }

    #[cfg(test)]
    pub(super) fn label(self) -> &'static str {
        self.as_str()
    }
}

#[allow(dead_code)] // Issue #864: generic deliverable variants are part of the model before every producer is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliverableKind {
    Code,
    Tests,
    UsageDocs,
    Setup,
    Data,
    ResearchNotes,
    OpsRunbook,
    File,
    Directory,
    CommandOutput,
    StructuredRecord,
    ExternalReference,
}

impl DeliverableKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            DeliverableKind::Code => "code",
            DeliverableKind::Tests => "tests",
            DeliverableKind::UsageDocs => "usage_docs",
            DeliverableKind::Setup => "setup",
            DeliverableKind::Data => "data",
            DeliverableKind::ResearchNotes => "research_notes",
            DeliverableKind::OpsRunbook => "ops_runbook",
            DeliverableKind::File => "file",
            DeliverableKind::Directory => "directory",
            DeliverableKind::CommandOutput => "command_output",
            DeliverableKind::StructuredRecord => "structured_record",
            DeliverableKind::ExternalReference => "external_reference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskDeliverable {
    pub(super) kind: DeliverableKind,
    pub(super) role: Option<ArtifactRole>,
    pub(super) path: Option<String>,
    pub(super) required_sections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructuredRecordSchema {
    pub(super) columns: Vec<String>,
    pub(super) expected_rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeliverableFormat {
    RustSource,
    JavaScriptSource,
    TypeScriptSource,
    Markdown,
    Toml,
    Json,
    Csv,
    Tsv,
    JsonLines,
    Text,
}

impl DeliverableFormat {
    pub(super) fn from_path(path: &str) -> Option<Self> {
        let lower = path.to_ascii_lowercase();
        if lower == "cargo.toml" || lower.ends_with(".toml") {
            return Some(Self::Toml);
        }
        if lower == "package.json" || lower.ends_with(".json") {
            return Some(Self::Json);
        }
        if lower.ends_with(".rs") {
            return Some(Self::RustSource);
        }
        if lower.ends_with(".ts") || lower.ends_with(".tsx") {
            return Some(Self::TypeScriptSource);
        }
        if lower.ends_with(".js") || lower.ends_with(".jsx") {
            return Some(Self::JavaScriptSource);
        }
        if lower.ends_with(".md") || lower.ends_with(".mdx") {
            return Some(Self::Markdown);
        }
        if lower.ends_with(".csv") {
            return Some(Self::Csv);
        }
        if lower.ends_with(".tsv") {
            return Some(Self::Tsv);
        }
        if lower.ends_with(".jsonl") || lower.ends_with(".ndjson") {
            return Some(Self::JsonLines);
        }
        if lower.ends_with(".txt") || lower.ends_with(".rst") {
            return Some(Self::Text);
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeliverableSchema {
    StructuredRecord(StructuredRecordSchema),
    JsonFields(Vec<String>),
    RequiredSections(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskIntent {
    Build,
    Modify,
    Fix,
    Install,
    Explain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectLanguage {
    Rust,
    Node,
    Python,
    Docs,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectShape {
    Cli,
    Library,
    Api,
    WebApp,
    Documentation,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationRequirement {
    NotRequired,
    Required {
        preferred_runner: Option<&'static str>,
    },
    ArtifactOnly,
}

/// Issue #975: objective-layer classification name for the generic
/// Objective/Evidence lifecycle. Coding is mainstreamed as *one* objective kind
/// (`ObjectiveKind::Coding`) rather than the privileged default, so docs / data
/// / research / ops / authoring objectives share the same lifecycle vocabulary.
///
/// This projects 1:1 from the classification-layer [`TaskKind`]: `TaskKind` is
/// *how the request was classified* (keyword inference + confirm), while
/// `ObjectiveKind` is *what the lifecycle is driving toward*. Keeping them as
/// distinct names — rather than reusing `TaskKind` directly at the lifecycle
/// layer — leaves a stable seam for future objectives that are not 1:1 with a
/// keyword-classified `TaskKind`. [`Self::from_task_kind`] is the SSOT mapping.
#[allow(dead_code)] // Issue #975: objective-layer vocabulary; producers wire incrementally (#947 pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectiveKind {
    Coding,
    Docs,
    Data,
    Research,
    Ops,
    Authoring,
}

impl ObjectiveKind {
    /// SSOT projection from the classification-layer [`TaskKind`].
    pub(super) fn from_task_kind(task_kind: TaskKind) -> Self {
        match task_kind {
            TaskKind::Coding => ObjectiveKind::Coding,
            TaskKind::Docs => ObjectiveKind::Docs,
            TaskKind::Data => ObjectiveKind::Data,
            TaskKind::Research => ObjectiveKind::Research,
            TaskKind::Ops => ObjectiveKind::Ops,
            TaskKind::Authoring => ObjectiveKind::Authoring,
        }
    }

    #[allow(dead_code)] // Issue #975: round-trip accessor for callers that bridge back to TaskKind.
    pub(super) fn to_task_kind(self) -> TaskKind {
        match self {
            ObjectiveKind::Coding => TaskKind::Coding,
            ObjectiveKind::Docs => TaskKind::Docs,
            ObjectiveKind::Data => TaskKind::Data,
            ObjectiveKind::Research => TaskKind::Research,
            ObjectiveKind::Ops => TaskKind::Ops,
            ObjectiveKind::Authoring => TaskKind::Authoring,
        }
    }

    #[allow(dead_code)] // Issue #975: byte-stable label reuses TaskKind::as_str so goldens stay green.
    pub(super) fn label(self) -> &'static str {
        self.to_task_kind().as_str()
    }

    #[allow(dead_code)] // Issue #975: coding is one objective kind, not the privileged default.
    pub(super) fn is_coding(self) -> bool {
        matches!(self, ObjectiveKind::Coding)
    }
}

/// Issue #947: projection vocabulary before all telemetry consumers are wired.
///
/// Issue #975: this is the canonical **DeliverableSpec** vocabulary the generic
/// Objective/Evidence lifecycle speaks (see the [`DeliverableSpec`] alias). It
/// can express every deliverable shape the lifecycle produces:
///
/// | request shape          | variant                                             |
/// |------------------------|-----------------------------------------------------|
/// | source / test / config | `SourceFiles` (role split lives in `ArtifactRole` / `DeliverableKind`) |
/// | document               | `DocumentSections`                                  |
/// | dataset                | `OutputFile`                                        |
/// | command result         | `CommandObservation`                                |
/// | research notes         | `ResearchNotes`                                     |
/// | visual observation     | `VisualObservation`                                 |
/// | explanation text       | `ProseArtifact` / `Answer`                          |
///
/// The objective layer intentionally groups source/test/config under a coding
/// objective's `SourceFiles`; the per-artifact role granularity (test vs config
/// vs source) is the obligation layer's job (`ArtifactRole` / `DeliverableKind`).
#[allow(dead_code)] // Issue #947: projection vocabulary before all telemetry consumers are wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectiveDeliverableKind {
    SourceFiles,
    DocumentSections,
    OutputFile,
    ResearchNotes,
    CommandObservation,
    /// Issue #975: media-reading / screenshot-description style deliverables
    /// whose product is an observation of visual content rather than a file
    /// edit or a command result.
    VisualObservation,
    ProseArtifact,
    Answer,
}

impl ObjectiveDeliverableKind {
    #[allow(dead_code)] // Issue #947: label projection is currently test/telemetry migration surface.
    pub(super) fn label(self) -> &'static str {
        match self {
            ObjectiveDeliverableKind::SourceFiles => "source_files",
            ObjectiveDeliverableKind::DocumentSections => "document_sections",
            ObjectiveDeliverableKind::OutputFile => "output_file",
            ObjectiveDeliverableKind::ResearchNotes => "research_notes",
            ObjectiveDeliverableKind::CommandObservation => "command_observation",
            ObjectiveDeliverableKind::VisualObservation => "visual_observation",
            ObjectiveDeliverableKind::ProseArtifact => "prose_artifact",
            ObjectiveDeliverableKind::Answer => "answer",
        }
    }
}

/// Issue #975: lifecycle-layer name for the canonical deliverable taxonomy.
/// Aliasing (rather than introducing a third parallel enum) keeps the generic
/// vocabulary single-sourced per the "abstraction を増やさない" project rule.
#[allow(dead_code)] // Issue #975: named scope for the generic lifecycle; consumers wire incrementally.
pub(super) type DeliverableSpec = ObjectiveDeliverableKind;

/// Issue #947: projection vocabulary before all telemetry consumers are wired.
///
/// Issue #975: this is the canonical **EvidenceSpec** vocabulary (see the
/// [`EvidenceSpec`] alias). It can express every evidence shape the lifecycle
/// accepts:
///
/// | evidence shape       | variant                  |
/// |----------------------|--------------------------|
/// | test run             | `TestRun`                |
/// | content check        | `ContentCheck`           |
/// | schema check         | `SchemaCheck`            |
/// | command observation  | `SafetyBoundaryEvidence` |
/// | source citation      | `SourceFetchEvidence`    |
/// | file layout          | `FileLayoutCheck`        |
/// | explanation coverage | `ContentAcceptance`      |
#[allow(dead_code)] // Issue #947: projection vocabulary before all telemetry consumers are wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectiveEvidenceKind {
    TestRun,
    ContentCheck,
    SchemaCheck,
    SourceFetchEvidence,
    SafetyBoundaryEvidence,
    /// Issue #975: file-organization / layout evidence — the deliverable is a
    /// directory shape or file placement, verified by observing the resulting
    /// layout rather than running a test or parsing a schema.
    FileLayoutCheck,
    ContentAcceptance,
}

impl ObjectiveEvidenceKind {
    #[allow(dead_code)] // Issue #947: label projection is currently test/telemetry migration surface.
    pub(super) fn label(self) -> &'static str {
        match self {
            ObjectiveEvidenceKind::TestRun => "test_run",
            ObjectiveEvidenceKind::ContentCheck => "content_check",
            ObjectiveEvidenceKind::SchemaCheck => "schema_check",
            ObjectiveEvidenceKind::SourceFetchEvidence => "source_fetch_evidence",
            ObjectiveEvidenceKind::SafetyBoundaryEvidence => "safety_boundary_evidence",
            ObjectiveEvidenceKind::FileLayoutCheck => "file_layout_check",
            ObjectiveEvidenceKind::ContentAcceptance => "content_acceptance",
        }
    }
}

/// Issue #975: lifecycle-layer name for the canonical evidence taxonomy. See
/// [`DeliverableSpec`] for the rationale behind aliasing instead of adding a
/// parallel enum.
#[allow(dead_code)] // Issue #975: named scope for the generic lifecycle; consumers wire incrementally.
pub(super) type EvidenceSpec = ObjectiveEvidenceKind;
