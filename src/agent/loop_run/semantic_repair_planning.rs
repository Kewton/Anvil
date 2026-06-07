use std::path::Path;

use super::repair_target_admission::RepairTargetAdmissionContext;
use super::verifier_assessment_parser::ParsedVerifierRepairAssessment;
use super::verifier_repair_targeting::recovery_target_hint_for_diagnostic_path;

pub(super) fn merge_legacy_targets_into_clusters(
    report: &mut super::semantic_failure::SemanticFailureReport,
    parsed: &ParsedVerifierRepairAssessment,
) {
    let all_empty = report
        .failure_clusters
        .iter()
        .all(|c| c.proposed_target_candidates.is_empty());
    if !all_empty {
        return;
    }
    let Some(first_cluster) = report.failure_clusters.first_mut() else {
        return;
    };
    for target in &parsed.repair_targets {
        if first_cluster.proposed_target_candidates.len()
            >= super::semantic_failure::MAX_PROPOSED_TARGETS_PER_CLUSTER
        {
            break;
        }
        first_cluster.proposed_target_candidates.push(
            super::semantic_failure::RawClusterTargetCandidate {
                raw_path: super::repair_job::sanitize_repair_job_text_with_char_cap(
                    &target.path,
                    super::semantic_failure::MAX_RAW_PATH_CHARS,
                ),
                role_hint: None,
                reason: super::repair_job::sanitize_repair_job_text_with_char_cap(
                    &target.reason,
                    240,
                ),
            },
        );
    }
    for entry in &parsed.repair_plan {
        if first_cluster.proposed_target_candidates.len()
            >= super::semantic_failure::MAX_PROPOSED_TARGETS_PER_CLUSTER
        {
            break;
        }
        let sanitized_path = super::repair_job::sanitize_repair_job_text_with_char_cap(
            &entry.path,
            super::semantic_failure::MAX_RAW_PATH_CHARS,
        );
        if first_cluster
            .proposed_target_candidates
            .iter()
            .any(|c| c.raw_path == sanitized_path)
        {
            continue;
        }
        first_cluster.proposed_target_candidates.push(
            super::semantic_failure::RawClusterTargetCandidate {
                raw_path: sanitized_path,
                role_hint: None,
                reason: super::repair_job::sanitize_repair_job_text_with_char_cap(
                    &entry.reason,
                    240,
                ),
            },
        );
    }
    for path in &parsed.secondary_targets {
        if first_cluster.proposed_target_candidates.len()
            >= super::semantic_failure::MAX_PROPOSED_TARGETS_PER_CLUSTER
        {
            break;
        }
        let sanitized_path = super::repair_job::sanitize_repair_job_text_with_char_cap(
            path,
            super::semantic_failure::MAX_RAW_PATH_CHARS,
        );
        if first_cluster
            .proposed_target_candidates
            .iter()
            .any(|c| c.raw_path == sanitized_path)
        {
            continue;
        }
        first_cluster.proposed_target_candidates.push(
            super::semantic_failure::RawClusterTargetCandidate {
                raw_path: sanitized_path,
                role_hint: None,
                reason: "diagnostic secondary target".to_string(),
            },
        );
    }
}

