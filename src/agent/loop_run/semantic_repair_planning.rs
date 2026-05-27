use super::verifier_assessment_parser::ParsedVerifierRepairAssessment;

pub(super) fn build_semantic_failure_report_from_legacy(
    parsed: &ParsedVerifierRepairAssessment,
    repair_job: &super::repair_job::RepairJob,
) -> Option<super::semantic_failure::SemanticFailureReport> {
    use super::semantic_failure::{
        ContractConflict, MAX_CLUSTER_TEXT_CHARS, MAX_REPAIR_HYPOTHESIS_CHARS,
        build_failure_cluster_from_observation,
    };

    let preferred_repair_role = parsed
        .probable_cause_role
        .unwrap_or(super::task_contract::ArtifactRole::Implementation);
    let observed = if !repair_job.failure_signature.is_empty() {
        repair_job.failure_signature.as_str()
    } else {
        repair_job.output_excerpt.as_str()
    };
    let expected = parsed.summary.as_deref().unwrap_or("");
    let cluster = build_failure_cluster_from_observation(
        observed,
        expected,
        "",
        "",
        &[preferred_repair_role],
        Vec::new(),
    );
    let contract_conflict = ContractConflict {
        implementation: super::repair_job::sanitize_repair_job_text_with_char_cap(
            parsed.summary.as_deref().unwrap_or(""),
            MAX_CLUSTER_TEXT_CHARS,
        ),
        test: String::new(),
        usage_docs: String::new(),
    };
    let raw_hypothesis = parsed
        .repair_plan
        .first()
        .map(|target| {
            if target.reason.is_empty() {
                target.path.clone()
            } else {
                format!("{}: {}", target.path, target.reason)
            }
        })
        .or_else(|| parsed.summary.clone())
        .unwrap_or_default();
    let repair_hypothesis = super::repair_job::sanitize_repair_job_text_with_char_cap(
        &raw_hypothesis,
        MAX_REPAIR_HYPOTHESIS_CHARS,
    );

    Some(super::semantic_failure::SemanticFailureReport {
        failure_kind: parsed.failure_kind,
        failure_clusters: vec![cluster],
        contract_conflict,
        preferred_repair_role,
        repair_hypothesis,
        confidence: 0.5,
    })
}

pub(super) fn build_semantic_failure_report_from_legacy_assessment(
    assessment: &super::VerifierRepairAssessment,
    repair_job: &super::repair_job::RepairJob,
) -> Option<super::semantic_failure::SemanticFailureReport> {
    use super::semantic_failure::{
        ContractConflict, MAX_CLUSTER_TEXT_CHARS, MAX_REPAIR_HYPOTHESIS_CHARS,
        build_failure_cluster_from_observation,
    };

    let mut admitted_targets = Vec::<super::task_contract::RecoveryTargetHint>::new();
    if let Some(target) = assessment.repair_target_hint.as_ref() {
        admitted_targets.push(target.clone());
    }
    for target in assessment
        .repair_plan
        .iter()
        .chain(assessment.needed_reads.iter())
    {
        if !admitted_targets
            .iter()
            .any(|existing| existing.role == target.role && existing.path == target.path)
        {
            admitted_targets.push(target.clone());
        }
    }
    if admitted_targets.is_empty() {
        return None;
    }

    let preferred_repair_role = assessment
        .repair_target_hint
        .as_ref()
        .map(|target| target.role)
        .or(assessment.probable_cause_role)
        .or_else(|| admitted_targets.first().map(|target| target.role))?;
    let failure_kind = semantic_failure_kind_for_legacy_assessment(
        assessment.failure_kind,
        repair_job.failure_type,
        preferred_repair_role,
    );
    let observed = if !repair_job.failure_signature.is_empty() {
        repair_job.failure_signature.as_str()
    } else {
        repair_job.output_excerpt.as_str()
    };
    let expected = assessment.summary.as_deref().unwrap_or("");
    let mut cluster = build_failure_cluster_from_observation(
        observed,
        expected,
        "",
        "",
        &[preferred_repair_role],
        Vec::new(),
    );
    cluster.admitted_cluster_targets = admitted_targets;
    let summary = assessment.summary.as_deref().unwrap_or("");
    let contract_conflict = ContractConflict {
        implementation: super::repair_job::sanitize_repair_job_text_with_char_cap(
            summary,
            MAX_CLUSTER_TEXT_CHARS,
        ),
        test: String::new(),
        usage_docs: String::new(),
    };
    let raw_hypothesis = assessment
        .repair_target_hint
        .as_ref()
        .or_else(|| assessment.repair_plan.first())
        .or_else(|| assessment.needed_reads.first())
        .map(|target| {
            if target.reason.is_empty() {
                target.path.clone()
            } else {
                format!("{}: {}", target.path, target.reason)
            }
        })
        .or_else(|| assessment.summary.clone())
        .unwrap_or_default();
    let repair_hypothesis = super::repair_job::sanitize_repair_job_text_with_char_cap(
        &raw_hypothesis,
        MAX_REPAIR_HYPOTHESIS_CHARS,
    );

    Some(super::semantic_failure::SemanticFailureReport {
        failure_kind,
        failure_clusters: vec![cluster],
        contract_conflict,
        preferred_repair_role,
        repair_hypothesis,
        confidence: 0.5,
    })
}

