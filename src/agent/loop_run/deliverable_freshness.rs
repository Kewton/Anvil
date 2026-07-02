//! Deliverable freshness policy for contract recovery.
//!
//! This module keeps freshness rules as a pure projection over typed contract
//! roles and observed artifact states. It deliberately does not inspect raw
//! prompts or stack-specific file names.

use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRole, ArtifactState, ArtifactStateKind, RecoveryTargetHint,
    TaskContract,
};

pub(super) fn stale_supporting_deliverable_action(
    contract: &TaskContract,
    artifacts: &[ArtifactState],
) -> Option<ArtifactRecoveryAction> {
    let required = contract.objective_contract();
    let required_roles = required.required_deliverables();
    if !required_roles.contains(&ArtifactRole::Implementation)
        || !required_roles.contains(&ArtifactRole::Test)
        || !artifact_changed_this_turn(artifacts, ArtifactRole::Implementation)
        || artifact_changed_this_turn(artifacts, ArtifactRole::Test)
    {
        return None;
    }

    let target_path = contract
        .required_identities_for_role(ArtifactRole::Test)
        .first()
        .map(|identity| identity.path.clone())
        .or_else(|| {
            artifacts
                .iter()
                .find(|artifact| artifact.role == ArtifactRole::Test)
                .and_then(|artifact| artifact.path.clone())
        })?;
    Some(ArtifactRecoveryAction::Continue {
        missing: vec![ArtifactRole::Test],
        target_hint: Some(RecoveryTargetHint {
            role: ArtifactRole::Test,
            path: target_path,
            reason: "implementation changed this turn; required supporting test artifact must be updated for the current contract".to_string(),
        }),
    })
}

fn artifact_changed_this_turn(artifacts: &[ArtifactState], role: ArtifactRole) -> bool {
    artifacts.iter().any(|artifact| {
        artifact.role == role && matches!(artifact.kind, ArtifactStateKind::ChangedThisTurn)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementation_change_requires_fresh_supporting_test_update() {
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Improve calculator source and extend tests","next_required_action":"artifact","required_artifacts":[{"path":"calculator.py","role":"source"},{"path":"tests/test_calculator.py","role":"test"}],"evidence_command":"python3 -m unittest discover -s tests"}"#
        );
        let contract = TaskContract::from_request(request);
        let artifacts = vec![
            ArtifactState::changed_at(ArtifactRole::Implementation, "calculator.py"),
            ArtifactState::exists(ArtifactRole::Test, "tests/test_calculator.py"),
        ];

        let action = stale_supporting_deliverable_action(&contract, &artifacts);

        assert!(
            matches!(
                action,
                Some(ArtifactRecoveryAction::Continue {
                    ref missing,
                    target_hint: Some(RecoveryTargetHint {
                        role: ArtifactRole::Test,
                        ref path,
                        ..
                    }),
                }) if missing == &vec![ArtifactRole::Test] && path == "tests/test_calculator.py"
            ),
            "implementation freshness must route to test update: {action:?}"
        );
    }

    #[test]
    fn supporting_test_freshness_is_satisfied_after_test_edit() {
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Improve calculator source and extend tests","next_required_action":"artifact","required_artifacts":[{"path":"calculator.py","role":"source"},{"path":"tests/test_calculator.py","role":"test"}],"evidence_command":"python3 -m unittest discover -s tests"}"#
        );
        let contract = TaskContract::from_request(request);
        let artifacts = vec![
            ArtifactState::changed_at(ArtifactRole::Implementation, "calculator.py"),
            ArtifactState::changed_at(ArtifactRole::Test, "tests/test_calculator.py"),
        ];

        assert!(stale_supporting_deliverable_action(&contract, &artifacts).is_none());
    }

    #[test]
    fn non_coding_data_deliverable_does_not_request_tests() {
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Create summary JSON","next_required_action":"artifact","required_artifacts":[{"path":"summary.json","role":"data"}]}"#
        );
        let contract = TaskContract::from_request(request);
        let artifacts = vec![ArtifactState::changed_at(
            ArtifactRole::DataOutput,
            "summary.json",
        )];

        assert!(stale_supporting_deliverable_action(&contract, &artifacts).is_none());
    }
}
