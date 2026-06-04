use super::completion_evidence::CompletionEvidence;
use super::failure_packet::{CandidateArtifact, FailurePacket};
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableFormat, DeliverableKind, DeliverableSchema,
    TaskKind,
};
use crate::tools::bash::BashCommandClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierDiagnosticCode {
    MissingFile,
    InvalidManifest,
    BadTest,
    WrongSemantics,
    EvidenceMissing,
    SchemaMismatch,
}

impl VerifierDiagnosticCode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "missing_file",
            Self::InvalidManifest => "invalid_manifest",
            Self::BadTest => "bad_test",
            Self::WrongSemantics => "wrong_semantics",
            Self::EvidenceMissing => "evidence_missing",
            Self::SchemaMismatch => "schema_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierDiagnostic {
    pub(super) task_kind: TaskKind,
    pub(super) code: VerifierDiagnosticCode,
    pub(super) role: ArtifactRole,
    pub(super) path: Option<String>,
    pub(super) message: String,
}

impl VerifierDiagnostic {
    fn new(
        task_kind: TaskKind,
        code: VerifierDiagnosticCode,
        role: ArtifactRole,
        path: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            task_kind,
            code,
            role,
            path: path.map(ToString::to_string),
            message: message.into(),
        }
    }

    #[allow(dead_code)] // Issue #902 migration surface; exercised by verifier tests.
    pub(super) fn to_failure_packet(&self, command: &str, output: &str) -> FailurePacket {
        let candidate_artifacts = self
            .path
            .as_ref()
            .map(|path| {
                let reason = format!(
                    "{} verifier diagnostic: {}",
                    self.task_kind.as_str(),
                    self.message
                );
                vec![CandidateArtifact::new(self.role, path, &reason)]
            })
            .unwrap_or_default();
        FailurePacket::new(
            command,
            self.code.as_str(),
            output,
            Vec::new(),
            Vec::new(),
            candidate_artifacts,
            Vec::new(),
        )
    }

    pub(super) fn reason(&self) -> String {
        format!(
            "structured verifier diagnostic: kind={}, task_kind={}, summary={}",
            self.code.as_str(),
            self.task_kind.as_str(),
            self.message
        )
    }
}

#[allow(dead_code)]
pub(super) trait Verifier {
    fn task_kind(&self) -> TaskKind;

    fn pass_evidence(
        &self,
        command: &str,
        bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence;

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence>;

    fn diagnostic(&self, artifact: VerifierArtifact<'_>) -> Option<VerifierDiagnostic> {
        verifier_diagnostic_for_artifact(self.task_kind(), artifact)
    }

    fn diagnose_obligation(
        &self,
        obligation: &ArtifactObligation,
        excerpt: Option<&str>,
        path_exists: bool,
    ) -> Option<VerifierDiagnostic> {
        verifier_diagnostic_for_obligation_parts(self.task_kind(), obligation, excerpt, path_exists)
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket;
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) struct VerifierArtifact<'a> {
    pub(super) path: Option<&'a str>,
    pub(super) excerpt: &'a str,
    pub(super) required_columns: &'a [String],
    /// Issue #922 (P5 / DR2-001): obligation-derived required sections, mirroring
    /// `required_columns`. Read by the research acceptance predicate so the
    /// artifact-evidence path and the obligation path share one input contract.
    /// Empty slice = no fixed sections (open-ended).
    pub(super) required_sections: &'a [String],
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct CodingVerifier;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DocsVerifier;

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct DataVerifier;

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct ResearchVerifier;

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct OpsVerifier;

/// Issue #919 (P2 / Decision #3 site #1 / OR-2): a thin verifier strategy whose
/// `task_kind()` returns `TaskKind::Authoring` (preserving the
/// `verifier_for_task_kind` round-trip invariant verbatim) and which delegates
/// `pass_evidence` / `artifact_evidence` / `failure_packet` to the docs
/// free-fns. The *accept-tier* (min-length / requested-sections) is routed
/// through `verifier_diagnostic_for_artifact`'s `Authoring` arm and the
/// obligation-parts semantic branch (`authoring_accept_tier_diagnostic`), not
/// here. We deliberately do NOT reuse `&DOCS_VERIFIER` because that would make
/// `verifier_for_task_kind(Authoring).task_kind() == Docs ≠ Authoring`,
/// breaking the #918 OCP fail-safe round-trip invariant.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct AuthoringVerifier;

static CODING_VERIFIER: CodingVerifier = CodingVerifier;
static DOCS_VERIFIER: DocsVerifier = DocsVerifier;
static DATA_VERIFIER: DataVerifier = DataVerifier;
static RESEARCH_VERIFIER: ResearchVerifier = ResearchVerifier;
static OPS_VERIFIER: OpsVerifier = OpsVerifier;
static AUTHORING_VERIFIER: AuthoringVerifier = AuthoringVerifier;

#[allow(dead_code)]
pub(super) fn verifier_for_task_kind(task_kind: TaskKind) -> &'static dyn Verifier {
    match task_kind {
        TaskKind::Coding => &CODING_VERIFIER,
        TaskKind::Docs => &DOCS_VERIFIER,
        TaskKind::Data => &DATA_VERIFIER,
        TaskKind::Research => &RESEARCH_VERIFIER,
        TaskKind::Ops => &OPS_VERIFIER,
        TaskKind::Authoring => &AUTHORING_VERIFIER,
    }
}

/// Issue #918 (P1): the single `capability_for(TaskKind)` dispatch spine.
///
/// A small `Copy` descriptor (no allocation, no boxing) that answers two
/// per-kind capability questions used by the verification-requirement gates
/// and the structured-verifier process-spawn guard. This is intentionally a
/// separate concern from [`verifier_for_task_kind`] (which selects the
/// `&'static dyn Verifier` *strategy*): the capability describes *whether* a
/// kind requires executable verification / may spawn a process, the strategy
/// describes *how* it verifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskCapability {
    kind: TaskKind,
    /// Only `TaskKind::Coding` may spawn a structured-verifier process.
    allows_process_exec: bool,
}

/// SSOT static `match` mapping each [`TaskKind`] to its [`TaskCapability`].
///
/// The non-coding arm is written as an explicit enumeration (never a `_`
/// wildcard) so that adding a sixth `TaskKind` is a compile error here,
/// forcing the capability of any new kind to be reviewed (OCP / fail-safe).
pub(super) const fn capability_for(kind: TaskKind) -> TaskCapability {
    match kind {
        TaskKind::Coding => TaskCapability {
            kind,
            allows_process_exec: true,
        },
        TaskKind::Docs
        | TaskKind::Data
        | TaskKind::Research
        | TaskKind::Ops
        | TaskKind::Authoring => TaskCapability {
            kind,
            allows_process_exec: false,
        },
    }
}

impl TaskCapability {
    /// Whether the structured verifier for this kind may spawn a child process.
    /// Only `CodingCapability` (`TaskKind::Coding`) returns `true`.
    pub(super) const fn allows_process_exec(self) -> bool {
        self.allows_process_exec
    }

    /// 1:1 replacement for the historical `coding_verifier_required` gate
    /// (`task_kind == Coding && !verifier_free_document_task`).
    ///
    /// This is a **bare gate**: it depends only on `verifier_free_document_task`
    /// (the DocsOnly/AnswerOnly suppression derived from project intent), NOT on
    /// `verification_required` or `test_execution_required`. Callers AND this
    /// result into each of those two fields *separately* — exactly as the
    /// pre-#918 code did — so no new inter-field dependency is introduced.
    ///
    /// Non-coding kinds are enumerated explicitly and always return `false`,
    /// preserving today's effective behavior (a sixth kind would be a compile
    /// error). See design policy §5.1 (the non-coding behavior-preservation
    /// invariant): `test_execution_required` is derived from request text and is
    /// kind-independent, so this clamp is the primary guard against a non-coding
    /// task ever requiring executable verification.
    pub(super) const fn requires_executable_verifier(
        self,
        verifier_free_document_task: bool,
    ) -> bool {
        match self.kind {
            TaskKind::Coding => !verifier_free_document_task,
            TaskKind::Docs
            | TaskKind::Data
            | TaskKind::Research
            | TaskKind::Ops
            | TaskKind::Authoring => false,
        }
    }
}

pub(super) fn verifier_diagnostic_for_obligation(
    task_kind: TaskKind,
    obligation: &ArtifactObligation,
    excerpt: Option<&str>,
    path_exists: bool,
) -> Option<VerifierDiagnostic> {
    verifier_for_task_kind(task_kind).diagnose_obligation(obligation, excerpt, path_exists)
}

impl Verifier for CodingVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Coding
    }

    fn pass_evidence(
        &self,
        command: &str,
        bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: command.to_string(),
            bound_test_artifacts_count: bound_artifacts_count,
        }
    }

    fn artifact_evidence(&self, _artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        None
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl Verifier for DocsVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Docs
    }

    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::RequiredSectionsPass { path: None }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.required_sections_evidence(artifact.path.map(str::to_string), artifact.excerpt)
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl DocsVerifier {
    pub(super) fn required_sections_pass(&self, excerpt: &str) -> bool {
        docs_required_sections_pass(excerpt)
    }

    #[allow(dead_code)]
    pub(super) fn required_sections_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
    ) -> Option<CompletionEvidence> {
        if self.required_sections_pass(excerpt) {
            Some(CompletionEvidence::RequiredSectionsPass { path })
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub(super) fn required_sections_failure_packet(
        &self,
        path: &str,
        excerpt: &str,
    ) -> FailurePacket {
        FailurePacket::new(
            "docs required sections",
            "required_sections_missing",
            excerpt,
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::UsageDocs,
                path,
                "documentation required sections verifier target",
            )],
            Vec::new(),
        )
    }
}

