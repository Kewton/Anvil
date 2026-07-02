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
    )
    .map(|brief| normalize_shadow_brief_for_packet(brief, &packet, assessment));

    match brief_result {
        Ok(brief) => {
            let evidence = authority_evidence_for_shadow(context, &packet);
            let action_payload = match super::repair_authority::build_repair_action_with_authority(
                &brief, &packet, &evidence,
            ) {
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

pub(super) fn verifier_repair_action_space_payload_for_context(
    context: &super::repair_job::RepairJob,
    target_hint: &super::task_contract::RecoveryTargetHint,
) -> serde_json::Value {
    let action = verifier_repair_action_for_context(context);
    super::repair_action_space::repair_action_plan_for_selected_target(
        Some(target_hint),
        action.as_ref(),
    )
    .to_json_value()
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
    let base = super::failure_packet::FailurePacket::from_repair_job(context);
    let mut candidate_artifacts = base.candidate_artifacts;
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

    let mut packet = super::failure_packet::FailurePacket::new(
        &context.command,
        assessment.failure_kind.as_str(),
        &context.output_excerpt,
        base.affected_cases,
        base.observed_expected_pairs,
        candidate_artifacts,
        base.prior_attempts,
    );
    if packet.timeout_kind.is_none() {
        packet.timeout_kind = context.timeout_kind;
    }
    packet
}

fn verifier_repair_action_for_context(
    context: &super::repair_job::RepairJob,
) -> Option<super::repair_action::RepairAction> {
    let assessment = context.assessment.as_ref()?;
    let packet = failure_packet_for_verifier_pipeline_shadow(context, assessment);
    let brief = super::repair_brief::repair_brief_from_legacy_diagnostic(
        legacy_repair_brief_input_from_assessment(assessment, 0.6),
    )
    .map(|brief| normalize_shadow_brief_for_packet(brief, &packet, assessment))
    .ok()?;
    let evidence = authority_evidence_for_shadow(context, &packet);
    super::repair_authority::build_repair_action_with_authority(&brief, &packet, &evidence).ok()
}

fn normalize_shadow_brief_for_packet(
    mut brief: super::repair_brief::RepairBrief,
    packet: &super::failure_packet::FailurePacket,
    assessment: &super::VerifierRepairAssessment,
) -> super::repair_brief::RepairBrief {
    let target_is_test = brief
        .repair_target
        .as_ref()
        .is_some_and(|target| target.role == super::task_contract::ArtifactRole::Test);
    if target_is_test
        && !packet.observed_expected_pairs.is_empty()
        && matches!(
            assessment.failure_kind,
            super::VerifierDiagnosticFailureKind::AssertionMismatch
                | super::VerifierDiagnosticFailureKind::BadTest
                | super::VerifierDiagnosticFailureKind::TestBug
        )
    {
        brief.allowed_change_kind =
            super::repair_brief::AllowedChangeKind::FixGeneratedTestExpectation;
    }
    brief
}

fn authority_evidence_for_shadow(
    context: &super::repair_job::RepairJob,
    packet: &super::failure_packet::FailurePacket,
) -> super::repair_authority::AuthorityEvidence {
    let spec_authority = context
        .semantic_plan
        .as_ref()
        .map(|plan| plan.spec_authority);
    super::repair_authority::AuthorityEvidence {
        user_request_has_explicit_spec: spec_authority
            == Some(super::spec_authority::SpecAuthority::UserRequest),
        behavior_contract_present: matches!(
            spec_authority,
            Some(
                super::spec_authority::SpecAuthority::BehaviorContract
                    | super::spec_authority::SpecAuthority::UserRequest
            )
        ),
        observed_expected_pair_count: packet.observed_expected_pairs.len(),
        candidate_artifact_count: packet.candidate_artifacts.len(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_target() -> super::super::task_contract::RecoveryTargetHint {
        super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Test,
            path: "tests/test_password_strength.py".to_string(),
            reason: "verifier output names this test artifact".to_string(),
        }
    }

    fn test_assessment() -> super::super::VerifierRepairAssessment {
        let target = test_target();
        super::super::VerifierRepairAssessment {
            failure_kind: super::super::VerifierDiagnosticFailureKind::TestBug,
            failure_type: super::super::VerifierFailureType::AssertionFailure,
            probable_cause_role: Some(super::super::task_contract::ArtifactRole::Test),
            needed_reads: vec![target.clone()],
            repair_target_hint: Some(target.clone()),
            repair_plan: vec![target],
            summary: Some(
                "generated test expected literal contradicts behavior contract".to_string(),
            ),
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        }
    }

    fn test_brief() -> super::super::repair_brief::RepairBrief {
        super::super::repair_brief::RepairBrief {
            failure_summary: "assertion mismatch".to_string(),
            root_cause: "generated test bug".to_string(),
            source_of_truth: super::super::repair_brief::SourceOfTruth::Unknown,
            repair_target: Some(super::super::repair_brief::RepairBriefTarget {
                role: super::super::task_contract::ArtifactRole::Test,
                path: "tests/test_password_strength.py".to_string(),
            }),
            allowed_change_kind:
                super::super::repair_brief::AllowedChangeKind::FixTestImportOrSetup,
            must_preserve: Vec::new(),
            concrete_fix_intent: "fix test expectation".to_string(),
            confidence: 0.9,
            source: super::super::repair_brief::RepairBriefSource::DiagnosticLlm,
        }
    }

    fn assertion_packet() -> super::super::failure_packet::FailurePacket {
        super::super::failure_packet::FailurePacket::new(
            "python3 -m pytest",
            "test_bug",
            "E AssertionError: assert 2 == 1",
            Vec::new(),
            vec![super::super::failure_packet::ObservedExpectedPair {
                observed: "2".to_string(),
                expected: "1".to_string(),
                assertion_shape: "assert_equal".to_string(),
            }],
            vec![super::super::failure_packet::CandidateArtifact::new(
                super::super::task_contract::ArtifactRole::Test,
                "tests/test_password_strength.py",
                "verifier output names this test artifact",
            )],
            Vec::new(),
        )
    }

    #[test]
    fn test_bug_with_observed_expected_pair_projects_to_expectation_repair() {
        let packet = assertion_packet();
        let assessment = test_assessment();
        let normalized = normalize_shadow_brief_for_packet(test_brief(), &packet, &assessment);

        assert_eq!(
            normalized.allowed_change_kind,
            super::super::repair_brief::AllowedChangeKind::FixGeneratedTestExpectation
        );

        let evidence = super::super::repair_authority::AuthorityEvidence {
            user_request_has_explicit_spec: true,
            behavior_contract_present: true,
            observed_expected_pair_count: packet.observed_expected_pairs.len(),
            candidate_artifact_count: packet.candidate_artifacts.len(),
        };
        let action = super::super::repair_authority::build_repair_action_with_authority(
            &normalized,
            &packet,
            &evidence,
        )
        .expect("authorized generated test expectation repair");
        assert_eq!(
            action.allowed_change_kind,
            super::super::repair_brief::AllowedChangeKind::FixGeneratedTestExpectation
        );
    }

    #[test]
    fn test_bug_without_observed_expected_pair_keeps_import_setup_repair() {
        let mut packet = assertion_packet();
        packet.observed_expected_pairs.clear();
        let assessment = test_assessment();
        let normalized = normalize_shadow_brief_for_packet(test_brief(), &packet, &assessment);

        assert_eq!(
            normalized.allowed_change_kind,
            super::super::repair_brief::AllowedChangeKind::FixTestImportOrSetup
        );
    }
}
