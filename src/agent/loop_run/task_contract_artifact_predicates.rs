//! Artifact completion and evidence predicates for TaskContract.
//!
//! This module owns role/path/content satisfaction checks. It keeps the main
//! contract module focused on admission and lifecycle assembly.

use super::completion_evidence::{CompletionEvidence, EvidenceSet};
use super::task_contract::{
    ArtifactExcerpts, ArtifactObligation, ArtifactRole, ArtifactState, DeliverableKind,
    DeliverableSchema, ObjectiveEvidenceKind, TaskContract, TaskKind,
    artifact_identity_path_ready_for_verification, is_deterministic_completion_authority_evidence,
    normalized_artifact_path_eq, observed_artifacts, role_from_repo_edit,
};

/// Whether the contract has any actionable behavior signal that can drive
/// the coverage gate. If neither `operations` nor `domain_terms` was
/// extracted, the gate is disabled and the legacy completion path runs.
pub(super) fn behavior_coverage_enabled(contract: &TaskContract) -> bool {
    contract.required_behavior.operations.is_some()
        || contract.required_behavior.domain_terms.is_some()
}

/// True when `excerpt` either hits any operation keyword or contains any
/// domain term. Both judgements stay behind the `required_behavior`
/// SSOT (DR1-005) so `KeywordMatch` / `OPERATION_KEYWORDS` never escape
/// the schema module.
pub(super) fn excerpt_satisfies_behavior(contract: &TaskContract, excerpt: &str) -> bool {
    contract
        .required_behavior
        .excerpt_hits_any_operation(excerpt)
        || contract
            .required_behavior
            .excerpt_hits_any_domain_term(excerpt)
}

fn implementation_excerpt_is_obviously_placeholder(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let markers = [
        "placeholder",
        "todo",
        "stub",
        "not implemented",
        "unimplemented",
        "dummy",
    ];
    markers.iter().any(|marker| lower.contains(marker))
}

fn implementation_excerpt_satisfies_completion(contract: &TaskContract, excerpt: &str) -> bool {
    if implementation_excerpt_is_obviously_placeholder(excerpt) {
        return false;
    }
    if excerpt_satisfies_behavior(contract, excerpt) {
        return true;
    }
    if implementation_excerpt_requires_behavior_hit(contract) {
        return false;
    }
    // Deterministic behavior labels are useful when they match, but they
    // are too brittle to be a hard multilingual semantic gate. The
    // verifier/repair pipeline owns semantic correctness after artifacts
    // exist; artifact completion only blocks obvious placeholder bodies.
    !excerpt.trim().is_empty()
}

/// At least two of {setup, run, verification} surface categories must
/// appear in the README excerpt for usage_docs to count as covered. One
/// category is too weak (scaffold READMEs that only mention `install`),
/// three is overly strict for minimal but honest docs.
fn usage_docs_surface_satisfied(excerpt: &str) -> bool {
    super::verifier::DocsVerifier.required_sections_pass(excerpt)
}

pub(super) fn usage_docs_excerpt_satisfies_obligations(
    contract: &TaskContract,
    excerpt: &str,
) -> bool {
    // Issue #922 (PR-001): a research `UsageDocs` obligation is evaluated by the
    // research acceptance predicate (sectioned coverage OR open-ended floor),
    // NOT the docs setup/run/verify surface gate. This keeps the recovery path
    // consistent with `verifier_diagnostic_for_obligation_parts` (DR3-002) so a
    // valid research report is not forced back to `Continue` here.
    if contract.task_kind == TaskKind::Research {
        let sections = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .into_iter()
            .flat_map(|identity| identity.required_sections.iter().cloned())
            .collect::<Vec<_>>();
        return super::verifier::assess_research_report(excerpt, &sections)
            .tier
            .is_accepted();
    }
    let section_obligations = contract
        .required_identities_for_role(ArtifactRole::UsageDocs)
        .into_iter()
        .flat_map(|identity| identity.required_sections.iter().cloned())
        .collect::<Vec<_>>();
    if section_obligations.is_empty() {
        return usage_docs_surface_satisfied(excerpt);
    }
    super::verifier::required_section_headings_present(excerpt, &section_obligations)
}