impl Verifier for AuthoringVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Authoring
    }

    // Issue #919 (Decision #3 site #1): delegate the *how* to the docs
    // free-fns. `pass_evidence` stays `RequiredSectionsPass` (consistent with
    // DocsVerifier); the Authoring accept-tier (min-length / requested-sections)
    // is enforced by `authoring_accept_tier_diagnostic` via the diagnostic path,
    // not by overriding these strategy methods.
    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::RequiredSectionsPass { path: None }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        DOCS_VERIFIER
            .required_sections_evidence(artifact.path.map(str::to_string), artifact.excerpt)
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl Verifier for DataVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Data
    }

    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::StructuredDataPass {
            path: None,
            columns: Vec::new(),
        }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.structured_data_evidence(
            artifact.path.map(str::to_string),
            artifact.excerpt,
            artifact.required_columns,
        )
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl DataVerifier {
    #[allow(dead_code)]
    pub(super) fn structured_data_pass(
        &self,
        path: Option<&str>,
        excerpt: &str,
        columns: &[String],
    ) -> bool {
        structured_data_pass(path, excerpt, columns)
    }

    #[allow(dead_code)]
    pub(super) fn structured_data_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
        required_columns: &[String],
    ) -> Option<CompletionEvidence> {
        let path_ref = path.as_deref();
        if !self.structured_data_pass(path_ref, excerpt, required_columns) {
            return None;
        }
        let columns = observed_data_columns(path_ref, excerpt, required_columns);
        Some(CompletionEvidence::StructuredDataPass { path, columns })
    }

    #[allow(dead_code)]
    pub(super) fn structured_data_failure_packet(
        &self,
        path: &str,
        excerpt: &str,
    ) -> FailurePacket {
        FailurePacket::new(
            "structured data artifact",
            "structured_data_schema_missing",
            excerpt,
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::DataOutput,
                path,
                "structured data verifier target",
            )],
            Vec::new(),
        )
    }
}

impl Verifier for ResearchVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Research
    }

    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::ReportCompletenessPass { path: None }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.research_report_evidence(
            artifact.path.map(str::to_string),
            artifact.excerpt,
            artifact.required_sections,
        )
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl ResearchVerifier {
    #[allow(dead_code)]
    pub(super) fn research_report_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
        required_sections: &[String],
    ) -> Option<CompletionEvidence> {
        // Issue #922 (P5): SSOT predicate. OR-tolerant — sectioned coverage OR
        // open-ended floor; no fixed citation∧claim∧uncertainty AND.
        assess_research_report(excerpt, required_sections)
            .tier
            .is_accepted()
            .then_some(CompletionEvidence::ReportCompletenessPass { path })
    }
}

impl Verifier for OpsVerifier {
    fn task_kind(&self) -> TaskKind {
        TaskKind::Ops
    }

    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::ReportCompletenessPass { path: None }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.ops_runbook_evidence(artifact.path.map(str::to_string), artifact.excerpt)
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl OpsVerifier {
    #[allow(dead_code)]
    pub(super) fn ops_runbook_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
    ) -> Option<CompletionEvidence> {
        // Test-only artifact adapter: no request context, so the context-free
        // (core + >=3/4) predicate applies (DSR1-003).
        if ops_runbook_pass(excerpt, &[]) {
            Some(CompletionEvidence::ReportCompletenessPass { path })
        } else {
            None
        }
    }
}

#[allow(dead_code)]
fn generic_verifier_failure_packet(
    command: &str,
    failure_kind: &str,
    output: &str,
) -> FailurePacket {
    FailurePacket::new(
        command,
        failure_kind,
        output,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
}

fn verifier_diagnostic_for_artifact(
    task_kind: TaskKind,
    artifact: VerifierArtifact<'_>,
) -> Option<VerifierDiagnostic> {
    let Some(path) = artifact.path else {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::MissingFile,
            default_role_for_task_kind(task_kind),
            None,
            "artifact path is missing",
        ));
    };
    if path.trim().is_empty() {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::MissingFile,
            default_role_for_task_kind(task_kind),
            Some(path),
            "artifact path is empty",
        ));
    }
    if let Some(diagnostic) = manifest_readiness_diagnostic(task_kind, path, artifact.excerpt) {
        return Some(diagnostic);
    }
    if artifact.excerpt.trim().is_empty() {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            role_for_path_or_task_kind(path, task_kind),
            Some(path),
            "artifact evidence is empty",
        ));
    }
    match task_kind {
        TaskKind::Coding => coding_artifact_diagnostic(task_kind, path, artifact.excerpt),
        TaskKind::Docs => (!docs_required_sections_pass(artifact.excerpt)).then(|| {
            VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                ArtifactRole::UsageDocs,
                Some(path),
                "documentation evidence is missing required setup/run/verify coverage",
            )
        }),
        TaskKind::Data => {
            data_artifact_diagnostic(task_kind, path, artifact.excerpt, artifact.required_columns)
        }
        TaskKind::Research => {
            (!assess_research_report(artifact.excerpt, artifact.required_sections)
                .tier
                .is_accepted())
            .then(|| {
                VerifierDiagnostic::new(
                    task_kind,
                    VerifierDiagnosticCode::EvidenceMissing,
                    ArtifactRole::UsageDocs,
                    Some(path),
                    "research evidence does not meet section coverage or open-ended report floor",
                )
            })
        }
        TaskKind::Ops => (!ops_runbook_pass(artifact.excerpt, &[])).then(|| {
            VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                ArtifactRole::UsageDocs,
                Some(path),
                OPS_RUNBOOK_DIAGNOSTIC_MESSAGE,
            )
        }),
        // Issue #919 (Decision #3 site #4 / Decision #4): accept-tier — empty
        // excerpt already short-circuited at `:521`; this arm enforces min-length
        // and any user-named sections (NOT the docs setup/run/verify gate).
        TaskKind::Authoring => {
            authoring_accept_tier_diagnostic(task_kind, path, artifact.excerpt, &[])
        }
    }
}

fn verifier_diagnostic_for_obligation_parts(
    task_kind: TaskKind,
    obligation: &ArtifactObligation,
    excerpt: Option<&str>,
    path_exists: bool,
) -> Option<VerifierDiagnostic> {
    if !path_exists {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::MissingFile,
            obligation.role,
            Some(&obligation.path),
            "required deliverable path was not observed",
        ));
    }
    // Issue #919 (Decision #4 / DR3-003): the Authoring UsageDocs accept-tier,
    // shared with the artifact-diagnostic path (`authoring_accept_tier_diagnostic`).
    // This is the production recovery path (`task_contract.rs` artifact
    // completion / recovery calls `verifier_diagnostic_for_obligation`). When
    // the path exists but no excerpt is available yet, we cannot prove the
    // accept-tier, so we report it missing (consistent with the docs path).
    if task_kind == TaskKind::Authoring && obligation.role == ArtifactRole::UsageDocs {
        let Some(excerpt) = excerpt else {
            return Some(VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                obligation.role,
                Some(&obligation.path),
                "authoring artifact is too short or missing requested sections",
            ));
        };
        let sections = match obligation.schema.as_ref() {
            Some(DeliverableSchema::RequiredSections(sections)) => sections.as_slice(),
            _ => &[],
        };
        return authoring_accept_tier_diagnostic(task_kind, &obligation.path, excerpt, sections);
    }
    if obligation_requires_manifest_parse(obligation) {
        let Some(excerpt) = excerpt else {
            return Some(VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                obligation.role,
                Some(&obligation.path),
                "manifest path exists but no parse evidence is available",
            ));
        };
        if let Some(detail) = manifest_readiness_diagnostic(task_kind, &obligation.path, excerpt) {
            return Some(VerifierDiagnostic::new(
                task_kind,
                detail.code,
                obligation.role,
                Some(&obligation.path),
                detail.message,
            ));
        }
    }
    if let Some(DeliverableSchema::JsonFields(fields)) = obligation.schema.as_ref() {
        let Some(excerpt) = excerpt else {
            return Some(VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                obligation.role,
                Some(&obligation.path),
                "JSON schema obligation has no field evidence",
            ));
        };
        if !json_fields_present(excerpt, fields) {
            return Some(VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::SchemaMismatch,
                obligation.role,
                Some(&obligation.path),
                "JSON manifest is missing required field evidence",
            ));
        }
    }
    if let Some(DeliverableSchema::StructuredRecord(schema)) = obligation.schema.as_ref() {
        let Some(excerpt) = excerpt else {
            return Some(VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                obligation.role,
                Some(&obligation.path),
                "structured data schema obligation has no parse evidence",
            ));
        };
        if let Some(diagnostic) =
            data_artifact_diagnostic(task_kind, &obligation.path, excerpt, &schema.columns)
        {
            return Some(diagnostic);
        }
    }
    // Issue #922 (S3-002 / DR3-002): the docs-shaped surface gate
    // (`docs_required_sections_pass`) must NOT apply to research obligations.
    // Research `RequiredSections` are evaluated by the research arm below via
    // `assess_research_report`, so exclude `TaskKind::Research` here.
    if task_kind != TaskKind::Research
        && let Some(DeliverableSchema::RequiredSections(sections)) = obligation.schema.as_ref()
        && !sections.is_empty()
        && let Some(excerpt) = excerpt
        && (!required_sections_present(excerpt, sections) || !docs_required_sections_pass(excerpt))
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            obligation.role,
            Some(&obligation.path),
            "documentation required sections are absent from the observed content",
        ));
    }
    if task_kind == TaskKind::Coding
        && obligation.role == ArtifactRole::Test
        && let Some(excerpt) = excerpt
        && !contains_test_assertion_shape(excerpt)
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::BadTest,
            obligation.role,
            Some(&obligation.path),
            "test artifact lacks a recognizable assertion or test case shape",
        ));
    }
    if task_kind == TaskKind::Coding
        && obligation.role == ArtifactRole::Implementation
        && let Some(excerpt) = excerpt
        && implementation_looks_semantically_wrong(excerpt)
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::WrongSemantics,
            obligation.role,
            Some(&obligation.path),
            "implementation artifact still looks placeholder or non-semantic",
        ));
    }
    // Issue #922 (P5): research obligations route through the SSOT predicate
    // with the obligation's own `required_sections` (sectioned path) or the
    // open-ended floor when none are declared. No docs-shaped gating.
    if task_kind == TaskKind::Research
        && let Some(excerpt) = excerpt
        && !assess_research_report(excerpt, &obligation.required_sections)
            .tier
            .is_accepted()
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            obligation.role,
            Some(&obligation.path),
            "research evidence does not meet section coverage or open-ended report floor",
        ));
    }
    // Issue #923 (DR3-003): the tiered Ops gate applies only to OpsRunbook
    // obligations, so a setup/install artifact routed to `TaskKind::Ops` is not
    // judged as a runbook. `obligation.required_sections` carries the
    // request-derived mandatory sections (canonical labels, DR4-002).
    if task_kind == TaskKind::Ops
        && obligation.kind == DeliverableKind::OpsRunbook
        && let Some(excerpt) = excerpt
        && !ops_runbook_pass(excerpt, &obligation.required_sections)
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            obligation.role,
            Some(&obligation.path),
            OPS_RUNBOOK_DIAGNOSTIC_MESSAGE,
        ));
    }
    None
}