pub(super) fn sort_admitted_by_authority_role_priority(
    admitted: &mut [super::task_contract::RecoveryTargetHint],
    spec_authority: super::spec_authority::SpecAuthority,
    failure_kind: super::VerifierDiagnosticFailureKind,
) {
    let primary_rank_for = |role: super::task_contract::ArtifactRole| -> u8 {
        if matches!(failure_kind, super::VerifierDiagnosticFailureKind::TestBug) {
            return match role {
                super::task_contract::ArtifactRole::Test => 0,
                super::task_contract::ArtifactRole::Implementation => 1,
                super::task_contract::ArtifactRole::UsageDocs => 2,
                super::task_contract::ArtifactRole::Setup => 3,
                super::task_contract::ArtifactRole::DataOutput => 4,
            };
        }
        if matches!(
            failure_kind,
            super::VerifierDiagnosticFailureKind::DependencyMissing
                | super::VerifierDiagnosticFailureKind::ConfigOrVerifierError
        ) {
            return match role {
                super::task_contract::ArtifactRole::Setup => 0,
                super::task_contract::ArtifactRole::Implementation => 1,
                super::task_contract::ArtifactRole::UsageDocs => 2,
                super::task_contract::ArtifactRole::Test => 3,
                super::task_contract::ArtifactRole::DataOutput => 4,
            };
        }
        if matches!(
            failure_kind,
            super::VerifierDiagnosticFailureKind::AssertionMismatch
        ) {
            return match spec_authority {
                super::spec_authority::SpecAuthority::UserRequest
                | super::spec_authority::SpecAuthority::BehaviorContract => match role {
                    super::task_contract::ArtifactRole::Implementation => 0,
                    super::task_contract::ArtifactRole::Test => 1,
                    super::task_contract::ArtifactRole::UsageDocs => 2,
                    super::task_contract::ArtifactRole::Setup => 3,
                    super::task_contract::ArtifactRole::DataOutput => 4,
                },
                super::spec_authority::SpecAuthority::VerifiedPublicInterface
                | super::spec_authority::SpecAuthority::ImplementationContract
                | super::spec_authority::SpecAuthority::LlmGeneratedTest => match role {
                    super::task_contract::ArtifactRole::Test => 0,
                    super::task_contract::ArtifactRole::Implementation => 1,
                    super::task_contract::ArtifactRole::UsageDocs => 2,
                    super::task_contract::ArtifactRole::Setup => 3,
                    super::task_contract::ArtifactRole::DataOutput => 4,
                },
            };
        }
        match role {
            super::task_contract::ArtifactRole::Implementation => 0,
            super::task_contract::ArtifactRole::UsageDocs => 1,
            super::task_contract::ArtifactRole::Setup => 2,
            super::task_contract::ArtifactRole::Test => 3,
            super::task_contract::ArtifactRole::DataOutput => 4,
        }
    };
    admitted.sort_by(|a, b| {
        let pa = primary_rank_for(a.role);
        let pb = primary_rank_for(b.role);
        pa.cmp(&pb).then_with(|| a.path.cmp(&b.path))
    });
}

/// Issue #647 / CB-017 A''' (Commit 3, CR-1 V2 / CR-3 V2 / CR-5):
/// for every failure cluster, admit each `proposed_target_candidate` via
/// the SSOT `recovery_target_hint_for_diagnostic_path` (which composes
/// syntactic safety + Owned admission + setup-target gating), dedup the
/// admitted hints by `(role, path)`, and sort them with the
/// authority/failure-kind decision table in
/// [`sort_admitted_by_authority_role_priority`].
///
/// CR-3 V2: `candidate.role_hint` is **advisory only** — it is never passed
/// to the admission helper. The admitted hint's role is decided by
/// `recovery_target_hint_for_existing_path`'s path classification.
///
/// CR-1 V2: the admission SSOT is invoked exactly once per candidate. We do
/// NOT additionally call `admit_repair_target_hint` after the fact (that
/// would double-gate Owned).
pub(super) fn enrich_failure_clusters_with_admitted_targets(
    report: &mut super::semantic_failure::SemanticFailureReport,
    work_root: &Path,
    admission_ctx: &RepairTargetAdmissionContext<'_>,
    spec_authority: super::spec_authority::SpecAuthority,
) {
    let failure_kind = report.failure_kind;
    for cluster in report.failure_clusters.iter_mut() {
        let mut admitted: Vec<super::task_contract::RecoveryTargetHint> = Vec::new();
        for candidate in cluster.proposed_target_candidates.iter() {
            // recovery_target_hint_for_diagnostic_path is the SSOT — it
            // composes syntactic safety + Owned admission via
            // admit_repair_target_hint. role_hint is intentionally not
            // forwarded (CR-3 V2).
            if let Some(hint) = recovery_target_hint_for_diagnostic_path(
                work_root,
                &candidate.raw_path,
                &candidate.reason,
                failure_kind,
                admission_ctx,
            ) {
                admitted.push(hint);
            }
        }
        // (role, path) dedup — Codex nice-to-have 2.
        admitted.sort_by(|a, b| (a.role, &a.path).cmp(&(b.role, &b.path)));
        admitted.dedup_by(|a, b| a.role == b.role && a.path == b.path);
        // Role priority sort (TestBug override / SetupRepair defensive /
        // default impl-first). Stable sort with (rank, path) tie-breaker.
        sort_admitted_by_authority_role_priority(&mut admitted, spec_authority, failure_kind);
        cluster.admitted_cluster_targets = admitted;
    }
}