fn usage_docs_obligation_has_content_gate(identity: &ArtifactObligation) -> bool {
    !identity.required_sections.is_empty()
        || matches!(
            identity.schema.as_ref(),
            Some(DeliverableSchema::RequiredSections(_))
        )
        || matches!(
            identity.kind,
            DeliverableKind::ResearchNotes
                | DeliverableKind::OpsRunbook
                | DeliverableKind::CommandOutput
        )
}

pub(super) fn usage_docs_role_has_content_gate(contract: &TaskContract) -> bool {
    contract
        .required_identities_for_role(ArtifactRole::UsageDocs)
        .into_iter()
        .any(usage_docs_obligation_has_content_gate)
}

fn structured_record_excerpt_satisfies_obligations(contract: &TaskContract, excerpt: &str) -> bool {
    // Issue #921 (P4 / DD1 / DR2-004): route the completion side through the same
    // OR-tolerant SSOT the diagnostic side uses. Each schema-bearing DataOutput
    // obligation is assessed with ITS OWN `validated_obligation_path`-checked
    // path so non-CSV formats (.jsonl/.tsv/.json) get extension-based column
    // observation instead of the old CSV-comma heuristic (the intended
    // unification). The `path` reaches `assess_structured_data` only for
    // extension dispatch — never as an fs/log/prompt sink (DR4-001).
    let schema_identities = contract
        .required_identities_for_role(ArtifactRole::DataOutput)
        .into_iter()
        .filter(|identity| identity.structured_record_schema.is_some())
        .collect::<Vec<_>>();
    if schema_identities.is_empty() {
        // No declared schema: parse-ready non-empty content is accepted. This
        // subsumes the historical `!excerpt.trim().is_empty()` accept-tier;
        // with no columns + `path = None`, `assess_structured_data` returns
        // `SchemaSatisfied` for any non-empty excerpt.
        return super::verifier::assess_structured_data(None, excerpt, &[]).is_accepted();
    }
    schema_identities.iter().all(|identity| {
        let Some(schema) = identity.structured_record_schema.as_ref() else {
            return true;
        };
        super::verifier::structured_data_schema_obligation_pass_with_rows(
            &identity.path,
            excerpt,
            &schema.columns,
            &schema.expected_rows,
        )
    })
}

pub(super) fn role_deliverable_content_satisfied(
    contract: &TaskContract,
    artifact_excerpts: &ArtifactExcerpts,
    role: ArtifactRole,
) -> bool {
    if !behavior_coverage_enabled(contract) {
        return true;
    }
    let Some(excerpt) = artifact_excerpts.get(&role).map(String::as_str) else {
        return true;
    };
    match role {
        ArtifactRole::Implementation => {
            implementation_excerpt_satisfies_completion(contract, excerpt)
        }
        ArtifactRole::Test | ArtifactRole::Setup => true,
        ArtifactRole::UsageDocs => {
            contract.task_kind == TaskKind::Ops
                || command_observation_usage_docs_behavior_satisfied(contract)
                || usage_docs_excerpt_satisfies_obligations(contract, excerpt)
        }
        ArtifactRole::DataOutput => {
            structured_record_excerpt_satisfies_obligations(contract, excerpt)
        }
    }
}

fn implementation_excerpt_requires_behavior_hit(contract: &TaskContract) -> bool {
    let identities = contract.required_identities_for_role(ArtifactRole::Implementation);
    contract
        .required_behavior
        .domain_terms
        .as_ref()
        .is_some_and(|terms| {
            terms.iter().any(|term| {
                domain_term_is_code_like_contract(term)
                    && !domain_term_matches_required_identity_path(term, &identities)
            })
        })
}