fn coding_artifact_diagnostic(
    task_kind: TaskKind,
    path: &str,
    excerpt: &str,
) -> Option<VerifierDiagnostic> {
    if is_test_artifact_path(path) && !contains_test_assertion_shape(excerpt) {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::BadTest,
            ArtifactRole::Test,
            Some(path),
            "test artifact lacks a recognizable assertion or test case shape",
        ));
    }
    None
}

fn data_artifact_diagnostic(
    task_kind: TaskKind,
    path: &str,
    excerpt: &str,
    required_columns: &[String],
) -> Option<VerifierDiagnostic> {
    // Issue #921 (P4 / DD2 / DR1-004): route through the OR-tolerant SSOT and
    // return a BLOCKING diagnostic ONLY when the tier is `Insufficient`. For
    // `SchemaSatisfied` / `AcceptTier` we MUST return strictly `None` — not
    // `EvidenceMissing` — because `artifact_identity_satisfied_for_verification`
    // only treats a diagnostic as a pass when it is `EvidenceMissing &&
    // excerpt.is_none()`. With a present excerpt that whitelist fails, so any
    // non-None diagnostic here would re-block the accept tier and make it
    // unreachable. The parse-error path stays `Insufficient` (→ blocking) and
    // keeps its specific message for better recovery hints.
    if let StructuredDataTier::Insufficient =
        assess_structured_data(Some(path), excerpt, required_columns)
    {
        let message = structured_data_parse_error(path, excerpt).unwrap_or(
            "structured data evidence is missing required columns or parse-ready records",
        );
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::SchemaMismatch,
            ArtifactRole::DataOutput,
            Some(path),
            message,
        ));
    }
    None
}

fn manifest_readiness_diagnostic(
    task_kind: TaskKind,
    path: &str,
    excerpt: &str,
) -> Option<VerifierDiagnostic> {
    let normalized = normalize_path_label(path);
    let message = if normalized == "package.json" {
        invalid_package_manifest_excerpt_detail(excerpt)
    } else if normalized == "cargo.toml" {
        invalid_cargo_manifest_excerpt_detail(excerpt)
    } else {
        None
    }?;
    Some(VerifierDiagnostic::new(
        task_kind,
        VerifierDiagnosticCode::InvalidManifest,
        ArtifactRole::Setup,
        Some(path),
        message,
    ))
}

fn obligation_requires_manifest_parse(obligation: &ArtifactObligation) -> bool {
    obligation.role == ArtifactRole::Setup
        && matches!(
            obligation.format,
            Some(DeliverableFormat::Json | DeliverableFormat::Toml)
        )
}

fn json_fields_present(excerpt: &str, fields: &[String]) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(excerpt) else {
        return false;
    };
    fields.iter().all(|field| value.get(field).is_some())
}

/// Issue #919 (Decision #6): OR-tolerant user-named section gate.
///
/// Softened from pure-AND (`.all()`) to **M-of-N (majority)** while preserving
/// the empty-list pass semantics: an empty list is still a pass (same as
/// `.all()`), `N=1` still requires that section (unchanged), and for `N=2` a
/// single hit now passes (was 2). This *loosens* completion (fewer
/// false-negatives) and never tightens. It is the single SSOT for the M-of-N
/// predicate — the Authoring accept tier (`authoring_accept_tier_diagnostic`)
/// calls this rather than re-implementing the majority logic inline. The
/// `docs_required_sections_pass` `categories>=2` gate and the
/// `verifier_diagnostic_for_obligation_parts` `:639` composition are unchanged.
fn required_sections_present(excerpt: &str, sections: &[String]) -> bool {
    if sections.is_empty() {
        return true;
    }
    let lower = excerpt.to_ascii_lowercase();
    let hits = sections
        .iter()
        .filter(|section| section_present(&lower, section))
        .count();
    hits * 2 >= sections.len()
}

/// Per-section presence check (the historical `## {n}` / `# {n}` / substring
/// logic), extracted from `required_sections_present` so the M-of-N predicate
/// has a single per-section SSOT. `lower` must already be lowercased.
fn section_present(lower: &str, section: &str) -> bool {
    let normalized = section.to_ascii_lowercase();
    lower.contains(&format!("## {normalized}"))
        || lower.contains(&format!("# {normalized}"))
        || lower.contains(&normalized)
}

fn implementation_looks_semantically_wrong(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "todo",
            "placeholder",
            "not implemented",
            "unimplemented",
            "dummy",
        ],
    )
}

fn structured_data_parse_error(path: &str, excerpt: &str) -> Option<&'static str> {
    match path_extension(path).as_deref() {
        Some("json") => serde_json::from_str::<serde_json::Value>(excerpt)
            .is_err()
            .then_some("JSON data artifact is not parse-ready"),
        Some("jsonl") | Some("ndjson") => excerpt
            .lines()
            .filter(|line| !line.trim().is_empty())
            .any(|line| serde_json::from_str::<serde_json::Value>(line).is_err())
            .then_some("JSONL data artifact contains a non-parseable record"),
        Some("csv") => delimited_header_columns(excerpt, ',')
            .is_empty()
            .then_some("CSV data artifact is missing a parse-ready header"),
        Some("tsv") => delimited_header_columns(excerpt, '\t')
            .is_empty()
            .then_some("TSV data artifact is missing a parse-ready header"),
        _ => None,
    }
}

fn invalid_package_manifest_excerpt_detail(excerpt: &str) -> Option<&'static str> {
    serde_json::from_str::<serde_json::Value>(excerpt)
        .is_err()
        .then_some("package.json is not valid JSON")
}

fn invalid_cargo_manifest_excerpt_detail(excerpt: &str) -> Option<&'static str> {
    let mut in_package = false;
    let mut saw_package = false;
    let mut saw_name = false;
    for raw_line in excerpt.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Some("Cargo.toml contains a malformed table header");
            }
            in_package = line == "[package]";
            saw_package |= in_package;
            continue;
        }
        if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            let Some(value) = parse_toml_string_value(value.trim()) else {
                return Some("Cargo.toml [package] name is malformed");
            };
            saw_name = !value.is_empty();
        }
    }
    if !saw_package {
        return Some("Cargo.toml does not contain a [package] section");
    }
    if !saw_name {
        return Some("Cargo.toml [package] does not declare a non-empty name");
    }
    None
}

fn parse_toml_string_value(value: &str) -> Option<String> {
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"')) || value.len() < 2 {
        return None;
    }
    Some(value[1..value.len() - 1].trim().to_string())
}

fn strip_toml_comment(line: &str) -> &str {
    line.split_once('#')
        .map(|(before, _)| before)
        .unwrap_or(line)
}

fn default_role_for_task_kind(task_kind: TaskKind) -> ArtifactRole {
    match task_kind {
        TaskKind::Coding => ArtifactRole::Implementation,
        TaskKind::Docs | TaskKind::Research | TaskKind::Ops | TaskKind::Authoring => {
            ArtifactRole::UsageDocs
        }
        TaskKind::Data => ArtifactRole::DataOutput,
    }
}

fn role_for_path_or_task_kind(path: &str, task_kind: TaskKind) -> ArtifactRole {
    let normalized = normalize_path_label(path);
    if normalized == "package.json" || normalized == "cargo.toml" {
        return ArtifactRole::Setup;
    }
    if is_test_artifact_path(path) {
        return ArtifactRole::Test;
    }
    default_role_for_task_kind(task_kind)
}

fn is_test_artifact_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("__tests__")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.py")
        || lower.ends_with("test.rs")
}

fn contains_test_assertion_shape(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "#[test]",
            "assert!",
            "assert_eq!",
            "assert ",
            "expect(",
            "it(",
            "test(",
            "pytest",
        ],
    )
}

