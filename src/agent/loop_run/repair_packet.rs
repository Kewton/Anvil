use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, DeliverableSchema, RecoveryTargetHint,
    TaskContract, TaskKind,
};

const MAX_EXPECTED_EVIDENCE_ITEMS: usize = 8;
const MAX_EXPECTED_EVIDENCE_CHARS: usize = 160;
const MAX_REPAIR_INSTRUCTION_CHARS: usize = 360;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CorrectionKind {
    Patch,
    #[cfg(test)]
    TestCorrection,
    #[cfg(test)]
    ManifestCorrection,
    SectionAddition,
    SchemaCorrection,
    CitationSupport,
    ChecklistCompletion,
}

impl CorrectionKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Patch => "patch",
            #[cfg(test)]
            Self::TestCorrection => "test_correction",
            #[cfg(test)]
            Self::ManifestCorrection => "manifest_correction",
            Self::SectionAddition => "section_addition",
            Self::SchemaCorrection => "schema_correction",
            Self::CitationSupport => "citation_support",
            Self::ChecklistCompletion => "checklist_completion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliverableFailureDomain {
    MissingDeliverable,
    MalformedDeliverable,
    VerifierFailed,
    #[cfg(test)]
    GeneratedTestBug,
    #[cfg(test)]
    InvalidManifest,
    SchemaMismatch,
    IncompleteSections,
    UnsafeOrOutOfScope,
}

impl DeliverableFailureDomain {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingDeliverable => "missing_deliverable",
            Self::MalformedDeliverable => "malformed_deliverable",
            Self::VerifierFailed => "verifier_failed",
            #[cfg(test)]
            Self::GeneratedTestBug => "generated_test_bug",
            #[cfg(test)]
            Self::InvalidManifest => "invalid_manifest",
            Self::SchemaMismatch => "schema_mismatch",
            Self::IncompleteSections => "incomplete_sections",
            Self::UnsafeOrOutOfScope => "unsafe_or_out_of_scope",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairObligationTarget {
    pub(super) obligation_id: String,
    pub(super) role: ArtifactRole,
    pub(super) kind: DeliverableKind,
    pub(super) path: Option<String>,
    pub(super) expected_evidence: Vec<String>,
    pub(super) failure_domain: DeliverableFailureDomain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairPacket {
    pub(super) target: RepairObligationTarget,
    pub(super) correction_kind: CorrectionKind,
    pub(super) instruction: String,
}

pub(super) type CorrectionPacket = RepairPacket;

impl RepairPacket {
    pub(super) fn for_obligation(
        contract: &TaskContract,
        obligation: &ArtifactObligation,
        failure_domain: DeliverableFailureDomain,
    ) -> Self {
        let target = target_from_obligation(obligation, failure_domain);
        Self {
            correction_kind: correction_kind_for_target(contract.task_kind, &target),
            instruction: adapter_instruction(contract.task_kind, &target),
            target,
        }
    }

    pub(super) fn for_recovery_target(
        contract: &TaskContract,
        hint: &RecoveryTargetHint,
        failure_domain: DeliverableFailureDomain,
    ) -> Self {
        if let Some(obligation) = contract.obligation_for_target(hint) {
            return Self::for_obligation(contract, obligation, failure_domain);
        }
        let target = target_from_recovery_hint(hint, failure_domain);
        Self {
            correction_kind: correction_kind_for_target(contract.task_kind, &target),
            instruction: adapter_instruction(contract.task_kind, &target),
            target,
        }
    }

    pub(super) fn for_verifier_failure(
        contract: &TaskContract,
        hint: &RecoveryTargetHint,
    ) -> CorrectionPacket {
        Self::for_recovery_target(contract, hint, DeliverableFailureDomain::VerifierFailed)
    }

    #[cfg(test)]
    pub(super) fn for_diagnostic_failure(
        contract: &TaskContract,
        hint: &RecoveryTargetHint,
        failure_kind: super::VerifierDiagnosticFailureKind,
    ) -> CorrectionPacket {
        Self::for_recovery_target(
            contract,
            hint,
            failure_domain_for_diagnostic_failure(hint, failure_kind),
        )
    }
}

pub(super) fn obligation_id_for_parts(role: ArtifactRole, path: Option<&str>) -> String {
    let path = path.unwrap_or("*");
    format!("{}:{path}", role.label())
}

fn obligation_id_for_obligation(obligation: &ArtifactObligation) -> String {
    let base = obligation_id_for_parts(obligation.role, Some(&obligation.path));
    let Some(suffix) = (match obligation.schema.as_ref() {
        Some(DeliverableSchema::JsonFields(fields)) => fields.first(),
        _ => None,
    }) else {
        return base;
    };
    let suffix = suffix
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_ascii_whitespace() || matches!(ch, '-' | '_' | '.') {
                Some('_')
            } else {
                None
            }
        })
        .take(40)
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if suffix.is_empty() {
        base
    } else {
        format!("{base}#{suffix}")
    }
}

