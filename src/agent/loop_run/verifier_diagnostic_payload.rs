//! Verifier diagnostic JSON payload assembly.
//!
//! This module owns wire-payload shaping for the short-lived diagnostic LLM.
//! It does not choose repair targets or encode success rules.

use std::path::Path;

use super::failure_packet::FailurePacket;
use super::repair_authority::AuthorityEvidence;
use super::repair_framework_findings::{
    VerifierDiagnosticFileExcerpt, VerifierDiagnosticFrameworkFinding,
};
use super::repair_job::RepairJob;
use super::required_behavior::{BehaviorContractProjection, behavior_contract_payload_value};
use super::task_contract::mask_and_cap_recovery_field;
use super::verifier_evidence_scope::verifier_evidence_scope_packet_for_context;
use super::verifier_failure_artifacts::verifier_output_failure_hints;
use super::verifier_failure_signature::compact_verifier_failure_text;
use super::verifier_repair_targeting::verifier_diagnostic_missing_setup_candidates;
use crate::session::feedback::mask_secrets;

pub(super) struct VerifierDiagnosticPayloadInput<'a> {
    pub(super) work_root: &'a Path,
    pub(super) context: &'a RepairJob,
    pub(super) active_request: &'a str,
    pub(super) behavior_projection: Option<&'a BehaviorContractProjection>,
    pub(super) diagnostic_excerpts: &'a [VerifierDiagnosticFileExcerpt],
    pub(super) framework_findings: &'a [VerifierDiagnosticFrameworkFinding],
}

pub(super) fn build_verifier_diagnostic_payload(
    input: VerifierDiagnosticPayloadInput<'_>,
) -> String {
    let context = input.context;
    let behavior_contract = behavior_contract_payload_value(input.behavior_projection);
    let failure_packet = FailurePacket::from_repair_job_with_work_root(input.work_root, context);
    let evidence_scope = verifier_evidence_scope_packet_for_context(input.work_root, context);
    let authority_evidence = AuthorityEvidence::from_packet_and_context(
        &failure_packet,
        input.active_request,
        input.behavior_projection.is_some(),
    );
    let payload = serde_json::json!({
        "task_summary": compact_verifier_failure_text(input.active_request, 500),
        "command": context.command,
        "output_excerpt": context.output_excerpt,
        "failure_packet": failure_packet.to_json_value(),
        "evidence_scope": evidence_scope.to_json_value(),
        "authority_evidence": authority_evidence.to_json_value(),
        "first_pass_failure_type": context.failure_type.as_str(),
        "failure_signature": context.failure_signature,
        "failure_count": context.failure_count,
        "previous_failure_signature": context.previous_failure_signature,
        "previous_failure_count": context.previous_failure_count,
        "repair_rerun_outcome": context.rerun_outcome.map(|outcome| outcome.as_str()),
        "failure_location": failure_location_payload(context),
        "changed_candidates": changed_candidates_payload(
            input.work_root,
            context,
            input.active_request,
        ),
        "exhausted_repair_targets": exhausted_repair_targets_payload(context),
        "no_progress_recovery": context.no_progress_diagnostic_payload(),
        "safe_file_excerpts": safe_file_excerpts_payload(input.diagnostic_excerpts),
        "framework_findings": framework_findings_payload(input.framework_findings),
        "behavior_contract": behavior_contract,
    });
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn safe_file_excerpts_payload(
    excerpts: &[VerifierDiagnosticFileExcerpt],
) -> Vec<serde_json::Value> {
    excerpts
        .iter()
        .map(|excerpt| {
            serde_json::json!({
                "path": mask_secrets(&excerpt.path),
                "role": excerpt.role.label(),
                "excerpt": excerpt.excerpt.as_str(),
            })
        })
        .collect()
}

fn framework_findings_payload(
    findings: &[VerifierDiagnosticFrameworkFinding],
) -> Vec<serde_json::Value> {
    findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "kind": finding.kind.as_str(),
                "path": mask_secrets(&finding.path),
                "role": finding.role.label(),
                "summary": finding.summary.as_str(),
            })
        })
        .collect()
}

fn failure_location_payload(context: &RepairJob) -> Option<serde_json::Value> {
    context.target_hint.as_ref().map(|hint| {
        serde_json::json!({
            "path": mask_secrets(&hint.path),
            "role": hint.role.label(),
            "reason": mask_and_cap_recovery_field(&hint.reason),
        })
    })
}

fn changed_candidates_payload(
    work_root: &Path,
    context: &RepairJob,
    active_request: &str,
) -> Vec<serde_json::Value> {
    let mut candidates = context
        .changed_file_hints
        .iter()
        .take(12)
        .map(|hint| {
            serde_json::json!({
                "path": mask_secrets(&hint.path),
                "role": hint.role.label(),
            })
        })
        .collect::<Vec<_>>();
    for hint in verifier_output_failure_hints(work_root, &context.output_excerpt) {
        push_candidate_if_absent(
            &mut candidates,
            &hint.path,
            hint.role.label(),
            DiagnosticCandidateKind::VerifierOutputFailureArtifact,
        );
    }
    for hint in verifier_diagnostic_missing_setup_candidates(work_root, context, active_request) {
        push_candidate_if_absent(
            &mut candidates,
            &hint.path,
            hint.role.label(),
            DiagnosticCandidateKind::MissingSetupArtifact,
        );
    }
    candidates
}

fn push_candidate_if_absent(
    candidates: &mut Vec<serde_json::Value>,
    path: &str,
    role: &str,
    kind: DiagnosticCandidateKind,
) {
    let masked_path = mask_secrets(path);
    if candidates.iter().any(|candidate| {
        candidate.get("path").and_then(serde_json::Value::as_str) == Some(masked_path.as_str())
    }) {
        return;
    }
    candidates.push(serde_json::json!({
        "path": masked_path,
        "role": role,
        "candidate_kind": kind.as_str(),
    }));
}

fn exhausted_repair_targets_payload(context: &RepairJob) -> Vec<serde_json::Value> {
    context
        .exhausted_repair_targets
        .iter()
        .take(12)
        .map(|target| {
            serde_json::json!({
                "path": mask_secrets(&target.path),
                "role": target.role.label(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticCandidateKind {
    VerifierOutputFailureArtifact,
    MissingSetupArtifact,
}

impl DiagnosticCandidateKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::VerifierOutputFailureArtifact => "verifier_output_failure_artifact",
            Self::MissingSetupArtifact => "missing_setup_artifact",
        }
    }
}