fn normalize_path_label(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

fn path_extension(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

/// Issue #919 (Decision #4 / OR-4): minimum trimmed content length (in chars)
/// for an Authoring accept-tier artifact. Dedicated SSOT — do NOT reuse an
/// unrelated cap. Lowered to 24 (from a proposed 64) so a legitimate
/// one-sentence translation (e.g. "Run the server.") is not false-negatived,
/// while stubs ("TODO", "see above", "placeholder") are still rejected. The
/// empty/whitespace short-circuit at `verifier_diagnostic_for_artifact:521`
/// already covers truly empty artifacts.
pub(super) const AUTHORING_MIN_CONTENT_CHARS: usize = 24;

/// Issue #919 (Decision #4 / DR3-003): shared accept-tier predicate for
/// `TaskKind::Authoring`. Called from BOTH `verifier_diagnostic_for_artifact`'s
/// `Authoring` arm AND the `verifier_diagnostic_for_obligation_parts` Authoring
/// UsageDocs semantic branch (the production recovery path).
///
/// It enforces only the accept-tier:
/// 1. min trimmed length (`AUTHORING_MIN_CONTENT_CHARS`); and
/// 2. if the user named sections, the **shared** Decision #6-softened
///    `required_sections_present` (M-of-N) — it does NOT re-implement the
///    majority logic inline and does NOT apply the docs `categories>=2` gate.
///
/// The diagnostic message is a static string (no path / section / excerpt
/// interpolation — Security §5).
pub(super) fn authoring_accept_tier_diagnostic(
    task_kind: TaskKind,
    path: &str,
    excerpt: &str,
    sections: &[String],
) -> Option<VerifierDiagnostic> {
    let too_short = excerpt.trim().chars().count() < AUTHORING_MIN_CONTENT_CHARS;
    let missing_sections = !required_sections_present(excerpt, sections);
    if too_short || missing_sections {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            ArtifactRole::UsageDocs,
            Some(path),
            "authoring artifact is too short or missing requested sections",
        ));
    }
    None
}

const USAGE_DOCS_SETUP_MARKERS: &[(&str, bool)] = &[
    ("install", true),
    ("setup", false),
    ("configuration", false),
    ("dependency", false),
    ("dependencies", false),
    ("package", false),
    ("requirements", false),
    ("cargo.toml", false),
    ("package.json", false),
    ("pyproject.toml", false),
    ("セットアップ", false),
    ("依存", false),
    ("設定", false),
];
const USAGE_DOCS_RUN_MARKERS: &[(&str, bool)] = &[
    ("run", true),
    ("start", false),
    ("usage", false),
    ("example", false),
    ("build", false),
    ("execute", false),
    ("使用", false),
    ("使い方", false),
    ("実行", false),
    ("例", false),
    ("ビルド", false),
];
const USAGE_DOCS_VERIFY_MARKERS: &[(&str, bool)] = &[
    ("test", false),
    ("verify", false),
    ("check", false),
    ("pytest", false),
    ("cargo test", false),
    ("npm test", false),
    ("テスト", false),
    ("検証", false),
];

fn docs_required_sections_pass(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let hit = |markers: &[(&str, bool)]| -> bool {
        markers
            .iter()
            .any(|(needle, token_boundary)| docs_marker_hit(&lower, needle, *token_boundary))
    };
    let mut categories = 0;
    if hit(USAGE_DOCS_SETUP_MARKERS) {
        categories += 1;
    }
    if hit(USAGE_DOCS_RUN_MARKERS) {
        categories += 1;
    }
    if hit(USAGE_DOCS_VERIFY_MARKERS) {
        categories += 1;
    }
    categories >= 2
}

fn docs_marker_hit(lower: &str, needle: &str, token_boundary: bool) -> bool {
    if token_boundary {
        contains_ascii_token(lower, needle)
    } else {
        lower.contains(needle)
    }
}

fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| token == needle)
}

/// Issue #921 (P4): minimum trimmed `chars().count()` an excerpt must clear to
/// qualify for the OR-tolerant `AcceptTier` (declared-but-missing columns).
///
/// Rationale: a structured-data artifact that has a parse-ready header plus at
/// least one record is the smallest honest deliverable. A single CSV header row
/// plus one record such as a two-column comma header with one value row is
/// already above this floor, and even a minimal two-line `id` + `1` file clears
/// it. The unit is **chars** (`chars().count()`), not bytes, so CJK
/// headers/records are not unfairly penalized (DR1-003 / DR2-003). The floor is
/// applied ONLY in the `AcceptTier` branch, never to `SchemaSatisfied`, so it
/// cannot regress any artifact the historical column-conjunctive
/// `structured_data_pass` already accepted (DR1-002).
pub(super) const STRUCTURED_DATA_MIN_CHARS: usize = 16;

/// Issue #921 (P4): the three-tier outcome of the single OR-tolerant
/// structured-data acceptance predicate ([`assess_structured_data`]).
///
/// Intentionally a CLOSED enum (NOT `#[non_exhaustive]`): the completion gate is
/// a closed decision and callers exhaustively match all tiers, so adding a tier
/// must be a deliberate, compile-checked change (DR1-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StructuredDataTier {
    /// Declared columns all observed, OR no columns declared — parse-ready and
    /// non-empty. No floor is applied (regression-free w.r.t. the old pass).
    SchemaSatisfied,
    /// Parse-ready, non-empty, floor-satisfied, but some declared column is not
    /// observed. This tier resolves the old conjunctive-AND dead-end.
    AcceptTier,
    /// Empty / not parse-ready / an `AcceptTier` candidate below the char floor.
    Insufficient,
}

impl StructuredDataTier {
    /// Both `SchemaSatisfied` and `AcceptTier` count as accepted; only
    /// `Insufficient` blocks completion.
    pub(super) fn is_accepted(self) -> bool {
        !matches!(self, StructuredDataTier::Insufficient)
    }
}

/// Issue #921 (P4): the single OR-tolerant structured-data acceptance SSOT that
/// both production gates (the free-fn `structured_data_pass` / diagnostic side
/// and `task_contract::structured_record_excerpt_satisfies_obligations` /
/// completion side) route through.
///
/// Pure string predicate (DR4-003): no filesystem / network / process spawn, no
/// `unsafe`. `path` is used ONLY for extension dispatch in
/// `structured_data_parse_error` / `observed_data_columns`; it is never opened,
/// logged, or projected to a prompt/recovery sink (DR4-001). The caller must
/// only pass a `validated_obligation_path`-checked path or `None`.
///
/// Tier rules (DD1):
/// - parse error (only checked when `path` is `Some`) → `Insufficient`.
/// - empty trimmed excerpt → `Insufficient`.
/// - no declared columns + parse-ready non-empty → `SchemaSatisfied` (no floor).
/// - all declared columns observed → `SchemaSatisfied` (no floor; preserves the
///   old `structured_data_pass` accept set exactly / DR1-002).
/// - declared-but-missing columns + trimmed `chars().count() >= MIN_CHARS` →
///   `AcceptTier`; otherwise `Insufficient`.
pub(super) fn assess_structured_data(
    path: Option<&str>,
    excerpt: &str,
    required_columns: &[String],
) -> StructuredDataTier {
    // Parse-readiness is extension-driven, so it is only meaningful with a path
    // (DD1): for `path == None` the floor/empty checks below carry the load.
    if let Some(path) = path
        && structured_data_parse_error(path, excerpt).is_some()
    {
        return StructuredDataTier::Insufficient;
    }
    if excerpt.trim().is_empty() {
        return StructuredDataTier::Insufficient;
    }
    if required_columns.is_empty() {
        return StructuredDataTier::SchemaSatisfied;
    }
    let observed = observed_data_columns(path, excerpt, required_columns);
    let all_observed = required_columns
        .iter()
        .all(|column| observed.iter().any(|seen| seen == column));
    if all_observed {
        return StructuredDataTier::SchemaSatisfied;
    }
    // Issue #921 (P4 / CB-001): the AcceptTier fallback admits a parse-ready
    // artifact whose declared columns are not all observed. It MUST NOT promote
    // a format we cannot parse-check (e.g. binary/columnar `.parquet`, which
    // `observed_data_columns`/`structured_data_parse_error` do not special-case
    // and which falls through to generic substring matching). For such a format
    // a missing-column excerpt is arbitrary text with no parse-readiness
    // guarantee, so it stays `Insufficient`. Parse-checkable text formats
    // (csv/tsv/json/jsonl/ndjson) and the path-less completion case keep the
    // accept tier.
    if !accept_tier_format_is_parse_checkable(path) {
        return StructuredDataTier::Insufficient;
    }
    if excerpt.trim().chars().count() >= STRUCTURED_DATA_MIN_CHARS {
        StructuredDataTier::AcceptTier
    } else {
        StructuredDataTier::Insufficient
    }
}

/// Issue #921 (P4 / CB-001): whether the AcceptTier fallback may apply to this
/// path's format. `None` (the path-less completion case) and the parse-checkable
/// text formats are allowed; any other recognized extension (e.g. binary
/// `.parquet`) is not, because parse-readiness cannot be established from a text
/// excerpt. Mirrors the extension dispatch in [`observed_data_columns`] /
/// [`structured_data_parse_error`].
fn accept_tier_format_is_parse_checkable(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return true;
    };
    match std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None => true,
        Some("csv" | "tsv" | "json" | "jsonl" | "ndjson") => true,
        Some(_) => false,
    }
}

fn structured_data_pass(path: Option<&str>, excerpt: &str, required_columns: &[String]) -> bool {
    assess_structured_data(path, excerpt, required_columns).is_accepted()
}

/// Issue #922 (P5): minimum non-empty length (chars) for an open-ended research
/// report (no fixed `required_sections`) to clear the acceptance floor. Below
/// this a report is treated as trivially-empty and rejected. Heuristic value,
/// pinned by boundary tests; kept as a single SSOT const for easy tuning.
pub(super) const RESEARCH_MIN_REPORT_CHARS: usize = 80;

/// Issue #922 (P5): declarative, OR-tolerant acceptance tier for a research
/// report. Replaces the brittle `citation ∧ claim ∧ uncertainty` conjunction.
///
/// Internal and intentionally exhaustive (no `#[non_exhaustive]`) so adding a
/// tier is a compile error at every match site (OCP / fail-safe, mirroring the
/// `capability_for` enumeration discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResearchAcceptTier {
    /// All explicit `required_sections` are present (OR-tolerant: section
    /// coverage alone suffices; no extra uncertainty phrase is required).
    Sectioned,
    /// No fixed sections; the report clears the open-ended floor.
    OpenEnded,
    /// Neither path is satisfied (trivially-empty / too-thin report, or
    /// required sections absent).
    Insufficient,
}

