//! Recovery target alignment helpers.
//!
//! This module keeps request/runtime-aware target-path normalization out of the
//! artifact target installer. It does not install jobs or mutate `Agent`.

use super::task_contract::{
    ArtifactObligation, ArtifactRole, RecoveryTargetHint,
    explicit_artifact_obligations_from_request,
};
use super::verifier_orchestration::{
    synthesized_missing_test_target_path_for_request, test_target_path_compatible_with_request,
};

pub(super) fn align_recovery_target_hint_to_request(
    request: Option<&str>,
    contract_identities: &[ArtifactObligation],
    mut hint: RecoveryTargetHint,
) -> RecoveryTargetHint {
    if hint.role != ArtifactRole::Test {
        return hint;
    }
    if let Some(identity) = contract_identities
        .iter()
        .find(|identity| identity.role == ArtifactRole::Test)
        && hint.path != identity.path
    {
        hint.path = identity.path.clone();
        hint.reason = "contract required test artifact identity is still missing".to_string();
        return hint;
    }
    let Some(request) = request else {
        return hint;
    };
    if let Some(explicit_path) = explicit_artifact_obligations_from_request(request)
        .into_iter()
        .find(|identity| identity.role == ArtifactRole::Test)
        .map(|identity| identity.path)
        && hint.path != explicit_path
    {
        hint.path = explicit_path;
        hint.reason = "explicit requested test artifact identity is still missing".to_string();
        return hint;
    }
    let Some((target_path, stack_label)) =
        synthesized_missing_test_target_path_for_request(request)
    else {
        return hint;
    };
    if hint.path == target_path || test_target_path_compatible_with_request(&hint.path, request) {
        return hint;
    }
    hint.path = target_path.to_string();
    hint.reason =
        format!("synthesized test artifact aligned with requested {stack_label} project family");
    hint
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(role: ArtifactRole, path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role,
            path: path.to_string(),
            reason: "missing target".to_string(),
        }
    }

    #[test]
    fn non_test_target_is_not_aligned() {
        let aligned = align_recovery_target_hint_to_request(
            Some("Create app.py and tests/test_app.py"),
            &[],
            hint(ArtifactRole::Implementation, "main.py"),
        );

        assert_eq!(aligned.path, "main.py");
        assert_eq!(aligned.reason, "missing target");
    }

    #[test]
    fn explicit_test_identity_precedes_synthesized_family_default() {
        let aligned = align_recovery_target_hint_to_request(
            Some("Create math_utils.py and tests/test_math_utils.py with Python unittest tests."),
            &[],
            hint(ArtifactRole::Test, "tests/cli.rs"),
        );

        assert_eq!(aligned.path, "tests/test_math_utils.py");
        assert_eq!(
            aligned.reason,
            "explicit requested test artifact identity is still missing"
        );
    }

    #[test]
    fn explicit_test_identity_precedes_synthesized_family_default_for_tdd_prompt() {
        let prompt = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum): return minimum when value is below minimum, maximum when value is above maximum, otherwise value. Use Python unittest and verify with python -m unittest discover -s tests. Do not create README, package.json, Cargo.toml, or setup files.";
        let aligned = align_recovery_target_hint_to_request(
            Some(prompt),
            &[],
            hint(ArtifactRole::Test, "tests/cli.rs"),
        );

        assert_eq!(aligned.path, "tests/test_math_utils.py");
        assert_eq!(
            aligned.reason,
            "explicit requested test artifact identity is still missing"
        );
    }

    #[test]
    fn contract_test_identity_precedes_request_family_default() {
        let identities = vec![ArtifactObligation::file(
            ArtifactRole::Test,
            "tests/test_math_utils.py",
        )];
        let aligned = align_recovery_target_hint_to_request(
            Some("Create a Rust CLI and verify with cargo test."),
            &identities,
            hint(ArtifactRole::Test, "tests/cli.rs"),
        );

        assert_eq!(aligned.path, "tests/test_math_utils.py");
        assert_eq!(
            aligned.reason,
            "contract required test artifact identity is still missing"
        );
    }

    #[test]
    fn incompatible_test_target_uses_synthesized_default_without_explicit_identity() {
        let aligned = align_recovery_target_hint_to_request(
            Some("Create a Python module with unittest tests."),
            &[],
            hint(ArtifactRole::Test, "tests/cli.rs"),
        );

        assert_eq!(aligned.path, "tests/test_main.py");
        assert!(aligned.reason.contains("python"));
    }

    #[test]
    fn compatible_test_target_is_preserved() {
        let aligned = align_recovery_target_hint_to_request(
            Some("Create src/lib.rs and tests/lib.rs. Verify with cargo test."),
            &[],
            hint(ArtifactRole::Test, "tests/lib.rs"),
        );

        assert_eq!(aligned.path, "tests/lib.rs");
        assert_eq!(aligned.reason, "missing target");
    }
}