pub(super) fn diagnostic_target_allowed_by_confidence(
    hint: &super::task_contract::RecoveryTargetHint,
    confidence: f64,
    failure_kind: super::VerifierDiagnosticFailureKind,
    probable_cause_role: Option<super::task_contract::ArtifactRole>,
    do_not_edit_tests_without_evidence: bool,
) -> bool {
    if hint.role != super::task_contract::ArtifactRole::Test || !do_not_edit_tests_without_evidence
    {
        return true;
    }
    if matches!(failure_kind, super::VerifierDiagnosticFailureKind::TestBug)
        || probable_cause_role == Some(super::task_contract::ArtifactRole::Test)
    {
        return confidence >= 0.60;
    }
    confidence >= 0.85
}

pub(super) fn first_role_kind_compatible_diagnostic_target(
    repair_plan: &[super::task_contract::RecoveryTargetHint],
    repair_candidates: &[(super::task_contract::RecoveryTargetHint, f64)],
    secondary_repair_candidates: &[super::task_contract::RecoveryTargetHint],
    changed_repair_candidates: &[super::task_contract::RecoveryTargetHint],
    failure_kind: super::VerifierDiagnosticFailureKind,
) -> Option<super::task_contract::RecoveryTargetHint> {
    repair_plan
        .iter()
        .chain(repair_candidates.iter().map(|(hint, _)| hint))
        .chain(secondary_repair_candidates.iter())
        .chain(changed_repair_candidates.iter())
        .find(|hint| diagnostic_target_role_matches_failure_kind(hint, failure_kind))
        .cloned()
}

fn diagnostic_target_role_matches_failure_kind(
    hint: &super::task_contract::RecoveryTargetHint,
    failure_kind: super::VerifierDiagnosticFailureKind,
) -> bool {
    let kind = super::repair_brief::legacy_kind_to_allowed_change_kind_for_role(
        failure_kind.as_str(),
        Some(hint.role),
    );
    if kind == super::repair_brief::AllowedChangeKind::InsufficientEvidence {
        return true;
    }
    super::repair_action::allowed_change_kind_allows_target_role(kind, hint.role)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(
        role: super::super::task_contract::ArtifactRole,
        path: &str,
    ) -> super::super::task_contract::RecoveryTargetHint {
        super::super::task_contract::RecoveryTargetHint {
            role,
            path: path.to_string(),
            reason: "test".to_string(),
        }
    }

    #[test]
    fn invalid_manifest_diagnostic_allows_setup_target_role() {
        let setup = hint(
            super::super::task_contract::ArtifactRole::Setup,
            "Cargo.toml",
        );
        let implementation = hint(
            super::super::task_contract::ArtifactRole::Implementation,
            "src/lib.rs",
        );

        assert!(diagnostic_target_role_matches_failure_kind(
            &setup,
            super::super::VerifierDiagnosticFailureKind::InvalidManifest
        ));
        assert!(diagnostic_target_role_matches_failure_kind(
            &implementation,
            super::super::VerifierDiagnosticFailureKind::InvalidManifest
        ));

        let selected = first_role_kind_compatible_diagnostic_target(
            std::slice::from_ref(&setup),
            &[],
            &[],
            &[implementation],
            super::super::VerifierDiagnosticFailureKind::InvalidManifest,
        )
        .expect("setup target remains selectable");

        assert_eq!(
            selected.role,
            super::super::task_contract::ArtifactRole::Setup
        );
        assert_eq!(selected.path, "Cargo.toml");
    }
}