impl ResearchAcceptTier {
    /// A report is accepted unless it is `Insufficient`. Single source of truth
    /// for "accepted?" — callers must not re-derive this elsewhere (DR1-002).
    pub(super) fn is_accepted(self) -> bool {
        !matches!(self, ResearchAcceptTier::Insufficient)
    }
}

/// Result of [`assess_research_report`]. The accept tier is the only field;
/// `accepted` is derived via [`ResearchAcceptTier::is_accepted`] (DR1-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResearchReportAssessment {
    pub(super) tier: ResearchAcceptTier,
}

/// Issue #922 (P5): the single SSOT predicate for research report acceptance.
///
/// All research acceptance / diagnostic call sites route through this so the
/// verdict is identical across the evidence and the two diagnostic paths
/// (DR1 / S1-003). This replaces the former `research_report_pass` 3-way AND
/// (`citation ∧ claim ∧ uncertainty`) which dead-ended sound reports that
/// merely omitted an explicit uncertainty phrase.
///
/// Pure string logic — no filesystem / network / process access (DR4-003).
/// `excerpt` / `required_sections` are read transiently; the masking and
/// sink-boundary defence live at the diagnostic-message and telemetry layers
/// (DR4-002), not in this predicate.
pub(super) fn assess_research_report(
    excerpt: &str,
    required_sections: &[String],
) -> ResearchReportAssessment {
    // Sectioned path: when the obligation carries explicit sections, those
    // drive acceptance. OR-tolerant = section coverage is sufficient; the old
    // uncertainty requirement is dropped. Docs-specific surface gating
    // (`docs_required_sections_pass`) is intentionally NOT applied to research
    // (S3-002 / DR3-002 double-gate fix).
    if !required_sections.is_empty() {
        let tier = if required_sections_present(excerpt, required_sections) {
            ResearchAcceptTier::Sectioned
        } else {
            ResearchAcceptTier::Insufficient
        };
        return ResearchReportAssessment { tier };
    }

    // Open-ended floor (no fixed sections): non-empty + minimum length + at
    // least one of {source-signal, claim-signal}. A trivially-empty or too-thin
    // report is rejected; a sound report is not dead-ended (S3-003 / S5-002).
    let trimmed = excerpt.trim();
    if trimmed.chars().count() < RESEARCH_MIN_REPORT_CHARS {
        return ResearchReportAssessment {
            tier: ResearchAcceptTier::Insufficient,
        };
    }
    let lower = excerpt.to_ascii_lowercase();
    let has_source = contains_any(
        &lower,
        &["http://", "https://", "source", "citation", "参考"],
    );
    let has_claim = contains_any(&lower, &["claim", "finding", "summary", "調査", "結論"]);
    let tier = if has_source || has_claim {
        ResearchAcceptTier::OpenEnded
    } else {
        ResearchAcceptTier::Insufficient
    };
    ResearchReportAssessment { tier }
}

/// Issue #923 (P6): SSOT for the Ops runbook EvidenceMissing diagnostic so the
/// artifact-diagnostic and obligation-diagnostic paths describe the tier rule
/// identically (no drift, S1-005 diagnostic parity).
const OPS_RUNBOOK_DIAGNOSTIC_MESSAGE: &str = "ops runbook is missing required sections (need a checklist + validation core and at least 3 of checklist/validation/rollback/risk)";

/// Issue #923 (P6): an Ops runbook section. SSOT for both excerpt-keyword
/// detection and request-label mapping (DRY, DSR1-001). `Checklist` and
/// `Validation` form the mandatory core; `Rollback` and `Risk` are optional
/// unless the request explicitly asks for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OpsSection {
    Checklist,
    Validation,
    Rollback,
    Risk,
}

impl OpsSection {
    /// All four sections in deterministic order.
    const ALL: [OpsSection; 4] = [
        OpsSection::Checklist,
        OpsSection::Validation,
        OpsSection::Rollback,
        OpsSection::Risk,
    ];

    /// Canonical label — the only string ever stored in
    /// `ArtifactObligation.required_sections` (DR4-002: never raw request text).
    pub(super) fn label(self) -> &'static str {
        match self {
            OpsSection::Checklist => "checklist",
            OpsSection::Validation => "validation",
            OpsSection::Rollback => "rollback",
            OpsSection::Risk => "risk",
        }
    }

    /// Bilingual excerpt keywords (SSOT, replaces the former inline lists). JP
    /// coverage preserved (手順 / 確認 / 検証 / 切り戻し / リスク / 注意).
    fn excerpt_keywords(self) -> &'static [&'static str] {
        match self {
            OpsSection::Checklist => &["[ ]", "[x]", "checklist", "手順"],
            OpsSection::Validation => &["validate", "validation", "verify", "確認", "検証"],
            OpsSection::Rollback => &["rollback", "roll back", "revert", "切り戻し"],
            OpsSection::Risk => &["risk", "impact", "注意", "リスク"],
        }
    }

    /// Map a request-derived section label (canonical or alias) onto a section.
    /// Unknown labels return `None` and are ignored (DR4-003 fail-safe).
    fn from_label(label: &str) -> Option<OpsSection> {
        match label.trim().to_ascii_lowercase().as_str() {
            "checklist" | "procedure" | "手順" => Some(OpsSection::Checklist),
            "validation" | "validate" | "verify" | "確認" | "検証" => {
                Some(OpsSection::Validation)
            }
            "rollback" | "restore" | "切り戻し" | "ロールバック" => {
                Some(OpsSection::Rollback)
            }
            "risk" | "impact" | "リスク" | "注意" => Some(OpsSection::Risk),
            _ => None,
        }
    }
}

/// Dependency-free set of Ops sections (DSR2-001 — no external crate). The fixed
/// 4-bit width bounds the input regardless of label-list length (DR4-003 DoS
/// boundary).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct OpsSectionSet {
    checklist: bool,
    validation: bool,
    rollback: bool,
    risk: bool,
}

impl OpsSectionSet {
    /// The mandatory core: a runbook missing either never accepts (core floor,
    /// Codex S3-001 — prevents a pure-OR predicate from passing empty content).
    const CORE: OpsSectionSet = OpsSectionSet {
        checklist: true,
        validation: true,
        rollback: false,
        risk: false,
    };

    fn with(mut self, section: OpsSection) -> Self {
        match section {
            OpsSection::Checklist => self.checklist = true,
            OpsSection::Validation => self.validation = true,
            OpsSection::Rollback => self.rollback = true,
            OpsSection::Risk => self.risk = true,
        }
        self
    }

    fn contains(self, section: OpsSection) -> bool {
        match section {
            OpsSection::Checklist => self.checklist,
            OpsSection::Validation => self.validation,
            OpsSection::Rollback => self.rollback,
            OpsSection::Risk => self.risk,
        }
    }

    fn count(self) -> usize {
        [self.checklist, self.validation, self.rollback, self.risk]
            .into_iter()
            .filter(|present| *present)
            .count()
    }

    fn is_superset_of(self, required: OpsSectionSet) -> bool {
        OpsSection::ALL
            .iter()
            .all(|section| !required.contains(*section) || self.contains(*section))
    }
}

/// Detect which Ops sections an excerpt covers (pure, linear, no regex —
/// DR4-003). Uses the `OpsSection` excerpt-keyword SSOT.
fn detect_present_sections(excerpt: &str) -> OpsSectionSet {
    let lower = excerpt.to_ascii_lowercase();
    let mut present = OpsSectionSet::default();
    for section in OpsSection::ALL {
        if contains_any(&lower, section.excerpt_keywords()) {
            present = present.with(section);
        }
    }
    present
}

/// Map request-derived required-section labels onto a set unioned with the
/// mandatory core. Unknown labels are ignored; the 4-bit width caps the result
/// at 4 sections regardless of input length (DR4-003).
fn ops_required_sections(required_sections: &[String]) -> OpsSectionSet {
    let mut required = OpsSectionSet::CORE;
    for label in required_sections {
        if let Some(section) = OpsSection::from_label(label) {
            required = required.with(section);
        }
    }
    required
}

/// OR-tolerant tier acceptance (Issue #923): the mandatory core must be present,
/// every explicitly-required section must be present, and at least 3 of the 4
/// sections must be covered. Replaces the historical strict 4-way AND.
fn accept_ops_tier(present: OpsSectionSet, required: OpsSectionSet) -> bool {
    present.is_superset_of(OpsSectionSet::CORE)
        && present.is_superset_of(required)
        && present.count() >= 3
}

