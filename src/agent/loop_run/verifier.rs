use super::completion_evidence::CompletionEvidence;
use super::failure_packet::{CandidateArtifact, FailurePacket};
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableFormat, DeliverableSchema, TaskKind,
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

static CODING_VERIFIER: CodingVerifier = CodingVerifier;
static DOCS_VERIFIER: DocsVerifier = DocsVerifier;
static DATA_VERIFIER: DataVerifier = DataVerifier;
static RESEARCH_VERIFIER: ResearchVerifier = ResearchVerifier;
static OPS_VERIFIER: OpsVerifier = OpsVerifier;

#[allow(dead_code)]
pub(super) fn verifier_for_task_kind(task_kind: TaskKind) -> &'static dyn Verifier {
    match task_kind {
        TaskKind::Coding => &CODING_VERIFIER,
        TaskKind::Docs => &DOCS_VERIFIER,
        TaskKind::Data => &DATA_VERIFIER,
        TaskKind::Research => &RESEARCH_VERIFIER,
        TaskKind::Ops => &OPS_VERIFIER,
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
        TaskKind::Docs | TaskKind::Data | TaskKind::Research | TaskKind::Ops => TaskCapability {
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
            TaskKind::Docs | TaskKind::Data | TaskKind::Research | TaskKind::Ops => false,
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
        self.research_report_evidence(artifact.path.map(str::to_string), artifact.excerpt)
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
    ) -> Option<CompletionEvidence> {
        if research_report_pass(excerpt) {
            Some(CompletionEvidence::ReportCompletenessPass { path })
        } else {
            None
        }
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
        if ops_runbook_pass(excerpt) {
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
        TaskKind::Research => (!research_report_pass(artifact.excerpt)).then(|| {
            VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                ArtifactRole::UsageDocs,
                Some(path),
                "research evidence is missing claim/source/limitation coverage",
            )
        }),
        TaskKind::Ops => (!ops_runbook_pass(artifact.excerpt)).then(|| {
            VerifierDiagnostic::new(
                task_kind,
                VerifierDiagnosticCode::EvidenceMissing,
                ArtifactRole::UsageDocs,
                Some(path),
                "ops evidence is missing checklist/validation/rollback/risk coverage",
            )
        }),
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
    if let Some(DeliverableSchema::RequiredSections(sections)) = obligation.schema.as_ref()
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
    if task_kind == TaskKind::Research
        && let Some(excerpt) = excerpt
        && !research_report_pass(excerpt)
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            obligation.role,
            Some(&obligation.path),
            "research evidence is missing claim/source/limitation coverage",
        ));
    }
    if task_kind == TaskKind::Ops
        && let Some(excerpt) = excerpt
        && !ops_runbook_pass(excerpt)
    {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::EvidenceMissing,
            obligation.role,
            Some(&obligation.path),
            "ops evidence is missing checklist/validation/rollback/risk coverage",
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
    if let Some(message) = structured_data_parse_error(path, excerpt) {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::SchemaMismatch,
            ArtifactRole::DataOutput,
            Some(path),
            message,
        ));
    }
    if !structured_data_pass(Some(path), excerpt, required_columns) {
        return Some(VerifierDiagnostic::new(
            task_kind,
            VerifierDiagnosticCode::SchemaMismatch,
            ArtifactRole::DataOutput,
            Some(path),
            "structured data evidence is missing required columns or parse-ready records",
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

fn required_sections_present(excerpt: &str, sections: &[String]) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    sections.iter().all(|section| {
        let normalized = section.to_ascii_lowercase();
        lower.contains(&format!("## {normalized}"))
            || lower.contains(&format!("# {normalized}"))
            || lower.contains(&normalized)
    })
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
        TaskKind::Docs | TaskKind::Research | TaskKind::Ops => ArtifactRole::UsageDocs,
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

fn structured_data_pass(path: Option<&str>, excerpt: &str, required_columns: &[String]) -> bool {
    let observed = observed_data_columns(path, excerpt, required_columns);
    if observed.is_empty() {
        return false;
    }
    required_columns
        .iter()
        .all(|column| observed.iter().any(|observed| observed == column))
}

fn research_report_pass(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let has_citation = contains_any(
        &lower,
        &["http://", "https://", "source", "citation", "参考"],
    );
    let has_claim = contains_any(&lower, &["claim", "finding", "summary", "調査", "結論"]);
    let has_uncertainty = contains_any(
        &lower,
        &[
            "uncertain",
            "unknown",
            "limitation",
            "confidence",
            "不明",
            "制約",
        ],
    );
    has_citation && has_claim && has_uncertainty
}

fn ops_runbook_pass(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let has_checklist = contains_any(&lower, &["[ ]", "[x]", "checklist", "手順"]);
    let has_validation = contains_any(
        &lower,
        &["validate", "validation", "verify", "確認", "検証"],
    );
    let has_rollback = contains_any(&lower, &["rollback", "roll back", "revert", "切り戻し"]);
    let has_risk = contains_any(&lower, &["risk", "impact", "注意", "リスク"]);
    has_checklist && has_validation && has_rollback && has_risk
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
        ] {
            assert!(!capability_for(kind).requires_executable_verifier(false));
            assert!(!capability_for(kind).requires_executable_verifier(true));
        }
    }

    // `capability_for` / `allows_process_exec` are `const fn`: assert const-eval works.
    const _CODING_CAP: TaskCapability = capability_for(TaskKind::Coding);
    const _CODING_EXEC: bool = _CODING_CAP.allows_process_exec();

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
    fn data_verifier_rejects_missing_required_column() {
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];

        assert_eq!(
            verifier.structured_data_evidence(
                Some("output.csv".to_string()),
                "Category,Amount\nA,1\n",
                &required,
            ),
            None
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
            })
            .expect("schema diagnostic");

        assert_eq!(diagnostic.code, VerifierDiagnosticCode::SchemaMismatch);
        assert_eq!(diagnostic.role, ArtifactRole::DataOutput);
    }

    #[test]
    fn data_verifier_diagnoses_missing_required_column() {
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];
        let diagnostic = verifier
            .diagnostic(VerifierArtifact {
                path: Some("output.csv"),
                excerpt: "Category,Amount\nA,1\n",
                required_columns: &required,
            })
            .expect("missing column diagnostic");

        assert_eq!(diagnostic.code, VerifierDiagnosticCode::SchemaMismatch);
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
        });

        assert_eq!(
            evidence,
            Some(CompletionEvidence::ReportCompletenessPass {
                path: Some("runbook.md".to_string()),
            })
        );
    }
}