fn domain_term_is_code_like_contract(term: &str) -> bool {
    let trimmed = term
        .trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation());
    !trimmed.is_empty()
        && trimmed.split_whitespace().count() == 1
        && (trimmed.contains('.') || trimmed.contains('_') || trimmed.contains('-'))
}

fn domain_term_matches_required_identity_path(
    term: &str,
    identities: &[&ArtifactObligation],
) -> bool {
    let term = term.trim_matches(|ch: char| ch.is_ascii_punctuation());
    identities
        .iter()
        .any(|identity| identity.path.as_str() == term)
}

pub(super) fn artifact_identity_satisfied_for_verification(
    contract: &TaskContract,
    evidence: &EvidenceSet,
    artifacts: &[ArtifactState],
    artifact_excerpts: &ArtifactExcerpts,
    identity: &ArtifactObligation,
) -> bool {
    // Issue #919: path-existence observation must still see a raw repo edit
    // (DR3-002 only governs *completion authority*, not whether the path exists);
    // the Authoring accept-tier is enforced by `verifier_diagnostic_for_obligation`.
    let completion_evidence_observed =
        artifact_identity_observed_in_evidence(evidence, identity, false);
    let path_exists = artifact_identity_path_ready_for_verification(artifacts, identity)
        || completion_evidence_observed;
    let excerpt = artifact_excerpts.get(&identity.role).map(String::as_str);
    if command_observation_file_identity_satisfied(contract, identity, path_exists) {
        return true;
    }
    if identity.role == ArtifactRole::DataOutput {
        return excerpt
            .map(|excerpt| data_output_identity_excerpt_satisfies(identity, excerpt))
            .unwrap_or_else(|| data_output_structured_evidence_observed(evidence, identity));
    }
    let Some(diagnostic) = super::verifier::verifier_diagnostic_for_obligation(
        contract.task_kind,
        identity,
        excerpt,
        path_exists,
    ) else {
        return true;
    };
    diagnostic.code == super::verifier::VerifierDiagnosticCode::EvidenceMissing
        && excerpt.is_none()
        && path_exists
}

fn command_observation_file_identity_satisfied(
    contract: &TaskContract,
    identity: &ArtifactObligation,
    path_exists: bool,
) -> bool {
    let objective = contract.objective_contract();
    objective.evidence_kind == ObjectiveEvidenceKind::SafetyBoundaryEvidence
        && path_exists
        && identity.schema.is_none()
        && identity.required_sections.is_empty()
}

fn command_observation_usage_docs_behavior_satisfied(contract: &TaskContract) -> bool {
    let objective = contract.objective_contract();
    let identities = contract.required_identities_for_role(ArtifactRole::UsageDocs);
    objective.evidence_kind == ObjectiveEvidenceKind::SafetyBoundaryEvidence
        && !identities.is_empty()
        && identities
            .iter()
            .all(|identity| identity.schema.is_none() && identity.required_sections.is_empty())
}

fn data_output_identity_excerpt_satisfies(identity: &ArtifactObligation, excerpt: &str) -> bool {
    let Some(schema) = identity.structured_record_schema.as_ref() else {
        return super::verifier::assess_structured_data(Some(&identity.path), excerpt, &[])
            .is_accepted();
    };
    if schema.columns.is_empty() && schema.expected_rows.is_empty() {
        return super::verifier::assess_structured_data(Some(&identity.path), excerpt, &[])
            .is_accepted();
    }
    super::verifier::structured_data_schema_obligation_pass_with_rows(
        &identity.path,
        excerpt,
        &schema.columns,
        &schema.expected_rows,
    )
}

fn data_output_structured_evidence_observed(
    evidence: &EvidenceSet,
    identity: &ArtifactObligation,
) -> bool {
    evidence.iter().any(|item| match item {
        CompletionEvidence::StructuredDataPass {
            path: Some(path),
            columns,
        } => {
            normalized_artifact_path_eq(path, &identity.path)
                && identity
                    .structured_record_schema
                    .as_ref()
                    .is_none_or(|schema| {
                        if !schema.expected_rows.is_empty() {
                            return false;
                        }
                        schema
                            .columns
                            .iter()
                            .all(|column| columns.iter().any(|observed| observed == column))
                    })
        }
        _ => false,
    })
}