pub(super) fn default_failure_domain_for_obligation(
    obligation: &ArtifactObligation,
) -> DeliverableFailureDomain {
    if obligation.role == ArtifactRole::UsageDocs && !obligation.required_sections.is_empty() {
        return DeliverableFailureDomain::IncompleteSections;
    }
    if obligation.role == ArtifactRole::DataOutput && obligation.structured_record_schema.is_some()
    {
        return DeliverableFailureDomain::SchemaMismatch;
    }
    if matches!(
        obligation.schema.as_ref(),
        Some(DeliverableSchema::JsonFields(_))
    ) {
        return DeliverableFailureDomain::SchemaMismatch;
    }
    DeliverableFailureDomain::MissingDeliverable
}

pub(super) fn repair_packets_for_contract(contract: &TaskContract) -> Vec<RepairPacket> {
    let mut packets = contract
        .required_artifact_identities
        .iter()
        .map(|obligation| {
            RepairPacket::for_obligation(
                contract,
                obligation,
                default_failure_domain_for_obligation(obligation),
            )
        })
        .collect::<Vec<_>>();
    if packets.is_empty() {
        packets.extend(contract.required_artifacts.iter().map(|role| {
            let hint = RecoveryTargetHint {
                role: *role,
                path: String::new(),
                reason: "required artifact role remains unsatisfied".to_string(),
            };
            RepairPacket::for_recovery_target(
                contract,
                &hint,
                DeliverableFailureDomain::MissingDeliverable,
            )
        }));
    }
    packets
}

fn target_from_obligation(
    obligation: &ArtifactObligation,
    failure_domain: DeliverableFailureDomain,
) -> RepairObligationTarget {
    RepairObligationTarget {
        obligation_id: obligation_id_for_obligation(obligation),
        role: obligation.role,
        kind: obligation.kind,
        path: Some(sanitize_repair_job_text_with_char_cap(
            &obligation.path,
            MAX_EXPECTED_EVIDENCE_CHARS,
        )),
        expected_evidence: expected_evidence_for_obligation(obligation),
        failure_domain,
    }
}

fn target_from_recovery_hint(
    hint: &RecoveryTargetHint,
    failure_domain: DeliverableFailureDomain,
) -> RepairObligationTarget {
    let path = if hint.path.is_empty() {
        None
    } else {
        Some(sanitize_repair_job_text_with_char_cap(
            &hint.path,
            MAX_EXPECTED_EVIDENCE_CHARS,
        ))
    };
    RepairObligationTarget {
        obligation_id: obligation_id_for_parts(hint.role, path.as_deref()),
        role: hint.role,
        kind: default_kind_for_role(hint.role),
        path,
        expected_evidence: vec![bounded_evidence(format!(
            "repair target path for {}",
            hint.role.label()
        ))],
        failure_domain,
    }
}