/// Issue #923: tiered Ops runbook acceptance. `required_sections` carries the
/// request-derived mandatory sections — empty for the context-free, test-only
/// artifact paths, and populated from `obligation.required_sections` on the
/// production obligation path (DR3-001/DR3-003).
fn ops_runbook_pass(excerpt: &str, required_sections: &[String]) -> bool {
    accept_ops_tier(
        detect_present_sections(excerpt),
        ops_required_sections(required_sections),
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn observed_data_columns(
    path: Option<&str>,
    excerpt: &str,
    required_columns: &[String],
) -> Vec<String> {
    let normalized_ext = path
        .and_then(|path| std::path::Path::new(path).extension())
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let columns = match normalized_ext.as_deref() {
        Some("tsv") => delimited_header_columns(excerpt, '\t'),
        Some("csv") => delimited_header_columns(excerpt, ','),
        Some("json") => json_columns(excerpt),
        Some("jsonl") | Some("ndjson") => jsonl_columns(excerpt),
        _ => generic_data_columns(excerpt, required_columns),
    };
    sorted_unique(columns)
}

fn delimited_header_columns(excerpt: &str, delimiter: char) -> Vec<String> {
    excerpt
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            line.split(delimiter)
                .map(clean_data_column)
                .filter(|column| !column.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn json_columns(excerpt: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(excerpt) else {
        return Vec::new();
    };
    value_columns(&value)
}

fn jsonl_columns(excerpt: &str) -> Vec<String> {
    let mut columns = Vec::new();
    for line in excerpt.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        columns.extend(value_columns(&value));
    }
    columns
}

fn value_columns(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        serde_json::Value::Array(items) => items.iter().flat_map(value_columns).collect(),
        _ => Vec::new(),
    }
}

fn generic_data_columns(excerpt: &str, required_columns: &[String]) -> Vec<String> {
    if excerpt.trim().is_empty() {
        return Vec::new();
    }
    let mut columns = required_columns
        .iter()
        .filter(|column| excerpt.contains(column.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if columns.is_empty() && required_columns.is_empty() {
        columns.push("data".to_string());
    }
    columns
}

fn clean_data_column(raw: &str) -> String {
    raw.trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim()
        .to_string()
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #918 (P1): exhaustive capability spine coverage. Every method is
    // exercised here for all 5 kinds in the SAME commit that introduces them, so
    // `-D warnings` dead_code never fires and the non-coding invariant is pinned.
    #[test]
    fn capability_for_allows_process_exec_only_for_coding() {
        assert!(capability_for(TaskKind::Coding).allows_process_exec());
        for kind in [
            TaskKind::Docs,
            TaskKind::Data,
            TaskKind::Research,
            TaskKind::Ops,
            TaskKind::Authoring,
        ] {
            assert!(
                !capability_for(kind).allows_process_exec(),
                "non-coding kind {kind:?} must not allow process exec"
            );
        }
    }

    #[test]
    fn requires_executable_verifier_is_coding_only_and_respects_doc_suppression() {
        // Coding requires an executable verifier unless the task is a verifier-free
        // document task (DocsOnly/AnswerOnly), exactly reproducing the old
        // `coding_verifier_required = Coding && !verifier_free_document_task` gate.
        assert!(capability_for(TaskKind::Coding).requires_executable_verifier(false));
        assert!(!capability_for(TaskKind::Coding).requires_executable_verifier(true));
        // Non-coding kinds are always false, regardless of the doc-suppression flag
        // (§5.1: the request-text-derived test_execution_required flag must not be
        // able to lift a non-coding task into executable verification).
        for kind in [
            TaskKind::Docs,
            TaskKind::Data,
            TaskKind::Research,
            TaskKind::Ops,
            TaskKind::Authoring,
        ] {
            assert!(!capability_for(kind).requires_executable_verifier(false));
            assert!(!capability_for(kind).requires_executable_verifier(true));
        }
    }

    // `capability_for` / `allows_process_exec` are `const fn`: assert const-eval works.
    const _CODING_CAP: TaskCapability = capability_for(TaskKind::Coding);
    const _CODING_EXEC: bool = _CODING_CAP.allows_process_exec();
    // Issue #919: const-eval pin for the new Authoring kind.
    const _AUTHORING_CAP: TaskCapability = capability_for(TaskKind::Authoring);
    const _AUTHORING_EXEC: bool = _AUTHORING_CAP.allows_process_exec();

    #[test]
    fn docs_required_sections_pass_becomes_completion_evidence() {
        let verifier = DocsVerifier;
        let evidence = verifier
            .required_sections_evidence(
                Some("README.md".to_string()),
                "## Setup\nInstall with cargo.\n## Usage\nRun an example.\n",
            )
            .expect("required sections pass");
        assert_eq!(
            evidence,
            CompletionEvidence::RequiredSectionsPass {
                path: Some("README.md".to_string())
            }
        );
    }

    #[test]
    fn docs_verifier_artifact_adapter_emits_required_sections_evidence() {
        let verifier = DocsVerifier;
        let artifact = VerifierArtifact {
            path: Some("README.md"),
            excerpt: "## Setup\nInstall it.\n## Usage\nRun it.\n",
            required_columns: &[],
            required_sections: &[],
        };

        assert_eq!(
            verifier.artifact_evidence(artifact),
            Some(CompletionEvidence::RequiredSectionsPass {
                path: Some("README.md".to_string()),
            })
        );
    }

    #[test]
    fn docs_verifier_pass_evidence_is_docs_only_not_build_test() {
        let verifier = DocsVerifier;

        assert_eq!(
            verifier.pass_evidence("docs evidence", None),
            CompletionEvidence::RequiredSectionsPass { path: None }
        );
    }

    #[test]
    fn verifier_failure_packet_is_task_kind_independent() {
        let verifier = DocsVerifier;
        let packet = verifier.required_sections_failure_packet("README.md", "only title");
        let json = packet.to_json_value();
        assert!(json.get("task_kind").is_none());
        assert_eq!(json["failure_kind"], "required_sections_missing");
        assert_eq!(json["candidate_artifacts"][0]["role"], "usage_docs");
    }

    #[test]
    fn coding_verifier_diagnoses_malformed_package_manifest() {
        let verifier = CodingVerifier;
        let diagnostic = verifier
            .diagnostic(VerifierArtifact {
                path: Some("package.json"),
                excerpt: r#"{"scripts":"#,
                required_columns: &[],
                required_sections: &[],
            })
            .expect("malformed manifest diagnostic");

        assert_eq!(diagnostic.code, VerifierDiagnosticCode::InvalidManifest);
        assert_eq!(diagnostic.role, ArtifactRole::Setup);

        let packet = diagnostic.to_failure_packet("npm test", "package parse failed");
        let json = packet.to_json_value();
        assert_eq!(json["failure_kind"], "invalid_manifest");
        assert_eq!(json["diagnostic_code"], "invalid_manifest");
        assert_eq!(json["candidate_artifacts"][0]["role"], "setup");
    }

    #[test]
    fn coding_verifier_diagnoses_bad_test_artifact() {
        let verifier = CodingVerifier;
        let diagnostic = verifier
            .diagnostic(VerifierArtifact {
                path: Some("tests/cli.rs"),
                excerpt: "fn helper() {}",
                required_columns: &[],
                required_sections: &[],
            })
            .expect("bad test diagnostic");

        assert_eq!(diagnostic.code, VerifierDiagnosticCode::BadTest);
        assert_eq!(diagnostic.role, ArtifactRole::Test);
    }

    #[test]
    fn data_verifier_csv_columns_become_structured_data_evidence() {
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];
        let evidence = verifier
            .structured_data_evidence(
                Some("output.csv".to_string()),
                "Category,Total\nA,1\n",
                &required,
            )
            .expect("schema pass");

        assert_eq!(
            evidence,
            CompletionEvidence::StructuredDataPass {
                path: Some("output.csv".to_string()),
                columns: required,
            }
        );
    }

    #[test]
    fn data_verifier_accepts_missing_required_column_when_parse_ready() {
        // Issue #921 (P4): the old conjunctive AND rejected a parse-ready CSV
        // whose header was missing a declared column. The OR-tolerant SSOT now
        // accepts it via `AcceptTier` (parse-ready, non-empty, above the char
        // floor) so legitimate structured output no longer dead-ends. The
        // surviving rejection paths (empty / unparseable / below-floor) are
        // pinned by `data_verifier_rejects_unparseable_or_empty_structured_data`.
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];

        // 19 trimmed chars (>= STRUCTURED_DATA_MIN_CHARS) → AcceptTier → Some.
        assert!(
            verifier
                .structured_data_evidence(
                    Some("output.csv".to_string()),
                    "Category,Amount\nA,1\n",
                    &required,
                )
                .is_some(),
            "parse-ready CSV missing a declared column must accept-tier complete"
        );
    }

    #[test]
    fn data_verifier_rejects_unparseable_or_empty_structured_data() {
        // Issue #921 (P4 / DD3): parse-error and empty stay `Insufficient` even
        // under the OR-tolerant SSOT — a truly broken/empty artifact is not done.
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];

        // Malformed JSON → parse error → Insufficient.
        assert_eq!(
            verifier.structured_data_evidence(
                Some("output.json".to_string()),
                r#"{"Category":"#,
                &required,
            ),
            None
        );
        // Empty excerpt → Insufficient.
        assert_eq!(
            verifier.structured_data_evidence(Some("output.csv".to_string()), "   \n", &required,),
            None
        );
    }

    // Issue #921 (P4): tier-boundary unit tests for the OR-tolerant SSOT.
    #[test]
    fn assess_structured_data_no_columns_accepts_nonempty() {
        assert_eq!(
            assess_structured_data(Some("output.csv"), "a,b\n1,2\n", &[]),
            StructuredDataTier::SchemaSatisfied
        );
        assert_eq!(
            assess_structured_data(None, "anything", &[]),
            StructuredDataTier::SchemaSatisfied
        );
    }

    #[test]
    fn assess_structured_data_empty_is_insufficient() {
        assert_eq!(
            assess_structured_data(Some("output.csv"), "   \n\t", &[]),
            StructuredDataTier::Insufficient
        );
        let cols = vec!["a".to_string()];
        assert_eq!(
            assess_structured_data(Some("output.csv"), "", &cols),
            StructuredDataTier::Insufficient
        );
    }

    #[test]
    fn assess_structured_data_all_columns_observed_is_schema_satisfied_no_floor() {
        let cols = vec!["a".to_string(), "b".to_string()];
        // "a,b\n1" trimmed = 5 chars < floor, but all declared columns observed →
        // SchemaSatisfied (NO floor / DR1-002 regression-free).
        assert_eq!(
            assess_structured_data(Some("output.csv"), "a,b\n1", &cols),
            StructuredDataTier::SchemaSatisfied
        );
    }

    #[test]
    fn assess_structured_data_missing_column_floor_boundary() {
        let cols = vec!["zzz".to_string()];
        // 15 chars, missing column → below floor → Insufficient.
        let fifteen = "abcd,efgh\n12345"; // 15 chars
        assert_eq!(fifteen.trim().chars().count(), 15);
        assert_eq!(
            assess_structured_data(Some("output.csv"), fifteen, &cols),
            StructuredDataTier::Insufficient
        );
        // 16 chars, missing column → at floor → AcceptTier.
        let sixteen = "abcd,efgh\n123456"; // 16 chars
        assert_eq!(sixteen.trim().chars().count(), 16);
        assert_eq!(
            assess_structured_data(Some("output.csv"), sixteen, &cols),
            StructuredDataTier::AcceptTier
        );
        // 17 chars, missing column → above floor → AcceptTier.
        let seventeen = "abcd,efgh\n1234567"; // 17 chars
        assert_eq!(seventeen.trim().chars().count(), 17);
        assert_eq!(
            assess_structured_data(Some("output.csv"), seventeen, &cols),
            StructuredDataTier::AcceptTier
        );
    }

    #[test]
    fn assess_structured_data_parse_error_is_insufficient() {
        let cols = vec!["category".to_string()];
        // Malformed JSON (long enough to clear the floor) is still Insufficient
        // because parse-error is never promoted to AcceptTier (DD3).
        assert_eq!(
            assess_structured_data(Some("output.json"), r#"{"category": broken broken"#, &cols),
            StructuredDataTier::Insufficient
        );
        // CSV with no parse-ready header → Insufficient.
        assert_eq!(
            assess_structured_data(Some("output.csv"), "\n\n   \n", &cols),
            StructuredDataTier::Insufficient
        );
    }

    #[test]
    fn assess_structured_data_path_none_skips_parse_error_check() {
        let cols = vec!["category".to_string()];
        // With path=None, the parse-error check is skipped (no extension to
        // dispatch on); the floor/empty path carries the decision. A long blob
        // that does NOT contain the declared column (so `generic_data_columns`
        // observes nothing for it) accept-tiers above the floor.
        let excerpt = "totally unrelated long content here";
        assert!(excerpt.trim().chars().count() >= STRUCTURED_DATA_MIN_CHARS);
        assert_eq!(
            assess_structured_data(None, excerpt, &cols),
            StructuredDataTier::AcceptTier
        );
        // Below floor with no observed declared column → Insufficient.
        assert_eq!(
            assess_structured_data(None, "short", &cols),
            StructuredDataTier::Insufficient
        );
    }

    #[test]
    fn assess_structured_data_unparseable_format_does_not_accept_tier() {
        // Issue #921 (CB-001): `.parquet` is admitted as a DataOutput path but is
        // a binary format the SSOT cannot parse-check (no special-case in
        // `observed_data_columns`/`structured_data_parse_error`). A long excerpt
        // whose declared columns are NOT observed must stay `Insufficient` — it
        // must NOT be promoted to AcceptTier on arbitrary text.
        let cols = vec!["zzz".to_string()];
        let long_blob = "this is a long text blob well over the char floor"; // > MIN_CHARS
        assert!(long_blob.trim().chars().count() >= STRUCTURED_DATA_MIN_CHARS);
        assert_eq!(
            assess_structured_data(Some("output.parquet"), long_blob, &cols),
            StructuredDataTier::Insufficient,
            "unparseable parquet with missing columns must not accept-tier arbitrary text"
        );
        // Same content/columns on a parse-checkable CSV path DOES accept-tier
        // (regression guard that the CB-001 gate is format-scoped, not blanket).
        assert_eq!(
            assess_structured_data(Some("output.csv"), long_blob, &cols),
            StructuredDataTier::AcceptTier
        );
        // No declared columns: parquet still accepted as non-empty (no schema to
        // verify) — the gate only governs the declared-but-missing AcceptTier path.
        assert_eq!(
            assess_structured_data(Some("output.parquet"), long_blob, &[]),
            StructuredDataTier::SchemaSatisfied
        );
    }

    #[test]
    fn data_verifier_diagnoses_malformed_json_before_schema_pass() {
        let verifier = DataVerifier;
        let required = vec!["category".to_string(), "total".to_string()];
        let diagnostic = verifier
            .diagnostic(VerifierArtifact {
                path: Some("output.json"),
                excerpt: r#"{"category":"#,
                required_columns: &required,
                required_sections: &[],
            })
            .expect("schema diagnostic");

        assert_eq!(diagnostic.code, VerifierDiagnosticCode::SchemaMismatch);
        assert_eq!(diagnostic.role, ArtifactRole::DataOutput);
    }

    #[test]
    fn data_verifier_does_not_diagnose_missing_column_when_parse_ready() {
        // Issue #921 (P4 / DD2 / DR1-004): a parse-ready CSV that is missing a
        // declared column is now `AcceptTier`, so the diagnostic side must
        // return strictly `None` (NOT a non-blocking `EvidenceMissing`). A
        // lingering diagnostic would re-block the accept tier via the
        // `artifact_identity_satisfied_for_verification` whitelist.
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];
        assert_eq!(
            verifier.diagnostic(VerifierArtifact {
                path: Some("output.csv"),
                excerpt: "Category,Amount\nA,1\n",
                required_columns: &required,
                required_sections: &[],
            }),
            None,
            "accept-tier artifact must yield no blocking diagnostic"
        );
    }

    #[test]
    fn data_verifier_diagnoses_below_floor_structured_data() {
        // Issue #921 (P4): a missing-column excerpt that is also below the char
        // floor stays `Insufficient` → blocking SchemaMismatch diagnostic.
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];
        // "a,b\n1" trimmed = 5 chars < STRUCTURED_DATA_MIN_CHARS, no declared
        // column observed → Insufficient.
        let diagnostic = verifier
            .diagnostic(VerifierArtifact {
                path: Some("output.csv"),
                excerpt: "a,b\n1",
                required_columns: &required,
                required_sections: &[],
            })
            .expect("below-floor diagnostic");
        assert_eq!(diagnostic.code, VerifierDiagnosticCode::SchemaMismatch);
        assert_eq!(diagnostic.role, ArtifactRole::DataOutput);
    }

    #[test]
    fn data_verifier_jsonl_columns_become_structured_data_evidence() {
        let verifier = DataVerifier;
        let required = vec!["category".to_string(), "total".to_string()];

        assert_eq!(
            verifier.artifact_evidence(VerifierArtifact {
                path: Some("output.jsonl"),
                excerpt: "{\"category\":\"A\",\"total\":1}\n{\"category\":\"B\",\"total\":2}\n",
                required_columns: &required,
                required_sections: &[],
            }),
            Some(CompletionEvidence::StructuredDataPass {
                path: Some("output.jsonl".to_string()),
                columns: required,
            })
        );
    }

    #[test]
    fn data_verifier_failure_packet_targets_data_output() {
        let verifier = DataVerifier;
        let packet = verifier.structured_data_failure_packet("output.csv", "Category\nA\n");
        let json = packet.to_json_value();
        assert_eq!(json["failure_kind"], "structured_data_schema_missing");
        assert_eq!(json["candidate_artifacts"][0]["role"], "data_output");
    }

    #[test]
    fn coding_verifier_pass_evidence_is_build_test() {
        let verifier = CodingVerifier;
        assert_eq!(verifier.task_kind(), TaskKind::Coding);
        assert_eq!(
            verifier.pass_evidence("cargo test", Some(1)),
            CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::BuildTest,
                command: "cargo test".to_string(),
                bound_test_artifacts_count: Some(1),
            }
        );
    }

    #[test]
    fn verifier_registry_selects_adapter_for_each_task_kind() {
        let cases = [
            TaskKind::Coding,
            TaskKind::Docs,
            TaskKind::Data,
            TaskKind::Research,
            TaskKind::Ops,
            TaskKind::Authoring,
        ];

        for task_kind in cases {
            assert_eq!(verifier_for_task_kind(task_kind).task_kind(), task_kind);
        }
    }

    #[test]
    fn data_verifier_pass_evidence_is_structured_data_not_build_test() {
        let verifier = DataVerifier;

        assert_eq!(
            verifier.pass_evidence("validate output.csv", None),
            CompletionEvidence::StructuredDataPass {
                path: None,
                columns: Vec::new(),
            }
        );
    }

    #[test]
    fn research_verifier_artifact_adapter_emits_report_evidence() {
        let verifier = ResearchVerifier;
        let evidence = verifier.artifact_evidence(VerifierArtifact {
            path: Some("research.md"),
            excerpt: "## Summary\nFinding: release cadence changed.\nSource: https://example.test/report\nLimitation: confidence is medium.\n",
            required_columns: &[],
            required_sections: &[],
        });

        assert_eq!(
            evidence,
            Some(CompletionEvidence::ReportCompletenessPass {
                path: Some("research.md".to_string()),
            })
        );
    }

    #[test]
    fn ops_verifier_artifact_adapter_emits_report_evidence() {
        let verifier = OpsVerifier;
        let evidence = verifier.artifact_evidence(VerifierArtifact {
            path: Some("runbook.md"),
            excerpt: "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Rollback\nRevert the deploy.\n## Risk\nImpact is low.\n",
            required_columns: &[],
            required_sections: &[],
        });

        assert_eq!(
            evidence,
            Some(CompletionEvidence::ReportCompletenessPass {
                path: Some("runbook.md".to_string()),
            })
        );
    }

    // ---- Issue #922 (P5): assess_research_report SSOT predicate ----------

    #[test]
    fn issue922_assess_research_report_sectioned_or_tolerant() {
        let sections = vec!["findings".to_string(), "sources".to_string()];
        // OR-tolerant: section coverage suffices (no uncertainty phrase).
        let covered = "## Findings\nrelease cadence changed.\n## Sources\nhttps://example.test\n";
        assert_eq!(
            assess_research_report(covered, &sections).tier,
            ResearchAcceptTier::Sectioned
        );
        // Issue #919 softened `required_sections_present` to M-of-N, so 1-of-2
        // sections now passes; NO section present -> Insufficient.
        let none_present = "A plain paragraph with neither named heading present here.";
        assert_eq!(
            assess_research_report(none_present, &sections).tier,
            ResearchAcceptTier::Insufficient
        );
    }

    #[test]
    fn issue922_assess_research_report_open_ended_floor_boundary() {
        let no_sections: &[String] = &[];
        // Below the floor -> Insufficient even with a claim signal present
        // (length is checked before signal coverage).
        let prefix = "finding ";
        let below = format!(
            "{prefix}{}",
            "a".repeat(RESEARCH_MIN_REPORT_CHARS - prefix.len() - 1)
        );
        assert!(below.chars().count() < RESEARCH_MIN_REPORT_CHARS);
        assert_eq!(
            assess_research_report(&below, no_sections).tier,
            ResearchAcceptTier::Insufficient
        );
        // At/above the floor with a claim signal -> OpenEnded.
        let at_floor = format!("{prefix}{}", "a".repeat(RESEARCH_MIN_REPORT_CHARS));
        assert!(at_floor.chars().count() >= RESEARCH_MIN_REPORT_CHARS);
        assert_eq!(
            assess_research_report(&at_floor, no_sections).tier,
            ResearchAcceptTier::OpenEnded
        );
    }

    #[test]
    fn issue922_assess_research_report_open_ended_signal_or_tolerance() {
        let no_sections: &[String] = &[];
        let filler = "x".repeat(RESEARCH_MIN_REPORT_CHARS);
        // claim signal only.
        let claim_only = format!("This finding describes the outcome. {filler}");
        assert_eq!(
            assess_research_report(&claim_only, no_sections).tier,
            ResearchAcceptTier::OpenEnded
        );
        // source signal only.
        let source_only = format!("See https://example.test/data {filler}");
        assert_eq!(
            assess_research_report(&source_only, no_sections).tier,
            ResearchAcceptTier::OpenEnded
        );
        // neither claim nor source -> Insufficient despite clearing the length.
        let neither = format!("General background and context overview. {filler}");
        assert_eq!(
            assess_research_report(&neither, no_sections).tier,
            ResearchAcceptTier::Insufficient
        );
    }

    #[test]
    fn issue922_report_with_citation_and_claim_but_no_uncertainty_now_accepted() {
        // Regression of the old `research_report_pass` dead-end: this sound
        // report has a citation + a claim but NO uncertainty phrase, which the
        // former 3-way AND rejected. It must now be accepted.
        let no_sections: &[String] = &[];
        let report =
            "Finding: latency dropped 30%. Source: https://example.test/benchmark results page.";
        assert!(report.chars().count() >= RESEARCH_MIN_REPORT_CHARS);
        assert_eq!(
            assess_research_report(report, no_sections).tier,
            ResearchAcceptTier::OpenEnded
        );
    }

    #[test]
    fn issue922_empty_report_is_insufficient() {
        let no_sections: &[String] = &[];
        assert_eq!(
            assess_research_report("", no_sections).tier,
            ResearchAcceptTier::Insufficient
        );
        assert_eq!(
            assess_research_report("   \n  ", no_sections).tier,
            ResearchAcceptTier::Insufficient
        );
    }

    // ---- Issue #923 (P6): Ops tier predicate ----

    #[test]
    fn ops_section_labels_are_canonical() {
        assert_eq!(OpsSection::Checklist.label(), "checklist");
        assert_eq!(OpsSection::Validation.label(), "validation");
        assert_eq!(OpsSection::Rollback.label(), "rollback");
        assert_eq!(OpsSection::Risk.label(), "risk");
    }

    #[test]
    fn ops_section_from_label_maps_aliases_and_ignores_unknown() {
        assert_eq!(
            OpsSection::from_label("procedure"),
            Some(OpsSection::Checklist)
        );
        assert_eq!(
            OpsSection::from_label("CHECKLIST"),
            Some(OpsSection::Checklist)
        );
        assert_eq!(
            OpsSection::from_label("verify"),
            Some(OpsSection::Validation)
        );
        assert_eq!(
            OpsSection::from_label("restore"),
            Some(OpsSection::Rollback)
        );
        assert_eq!(OpsSection::from_label("impact"), Some(OpsSection::Risk));
        assert_eq!(OpsSection::from_label("リスク"), Some(OpsSection::Risk));
        assert_eq!(OpsSection::from_label("totally-unknown"), None);
    }

    #[test]
    fn ops_tier_accepts_three_of_four_with_core_present() {
        // rollback omitted, core (checklist+validation) present, 3/4 → pass.
        let excerpt = "## Checklist\n[x] deploy\n## Validation\nVerify health endpoint.\n## Risk\nImpact low.";
        assert!(ops_runbook_pass(excerpt, &[]));
    }

    #[test]
    fn ops_tier_rejects_missing_core() {
        // checklist + risk only: validation core absent → fail (core floor).
        let excerpt = "## Checklist\n[ ] deploy\n## Risk\nlow";
        assert!(!ops_runbook_pass(excerpt, &[]));
    }

    #[test]
    fn ops_tier_rejects_pure_or_single_section() {
        // A single section can never satisfy the core floor + >=3 threshold.
        assert!(!ops_runbook_pass("## Checklist\n[x] deploy", &[]));
        assert!(!ops_runbook_pass("Some prose with rollback mentioned", &[]));
    }

    #[test]
    fn ops_tier_explicit_required_section_must_be_present() {
        // Request explicitly required rollback: a 3/4 runbook that omits
        // rollback must FAIL even though it would otherwise pass.
        let rollback_omitted =
            "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Risk\nImpact low.";
        let required = vec!["rollback".to_string()];
        assert!(!ops_runbook_pass(rollback_omitted, &required));

        // Same explicit requirement, but rollback present and risk omitted → pass.
        let rollback_present =
            "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Rollback\nRevert deploy.";
        assert!(ops_runbook_pass(rollback_present, &required));
    }

    #[test]
    fn ops_tier_bilingual_japanese_runbook_passes() {
        // 手順 / 検証 / リスク present (切り戻し omitted), 3/4 → pass.
        let excerpt = "## 手順\n[x] デプロイ\n## 検証\nヘルスチェック確認。\n## リスク\n影響は小。";
        assert!(ops_runbook_pass(excerpt, &[]));
    }

    #[test]
    fn ops_tier_unknown_required_labels_are_ignored() {
        // Unknown labels do not raise the bar (DR4-003 fail-safe): a normal 3/4
        // runbook still passes.
        let excerpt =
            "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Risk\nImpact low.";
        let required = vec!["totally-unknown".to_string(), "checklist".to_string()];
        assert!(ops_runbook_pass(excerpt, &required));
    }

    #[test]
    fn ops_detect_present_sections_counts_correctly() {
        let excerpt = "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Rollback\nRevert.\n## Risk\nlow.";
        let present = detect_present_sections(excerpt);
        assert_eq!(present.count(), 4);
        assert!(present.contains(OpsSection::Rollback));
        assert!(present.is_superset_of(OpsSectionSet::CORE));
    }

    // ----- Issue #919: accept-tier predicate (Decision #4) -----

    #[test]
    fn authoring_accept_tier_rejects_stub() {
        let diag = authoring_accept_tier_diagnostic(TaskKind::Authoring, "README.md", "TODO", &[]);
        let diag = diag.expect("stub must produce a diagnostic");
        assert_eq!(diag.code, VerifierDiagnosticCode::EvidenceMissing);
        assert_eq!(diag.role, ArtifactRole::UsageDocs);
    }

    #[test]
    fn authoring_accept_tier_accepts_one_sentence() {
        // >= 24 chars but < 64 -- guards against an over-high floor (OR-4).
        let excerpt = "This document describes setup.";
        assert!(excerpt.chars().count() >= AUTHORING_MIN_CONTENT_CHARS);
        assert!(excerpt.chars().count() < 64);
        assert!(
            authoring_accept_tier_diagnostic(TaskKind::Authoring, "README.md", excerpt, &[])
                .is_none()
        );
    }

    #[test]
    fn authoring_accept_tier_requested_sections_or_tolerant() {
        // 1-of-2 named sections present -> accepted via the shared softened
        // `required_sections_present` (M-of-N), NOT a re-implemented inline gate.
        let sections = vec!["overview".to_string(), "details".to_string()];
        let excerpt = "## Overview\nThis is a sufficiently long overview paragraph.\n";
        assert!(
            authoring_accept_tier_diagnostic(TaskKind::Authoring, "README.md", excerpt, &sections)
                .is_none()
        );
    }

    #[test]
    fn authoring_obligation_diagnostic_uses_accept_tier() {
        // DR3-003: the obligation-parts path runs the same accept tier. A stub
        // excerpt at an existing path yields the static authoring diagnostic.
        let obligation = ArtifactObligation::file(ArtifactRole::UsageDocs, "README.md".to_string());
        let diag = verifier_diagnostic_for_obligation_parts(
            TaskKind::Authoring,
            &obligation,
            Some("TODO"),
            true,
        );
        let diag = diag.expect("stub obligation must produce a diagnostic");
        assert_eq!(diag.code, VerifierDiagnosticCode::EvidenceMissing);
        // A real paragraph passes the accept tier (no diagnostic).
        assert!(
            verifier_diagnostic_for_obligation_parts(
                TaskKind::Authoring,
                &obligation,
                Some("This document describes the setup and usage of the project clearly."),
                true,
            )
            .is_none()
        );
    }

    // ----- Issue #919: M-of-N required_sections_present (Decision #6) -----

    #[test]
    fn required_sections_present_empty_is_true() {
        assert!(required_sections_present("anything", &[]));
    }

    #[test]
    fn required_sections_present_majority_passes() {
        let two = vec!["overview".to_string(), "details".to_string()];
        // 1-of-2 present now passes (was all-or-nothing).
        assert!(required_sections_present("## Overview\nstuff\n", &two));
        // 0-of-2 fails.
        assert!(!required_sections_present("nothing relevant here", &two));
        // N=1 still requires that section (unchanged from `.all()`).
        let one = vec!["usage".to_string()];
        assert!(required_sections_present("## Usage\nrun it\n", &one));
        assert!(!required_sections_present("no section", &one));
    }
}