pub(super) fn required_role_satisfied_by_evidence(
    contract: &TaskContract,
    evidence: &EvidenceSet,
    role: ArtifactRole,
) -> bool {
    // Issue #919 (DR3-002): for an Authoring contract the UsageDocs role is
    // completion authority ONLY through a path-matched accept-tier pass
    // (`ReportCompletenessPass`/`RequiredSectionsPass`). Raw `RepoEdit(Docs)` is
    // existence/progress evidence, not completion authority, so we must NOT take
    // either the `observed_artifacts` short-circuit (which `RepoEdit(Docs)`
    // trips) nor allow `RepoEdit(Docs)` to satisfy an identity.
    let authoring = contract.task_kind == TaskKind::Authoring;
    let identities = contract.required_identities_for_role(role);
    if identities.is_empty() {
        if authoring && role == ArtifactRole::UsageDocs {
            return false;
        }
        return observed_artifacts(evidence).contains(&role);
    }
    // The evaluate() completion authority must NOT let the docs observed-role
    // shortcut satisfy a UsageDocs role for any kind whose UsageDocs obligation
    // carries a content gate — fall through to the path-specific
    // `artifact_identity_observed_in_evidence` instead:
    //  - Research (#922 DR3-001): an unconditional `ReportCompletenessPass{path:None}`
    //    must not falsely satisfy the section / open-ended-floor check.
    //  - Authoring (#919 DR3-002): raw `RepoEdit(Docs)` is not completion authority;
    //    needs the accept-tier pass.
    //  - Ops (#923): the OpsRunbook obligation is the authority; needs the tier predicate.
    let usage_docs_content_gate =
        role == ArtifactRole::UsageDocs && usage_docs_role_has_content_gate(contract);
    if role == ArtifactRole::UsageDocs
        && !usage_docs_content_gate
        && contract.task_kind != TaskKind::Research
        && !authoring
        && contract.task_kind != TaskKind::Ops
        && observed_artifacts(evidence).contains(&role)
    {
        return true;
    }
    identities
        .iter()
        .all(|identity| artifact_identity_observed_in_evidence(evidence, identity, authoring))
}

fn artifact_identity_observed_in_evidence(
    evidence: &EvidenceSet,
    identity: &ArtifactObligation,
    authoring: bool,
) -> bool {
    evidence
        .iter()
        .filter(|item| is_deterministic_completion_authority_evidence(item))
        .any(|item| match item {
            CompletionEvidence::RepoEdit {
                category,
                path: Some(path),
                ..
            } => {
                // DR3-002: a raw repo edit never satisfies an Authoring UsageDocs
                // identity — the accept tier must observe a path-matched pass.
                !(authoring && identity.role == ArtifactRole::UsageDocs)
                    && role_from_repo_edit(*category) == Some(identity.role)
                    && normalized_artifact_path_eq(path, &identity.path)
            }
            CompletionEvidence::RequiredSectionsPass { path: Some(path) } => {
                identity.role == ArtifactRole::UsageDocs
                    && normalized_artifact_path_eq(path, &identity.path)
            }
            CompletionEvidence::StructuredDataPass {
                path: Some(path),
                columns,
            } => {
                identity.role == ArtifactRole::DataOutput
                    && normalized_artifact_path_eq(path, &identity.path)
                    && identity
                        .structured_record_schema
                        .as_ref()
                        .is_none_or(|schema| {
                            if !schema.expected_rows.is_empty() {
                                return false;
                            }
                            schema
                                .columns
                                .iter()
                                .all(|column| columns.iter().any(|observed| observed == column))
                        })
            }
            CompletionEvidence::ReportCompletenessPass { path: Some(path) } => {
                identity.role == ArtifactRole::UsageDocs
                    && normalized_artifact_path_eq(path, &identity.path)
            }
            _ => false,
        })
}
