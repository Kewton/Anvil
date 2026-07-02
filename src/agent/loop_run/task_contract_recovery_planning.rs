//! Pure TaskContract recovery-target planning.
//!
//! This module keeps target selection separate from contract construction. It
//! does not inspect prompts or agent state; it only converts typed contract
//! obligations, artifact states, and verifier diagnostics into a recovery target.

use super::task_contract::{
    ArtifactExcerpts, ArtifactRole, ArtifactState, ArtifactStateKind, RecoveryTargetHint,
    TaskContract, artifact_identity_path_ready_for_verification, obligation_report_label,
};

pub(super) fn order_missing_deliverables_for_recovery(missing: &mut [ArtifactRole]) {
    missing.sort_by_key(|role| match role {
        ArtifactRole::Setup => 0,
        ArtifactRole::Implementation => 1,
        ArtifactRole::Test => 2,
        ArtifactRole::DataOutput => 3,
        ArtifactRole::UsageDocs => 4,
    });
}

pub(super) fn recovery_target_hint_for_missing_with_contract(
    contract: &TaskContract,
    artifacts: &[ArtifactState],
    artifact_excerpts: &ArtifactExcerpts,
    missing: &[ArtifactRole],
) -> Option<RecoveryTargetHint> {
    let role = missing.first().copied()?;
    if let Some(target_hint) = recovery_target_hint_for_blocking_obligation_diagnostic(
        contract,
        artifacts,
        artifact_excerpts,
        role,
    ) {
        return Some(target_hint);
    }
    if let Some(identity) = contract.required_identities_for_role(role).first() {
        return Some(RecoveryTargetHint {
            role,
            path: identity.path.clone(),
            reason: format!(
                "required deliverable obligation is still missing: {}",
                obligation_report_label(identity)
            ),
        });
    }
    recovery_target_hint_for_missing(artifacts, missing)
}

fn recovery_target_hint_for_missing(
    artifacts: &[ArtifactState],
    missing: &[ArtifactRole],
) -> Option<RecoveryTargetHint> {
    let role = missing.first().copied()?;
    if let Some(scaffold_hint) = artifacts
        .iter()
        .find(|artifact| {
            artifact.role == role && artifact.kind == ArtifactStateKind::ScaffoldUnchanged
        })
        .and_then(|artifact| {
            artifact.path.as_ref().map(|path| RecoveryTargetHint {
                role,
                path: path.clone(),
                reason: "bootstrap scaffold artifact for the missing role is still unchanged"
                    .to_string(),
            })
        })
    {
        return Some(scaffold_hint);
    }
    synthesized_missing_role_target_hint(artifacts, role)
}

pub(super) fn recovery_target_hint_for_blocking_obligation_diagnostic(
    contract: &TaskContract,
    artifacts: &[ArtifactState],
    artifact_excerpts: &ArtifactExcerpts,
    role: ArtifactRole,
) -> Option<RecoveryTargetHint> {
    blocking_obligation_diagnostic_for_role(contract, artifacts, artifact_excerpts, role)
        .map(|diagnostic| diagnostic.target_hint)
}

pub(super) struct BlockingObligationDiagnostic {
    pub(super) target_hint: RecoveryTargetHint,
    pub(super) code: super::verifier::VerifierDiagnosticCode,
}

pub(super) fn blocking_obligation_diagnostic_for_role(
    contract: &TaskContract,
    artifacts: &[ArtifactState],
    artifact_excerpts: &ArtifactExcerpts,
    role: ArtifactRole,
) -> Option<BlockingObligationDiagnostic> {
    contract
        .required_identities_for_role(role)
        .into_iter()
        .filter_map(|identity| {
            let path_exists = artifact_identity_path_ready_for_verification(artifacts, identity);
            let excerpt = artifact_excerpts.get(&identity.role).map(String::as_str);
            let diagnostic = super::verifier::verifier_diagnostic_for_obligation(
                contract.task_kind,
                identity,
                excerpt,
                path_exists,
            )?;
            if diagnostic.code == super::verifier::VerifierDiagnosticCode::EvidenceMissing
                && excerpt.is_none()
                && path_exists
                && role != ArtifactRole::DataOutput
            {
                return None;
            }
            let reason = if diagnostic.code == super::verifier::VerifierDiagnosticCode::MissingFile
            {
                format!(
                    "required deliverable obligation is still missing: {}",
                    obligation_report_label(identity)
                )
            } else {
                diagnostic.reason()
            };
            Some(BlockingObligationDiagnostic {
                target_hint: RecoveryTargetHint {
                    role,
                    path: identity.path.clone(),
                    reason,
                },
                code: diagnostic.code,
            })
        })
        .next()
}