fn expected_evidence_for_obligation(obligation: &ArtifactObligation) -> Vec<String> {
    let mut evidence = Vec::new();
    if !obligation.required_sections.is_empty() {
        evidence.push(bounded_evidence(format!(
            "required sections: {}",
            obligation
                .required_sections
                .iter()
                .take(MAX_EXPECTED_EVIDENCE_ITEMS)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if let Some(schema) = obligation.structured_record_schema.as_ref()
        && !schema.columns.is_empty()
    {
        evidence.push(bounded_evidence(format!(
            "required columns: {}",
            schema
                .columns
                .iter()
                .take(MAX_EXPECTED_EVIDENCE_ITEMS)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if let Some(DeliverableSchema::JsonFields(fields)) = obligation.schema.as_ref()
        && !fields.is_empty()
    {
        evidence.push(bounded_evidence(format!(
            "required JSON field(s): {}",
            fields
                .iter()
                .take(MAX_EXPECTED_EVIDENCE_ITEMS)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !obligation.acceptance_criteria.is_empty() {
        evidence.push(bounded_evidence(format!(
            "acceptance criteria: {}",
            obligation
                .acceptance_criteria
                .iter()
                .take(MAX_EXPECTED_EVIDENCE_ITEMS)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    evidence.push(bounded_evidence(format!(
        "deliverable path: {}",
        obligation.path
    )));
    evidence.truncate(MAX_EXPECTED_EVIDENCE_ITEMS);
    evidence
}

fn adapter_instruction(task_kind: TaskKind, target: &RepairObligationTarget) -> String {
    let instruction = match (task_kind, target.failure_domain) {
        (TaskKind::Docs, DeliverableFailureDomain::IncompleteSections)
        | (_, DeliverableFailureDomain::IncompleteSections) => {
            "add or repair the required documentation sections for this deliverable obligation"
        }
        (TaskKind::Data, DeliverableFailureDomain::SchemaMismatch)
        | (_, DeliverableFailureDomain::SchemaMismatch) => {
            "repair the structured data deliverable so its schema matches the required evidence"
        }
        (TaskKind::Coding, DeliverableFailureDomain::VerifierFailed)
        | (_, DeliverableFailureDomain::VerifierFailed) => {
            "repair the selected deliverable obligation while preserving verifier safety gates"
        }
        #[cfg(test)]
        (_, DeliverableFailureDomain::GeneratedTestBug) => {
            "repair or replace the generated test artifact before using it as verifier authority"
        }
        #[cfg(test)]
        (_, DeliverableFailureDomain::InvalidManifest) => {
            "repair the setup or manifest artifact so verifier setup is structurally valid"
        }
        (_, DeliverableFailureDomain::UnsafeOrOutOfScope) => {
            "discard the unsafe or out-of-scope proposal and choose an admitted obligation target"
        }
        (_, DeliverableFailureDomain::MalformedDeliverable) => {
            "repair malformed deliverable content for the obligation target"
        }
        (_, DeliverableFailureDomain::MissingDeliverable) => {
            "create or complete the missing deliverable obligation"
        }
    };
    sanitize_repair_job_text_with_char_cap(instruction, MAX_REPAIR_INSTRUCTION_CHARS)
}

fn correction_kind_for_target(
    task_kind: TaskKind,
    target: &RepairObligationTarget,
) -> CorrectionKind {
    match (task_kind, target.failure_domain, target.role) {
        #[cfg(test)]
        (_, DeliverableFailureDomain::GeneratedTestBug, _) => CorrectionKind::TestCorrection,
        #[cfg(test)]
        (_, DeliverableFailureDomain::InvalidManifest, _) => CorrectionKind::ManifestCorrection,
        (TaskKind::Docs, DeliverableFailureDomain::IncompleteSections, _)
        | (_, DeliverableFailureDomain::IncompleteSections, ArtifactRole::UsageDocs) => {
            CorrectionKind::SectionAddition
        }
        (TaskKind::Data, DeliverableFailureDomain::SchemaMismatch, _)
        | (_, DeliverableFailureDomain::SchemaMismatch, ArtifactRole::DataOutput) => {
            CorrectionKind::SchemaCorrection
        }
        (TaskKind::Research, DeliverableFailureDomain::MissingDeliverable, _)
        | (TaskKind::Research, DeliverableFailureDomain::MalformedDeliverable, _) => {
            CorrectionKind::CitationSupport
        }
        (_, DeliverableFailureDomain::MissingDeliverable, ArtifactRole::UsageDocs) => {
            CorrectionKind::ChecklistCompletion
        }
        (_, DeliverableFailureDomain::MissingDeliverable, _) => CorrectionKind::ChecklistCompletion,
        (_, DeliverableFailureDomain::MalformedDeliverable, ArtifactRole::DataOutput) => {
            CorrectionKind::SchemaCorrection
        }
        (_, DeliverableFailureDomain::UnsafeOrOutOfScope, _) => CorrectionKind::ChecklistCompletion,
        _ => CorrectionKind::Patch,
    }
}

#[cfg(test)]
fn failure_domain_for_diagnostic_failure(
    hint: &RecoveryTargetHint,
    failure_kind: super::VerifierDiagnosticFailureKind,
) -> DeliverableFailureDomain {
    match failure_kind {
        super::VerifierDiagnosticFailureKind::TestBug => DeliverableFailureDomain::GeneratedTestBug,
        super::VerifierDiagnosticFailureKind::ConfigOrVerifierError
            if hint.role == ArtifactRole::Setup =>
        {
            DeliverableFailureDomain::InvalidManifest
        }
        super::VerifierDiagnosticFailureKind::ConfigOrVerifierError => {
            DeliverableFailureDomain::MalformedDeliverable
        }
        _ => DeliverableFailureDomain::VerifierFailed,
    }
}

fn bounded_evidence(raw: String) -> String {
    sanitize_repair_job_text_with_char_cap(&raw, MAX_EXPECTED_EVIDENCE_CHARS)
}

fn default_kind_for_role(role: ArtifactRole) -> DeliverableKind {
    match role {
        ArtifactRole::Implementation => DeliverableKind::Code,
        ArtifactRole::Test => DeliverableKind::Tests,
        ArtifactRole::UsageDocs => DeliverableKind::UsageDocs,
        ArtifactRole::Setup => DeliverableKind::Setup,
        ArtifactRole::DataOutput => DeliverableKind::Data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_for_first_obligation(request: &str) -> RepairPacket {
        let contract = TaskContract::from_request(request);
        let obligation = contract
            .required_artifact_identities
            .first()
            .expect("expected a required obligation");
        RepairPacket::for_obligation(
            &contract,
            obligation,
            default_failure_domain_for_obligation(obligation),
        )
    }

    #[test]
    fn docs_required_sections_produce_incomplete_sections_packet() {
        let packet =
            packet_for_first_obligation("Update README.md with setup, usage, and test sections.");

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::IncompleteSections
        );
        assert_eq!(packet.correction_kind, CorrectionKind::SectionAddition);
        assert_eq!(packet.target.obligation_id, "usage_docs:README.md");
        assert!(
            packet
                .target
                .expected_evidence
                .iter()
                .any(|item| item.contains("required sections"))
        );
    }

    #[test]
    fn data_schema_mismatch_produces_structured_record_packet() {
        let packet =
            packet_for_first_obligation("Generate output.csv with columns id, name, score.");

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::SchemaMismatch
        );
        assert_eq!(packet.correction_kind, CorrectionKind::SchemaCorrection);
        assert_eq!(packet.target.kind, DeliverableKind::StructuredRecord);
        assert!(
            packet
                .target
                .expected_evidence
                .iter()
                .any(|item| item.contains("required columns"))
        );
    }

    #[test]
    fn coding_verifier_failure_packet_targets_obligation() {
        let contract = TaskContract::from_request("Create main.py as a Python CLI and add tests.");
        let hint = RecoveryTargetHint {
            role: ArtifactRole::Implementation,
            path: "main.py".to_string(),
            reason: "verifier failed".to_string(),
        };
        let packet = RepairPacket::for_verifier_failure(&contract, &hint);

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::VerifierFailed
        );
        assert_eq!(packet.correction_kind, CorrectionKind::Patch);
        assert_eq!(packet.target.obligation_id, "implementation:main.py");
        assert_eq!(packet.target.role, ArtifactRole::Implementation);
    }

    #[test]
    fn node_cli_bin_entry_packet_exposes_json_field_obligation() {
        let contract = TaskContract::from_request(
            "Create a Node CLI. Include package.json with a bin entry, source, tests, and README.md.",
        );
        let obligation = contract
            .required_artifact_identities
            .iter()
            .find(|obligation| {
                obligation.path == "package.json"
                    && matches!(
                        obligation.schema.as_ref(),
                        Some(DeliverableSchema::JsonFields(fields)) if fields.as_slice() == ["bin"]
                    )
            })
            .expect("bin entry obligation");
        let packet = RepairPacket::for_obligation(
            &contract,
            obligation,
            default_failure_domain_for_obligation(obligation),
        );

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::SchemaMismatch
        );
        assert!(packet.target.obligation_id.contains("package.json#"));
        assert!(
            packet
                .target
                .expected_evidence
                .iter()
                .any(|item| item.contains("required JSON field(s): bin"))
        );
        assert!(
            packet
                .target
                .expected_evidence
                .iter()
                .any(|item| item.contains("bin entry"))
        );
    }

    #[test]
    fn research_missing_deliverable_can_request_citation_support() {
        let contract = TaskContract::from_request("Research battery safety and cite sources.");
        let hint = RecoveryTargetHint {
            role: ArtifactRole::UsageDocs,
            path: "notes.md".to_string(),
            reason: "research notes missing citation support".to_string(),
        };
        let packet = RepairPacket::for_recovery_target(
            &contract,
            &hint,
            DeliverableFailureDomain::MissingDeliverable,
        );

        assert_eq!(packet.correction_kind, CorrectionKind::CitationSupport);
    }

    #[test]
    fn test_bug_diagnostic_creates_test_correction_packet() {
        let contract = TaskContract::from_request("Create a Rust CLI and add tests.");
        let hint = RecoveryTargetHint {
            role: ArtifactRole::Test,
            path: "tests/cli.rs".to_string(),
            reason: "generated test uses a bad binary path".to_string(),
        };
        let packet = RepairPacket::for_diagnostic_failure(
            &contract,
            &hint,
            super::super::VerifierDiagnosticFailureKind::TestBug,
        );

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::GeneratedTestBug
        );
        assert_eq!(packet.correction_kind, CorrectionKind::TestCorrection);
        assert_eq!(packet.correction_kind.as_str(), "test_correction");
    }

    #[test]
    fn config_diagnostic_on_setup_creates_manifest_correction_packet() {
        let contract = TaskContract::from_request("Create a Node CLI with package.json and tests.");
        let hint = RecoveryTargetHint {
            role: ArtifactRole::Setup,
            path: "package.json".to_string(),
            reason: "package.json is malformed".to_string(),
        };
        let packet = RepairPacket::for_diagnostic_failure(
            &contract,
            &hint,
            super::super::VerifierDiagnosticFailureKind::ConfigOrVerifierError,
        );

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::InvalidManifest
        );
        assert_eq!(packet.correction_kind, CorrectionKind::ManifestCorrection);
        assert_eq!(packet.correction_kind.as_str(), "manifest_correction");
    }
}
