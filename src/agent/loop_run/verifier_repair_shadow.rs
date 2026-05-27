use crate::logging::stable_path_hash;

use super::verifier_assessment_parser::ParsedVerifierRepairAssessment;

pub(super) fn build_verifier_repair_pipeline_shadow_payload(
    session_id: &str,
    model: &str,
    model_role: &'static str,
    context: &super::repair_job::RepairJob,
    parsed: &ParsedVerifierRepairAssessment,
    assessment: &super::VerifierRepairAssessment,
) -> serde_json::Value {
    let packet = failure_packet_for_verifier_pipeline_shadow(context, assessment);
    let brief_result = super::repair_brief::repair_brief_from_legacy_diagnostic(
        legacy_repair_brief_input_for_shadow(parsed, assessment),
    );

    match brief_result {
        Ok(brief) => {
            let action_payload = match super::repair_action::build_repair_action(&brief, &packet) {
                Ok(action) => serde_json::json!({
                    "status": "accepted",
                    "target_role": action.target_role.label(),
                    "target_path_hash": stable_path_hash(&action.target_path),
                    "allowed_change_kind": action.allowed_change_kind.as_str(),
                    "source_of_truth": action.source_of_truth.as_str(),
                    "budget": action.budget,
                }),
                Err(err) => serde_json::json!({
                    "status": "rejected",
                    "reason": err.as_str(),
                }),
            };
            let brief_target = brief.repair_target.as_ref();
            serde_json::json!({
                "session_id": session_id,
                "model": model,
                "model_role": model_role,
                "packet_signature": packet.failure_signature,
                "candidate_count": packet.candidate_artifacts.len(),
                "brief": {
                    "status": "accepted",
                    "source": brief.source.as_str(),
                    "target_role": brief_target.map(|target| target.role.label()),
                    "target_path_hash": brief_target.map(|target| stable_path_hash(&target.path)),
                    "allowed_change_kind": brief.allowed_change_kind.as_str(),
                    "source_of_truth": brief.source_of_truth.as_str(),
                    "confidence": brief.confidence,
                },
                "action": action_payload,
            })
        }
        Err(err) => serde_json::json!({
            "session_id": session_id,
            "model": model,
            "model_role": model_role,
            "packet_signature": packet.failure_signature,
            "candidate_count": packet.candidate_artifacts.len(),
            "brief": {
                "status": "rejected",
                "reason": err.as_str(),
            },
            "action": {
                "status": "skipped",
                "reason": "brief_rejected",
            },
        }),
    }
}

pub(super) fn verifier_repair_action_payload_for_context(
    context: &super::repair_job::RepairJob,
) -> Option<serde_json::Value> {
    let action = verifier_repair_action_for_context(context)?;
    Some(serde_json::json!({
        "target_role": action.target_role.label(),
        "target_path": action.target_path,
        "allowed_change_kind": action.allowed_change_kind.as_str(),
        "source_of_truth": action.source_of_truth.as_str(),
        "budget": action.budget,
        "brief_confidence": action.brief_confidence,
    }))
}

pub(super) fn legacy_repair_brief_input_from_assessment(
    assessment: &super::VerifierRepairAssessment,
    default_confidence: f64,
) -> super::repair_brief::LegacyDiagnosticBriefInput {
    let repair_target = assessment.repair_target_hint.as_ref();
    let repair_target_path = repair_target.map(|target| target.path.clone());
    super::repair_brief::LegacyDiagnosticBriefInput {
        failure_kind: assessment.failure_kind.as_str().to_string(),
        probable_cause_role: repair_target
            .map(|target| target.role)
            .or(assessment.probable_cause_role),
        repair_target_path,
        repair_target_confidence: repair_target.map(|_| default_confidence),
        summary: assessment.summary.clone(),
    }
}

fn failure_packet_for_verifier_pipeline_shadow(
    context: &super::repair_job::RepairJob,
    assessment: &super::VerifierRepairAssessment,
) -> super::failure_packet::FailurePacket {
    let mut candidate_artifacts = Vec::new();
    for (hint, reason) in assessment
        .repair_target_hint
        .iter()
        .map(|hint| (hint, "diagnostic selected repair target"))
        .chain(
            assessment
                .repair_plan
                .iter()
                .map(|hint| (hint, "diagnostic repair plan candidate")),
        )
        .chain(
            assessment
                .needed_reads
                .iter()
                .map(|hint| (hint, "diagnostic read candidate")),
        )
        .chain(
            context
                .changed_file_hints
                .iter()
                .map(|hint| (hint, "changed workspace file candidate")),
        )
    {
        push_shadow_candidate_artifact(&mut candidate_artifacts, hint, reason);
    }

    super::failure_packet::FailurePacket::new(
        &context.command,
        assessment.failure_kind.as_str(),
        &context.output_excerpt,
        Vec::new(),
        Vec::new(),
        candidate_artifacts,
        Vec::new(),
    )
}

fn verifier_repair_action_for_context(
    context: &super::repair_job::RepairJob,
) -> Option<super::repair_action::RepairAction> {
    let assessment = context.assessment.as_ref()?;
    let packet = failure_packet_for_verifier_pipeline_shadow(context, assessment);
    let brief = super::repair_brief::repair_brief_from_legacy_diagnostic(
        legacy_repair_brief_input_from_assessment(assessment, 0.6),
    )
    .ok()?;
    super::repair_action::build_repair_action(&brief, &packet).ok()
}

fn push_shadow_candidate_artifact(
    candidate_artifacts: &mut Vec<super::failure_packet::CandidateArtifact>,
    hint: &super::task_contract::RecoveryTargetHint,
    reason: &str,
) {
    let candidate = super::failure_packet::CandidateArtifact::new(hint.role, &hint.path, reason);
    if candidate.path.is_empty() {
        return;
    }
    if candidate_artifacts
        .iter()
        .any(|existing| existing.path == candidate.path)
    {
        return;
    }
    candidate_artifacts.push(candidate);
}

fn legacy_repair_brief_input_for_shadow(
    parsed: &ParsedVerifierRepairAssessment,
    assessment: &super::VerifierRepairAssessment,
) -> super::repair_brief::LegacyDiagnosticBriefInput {
    let repair_target = assessment.repair_target_hint.as_ref();
    let repair_target_path = repair_target.map(|target| target.path.clone());
    let repair_target_confidence = repair_target_path
        .as_deref()
        .and_then(|path| parsed_confidence_for_path(parsed, path))
        .or_else(|| repair_target_path.as_ref().map(|_| 0.6));

    let mut input = legacy_repair_brief_input_from_assessment(assessment, 0.6);
    input.probable_cause_role = repair_target
        .map(|target| target.role)
        .or(assessment.probable_cause_role)
        .or(parsed.probable_cause_role);
    input.repair_target_confidence = repair_target_confidence;
    input.summary = assessment
        .summary
        .clone()
        .or_else(|| parsed.summary.clone());
    input
}

fn parsed_confidence_for_path(parsed: &ParsedVerifierRepairAssessment, path: &str) -> Option<f64> {
    let normalized_path = normalize_shadow_path(path);
    parsed
        .repair_plan
        .iter()
        .chain(parsed.repair_targets.iter())
        .find(|target| normalize_shadow_path(&target.path) == normalized_path)
        .map(|target| target.confidence)
}

fn normalize_shadow_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}
