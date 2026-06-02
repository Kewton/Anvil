use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, RecoveryTargetHint, TaskContract, TaskKind,
};

const MAX_EXPECTED_EVIDENCE_ITEMS: usize = 8;
const MAX_EXPECTED_EVIDENCE_CHARS: usize = 160;
const MAX_REPAIR_INSTRUCTION_CHARS: usize = 360;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliverableFailureDomain {
    MissingDeliverable,
    MalformedDeliverable,
    VerifierFailed,
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
    pub(super) instruction: String,
}

impl RepairPacket {
    pub(super) fn for_obligation(
        contract: &TaskContract,
        obligation: &ArtifactObligation,
        failure_domain: DeliverableFailureDomain,
    ) -> Self {
        let target = target_from_obligation(obligation, failure_domain);
        Self {
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
            instruction: adapter_instruction(contract.task_kind, &target),
            target,
        }
    }
}

pub(super) fn obligation_id_for_parts(role: ArtifactRole, path: Option<&str>) -> String {
    let path = path.unwrap_or("*");
    format!("{}:{path}", role.label())
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
        obligation_id: obligation_id_for_parts(obligation.role, Some(&obligation.path)),
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
        let packet = RepairPacket::for_recovery_target(
            &contract,
            &hint,
            DeliverableFailureDomain::VerifierFailed,
        );

        assert_eq!(
            packet.target.failure_domain,
            DeliverableFailureDomain::VerifierFailed
        );
        assert_eq!(packet.target.obligation_id, "implementation:main.py");
        assert_eq!(packet.target.role, ArtifactRole::Implementation);
    }
}