fn synthesized_missing_role_target_hint(
    artifacts: &[ArtifactState],
    role: ArtifactRole,
) -> Option<RecoveryTargetHint> {
    let path = match role {
        ArtifactRole::Test => synthesized_test_target_path(artifacts)?,
        ArtifactRole::UsageDocs => "README.md".to_string(),
        ArtifactRole::DataOutput => "output.csv".to_string(),
        _ => return None,
    };
    Some(RecoveryTargetHint {
        role,
        path,
        reason: "no existing artifact for the missing role; create a conventional artifact path"
            .to_string(),
    })
}

fn synthesized_test_target_path(artifacts: &[ArtifactState]) -> Option<String> {
    let impl_path = artifacts
        .iter()
        .find(|artifact| {
            artifact.role == ArtifactRole::Implementation
                && matches!(
                    artifact.kind,
                    ArtifactStateKind::ExistsButUnverified
                        | ArtifactStateKind::ChangedThisTurn
                        | ArtifactStateKind::Verified
                )
        })
        .and_then(|artifact| artifact.path.as_deref());
    let Some(path) = impl_path else {
        return Some("tests/test_main.py".to_string());
    };
    let stem = sanitized_file_stem(path).unwrap_or("main");
    if path.ends_with(".rs") {
        Some(format!("tests/{stem}.rs"))
    } else if path.ends_with(".ts") || path.ends_with(".tsx") {
        Some(format!("tests/{stem}.test.ts"))
    } else if path.ends_with(".js") || path.ends_with(".jsx") {
        Some(format!("tests/{stem}.test.js"))
    } else {
        Some(format!("tests/test_{stem}.py"))
    }
}

fn sanitized_file_stem(path: &str) -> Option<&str> {
    let file_name = path.rsplit('/').next()?.rsplit('\\').next()?;
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_impl(path: &str) -> ArtifactState {
        ArtifactState::changed_at(ArtifactRole::Implementation, path)
    }

    #[test]
    fn orders_missing_deliverables_for_lifecycle_recovery() {
        let mut missing = vec![
            ArtifactRole::UsageDocs,
            ArtifactRole::DataOutput,
            ArtifactRole::Test,
            ArtifactRole::Implementation,
            ArtifactRole::Setup,
        ];

        order_missing_deliverables_for_recovery(&mut missing);

        assert_eq!(
            missing,
            vec![
                ArtifactRole::Setup,
                ArtifactRole::Implementation,
                ArtifactRole::Test,
                ArtifactRole::DataOutput,
                ArtifactRole::UsageDocs,
            ]
        );
    }

    #[test]
    fn synthesized_test_target_uses_implementation_shape() {
        assert_eq!(
            recovery_target_hint_for_missing(&[changed_impl("src/lib.rs")], &[ArtifactRole::Test])
                .map(|hint| hint.path),
            Some("tests/lib.rs".to_string())
        );
        assert_eq!(
            recovery_target_hint_for_missing(
                &[changed_impl("src/index.ts")],
                &[ArtifactRole::Test]
            )
            .map(|hint| hint.path),
            Some("tests/index.test.ts".to_string())
        );
        assert_eq!(
            recovery_target_hint_for_missing(&[changed_impl("calc.py")], &[ArtifactRole::Test])
                .map(|hint| hint.path),
            Some("tests/test_calc.py".to_string())
        );
    }

    #[test]
    fn scaffold_target_wins_before_conventional_target() {
        let hint = recovery_target_hint_for_missing(
            &[
                changed_impl("calc.py"),
                ArtifactState::scaffold(ArtifactRole::Test, "tests/test_existing.py"),
            ],
            &[ArtifactRole::Test],
        )
        .expect("hint");

        assert_eq!(hint.path, "tests/test_existing.py");
        assert!(hint.reason.contains("bootstrap scaffold"));
    }
}