fn semantic_failure_kind_for_legacy_assessment(
    assessment_kind: super::VerifierDiagnosticFailureKind,
    verifier_failure_type: super::VerifierFailureType,
    preferred_repair_role: super::task_contract::ArtifactRole,
) -> super::VerifierDiagnosticFailureKind {
    if preferred_repair_role != super::task_contract::ArtifactRole::Test {
        return assessment_kind;
    }
    if !matches!(
        assessment_kind,
        super::VerifierDiagnosticFailureKind::DependencyMissing
            | super::VerifierDiagnosticFailureKind::ConfigOrVerifierError
    ) {
        return assessment_kind;
    }
    if matches!(
        verifier_failure_type,
        super::VerifierFailureType::AssertionFailure | super::VerifierFailureType::RuntimeError
    ) {
        return super::VerifierDiagnosticFailureKind::TestBug;
    }
    assessment_kind
}

pub(super) fn build_spec_authority_input_for_active_request(
    active_request: Option<&str>,
    semantic_report: Option<&super::semantic_failure::SemanticFailureReport>,
    agent_history_hint: super::spec_authority::AgentHistoryHint,
) -> super::spec_authority::SpecAuthorityInput {
    let has_behavior_contract = active_request
        .map(|request| {
            let contract = super::task_contract::TaskContract::from_request(request);
            super::required_behavior::behavior_contract_has_repair_authority(&contract)
        })
        .unwrap_or(false);
    let has_user_request_match = active_request
        .map(super::spec_authority::detect_explicit_spec_in_user_request)
        .unwrap_or(false);
    let has_verified_public_interface =
        super::spec_authority::detect_verified_public_interface_from_history(agent_history_hint);
    let consensus = semantic_report.and_then(|report| {
        super::spec_authority::detect_consensus_from_contract_conflict(
            &report.contract_conflict.implementation,
            &report.contract_conflict.test,
            &report.contract_conflict.usage_docs,
        )
    });
    super::spec_authority::SpecAuthorityInput {
        has_user_request_match,
        has_behavior_contract,
        has_verified_public_interface,
        is_newly_generated_task: true,
        consensus,
    }
}

#[cfg(test)]
pub(super) fn build_semantic_repair_plan_from_report(
    report: super::semantic_failure::SemanticFailureReport,
) -> Option<super::repair_job::SemanticRepairPlan> {
    build_semantic_repair_plan_from_report_with_authority_input(
        report,
        default_spec_authority_input(),
        0,
    )
}

#[cfg(test)]
pub(super) fn default_spec_authority_input() -> super::spec_authority::SpecAuthorityInput {
    super::spec_authority::SpecAuthorityInput {
        has_user_request_match: false,
        has_behavior_contract: false,
        has_verified_public_interface: false,
        is_newly_generated_task: true,
        consensus: None,
    }
}

pub(super) fn build_semantic_repair_plan_from_report_with_authority_input(
    report: super::semantic_failure::SemanticFailureReport,
    authority_input: super::spec_authority::SpecAuthorityInput,
    assessment_generation: u32,
) -> Option<super::repair_job::SemanticRepairPlan> {
    if super::semantic_failure::dispatch_target(&report)
        == super::semantic_failure::SemanticDispatchTarget::SetupRepair
    {
        return None;
    }
    let failure_cluster_id = super::repair_job::first_repairable_cluster(&report)
        .or_else(|| report.failure_clusters.first())?
        .cluster_key
        .clone();
    let spec_authority = super::spec_authority::resolve(&authority_input);
    Some(super::repair_job::SemanticRepairPlan {
        semantic_cause: report.failure_kind,
        spec_authority,
        preferred_repair_role: report.preferred_repair_role,
        repair_hypothesis: report.repair_hypothesis.clone(),
        failure_cluster_id,
        expected_improvement: None,
        semantic_report: report,
        assessment_generation_at_creation: assessment_generation,
    })
}
