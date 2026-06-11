//! Pure actor-loop phase decisions.
//!
//! This module keeps terminal projection decisions out of the actor-loop
//! dispatcher without changing runtime behavior or log payloads.

use super::summary::ExitReason;
use super::task_contract::{ArtifactRecoveryAction, SafeStopReason};

pub(super) fn task_contract_safe_stop_clear_tag(reason: SafeStopReason) -> &'static str {
    match reason {
        SafeStopReason::VerifierWeak => "task_contract_safe_stop_verifier_weak",
        SafeStopReason::VerifierMissing => "task_contract_safe_stop_verifier_missing",
    }
}

pub(super) fn task_contract_verifier_safe_stop_mapping(
    reason: SafeStopReason,
) -> (ExitReason, &'static str) {
    match reason {
        SafeStopReason::VerifierWeak => {
            (ExitReason::SafeStopVerifierWeak, "safe_stop_verifier_weak")
        }
        SafeStopReason::VerifierMissing => (
            ExitReason::SafeStopVerifierMissing,
            "safe_stop_verifier_missing",
        ),
    }
}

pub(super) fn task_contract_continue_requires_tool_recovery(
    action: Option<&ArtifactRecoveryAction>,
    current_reply_tool_calls: usize,
) -> bool {
    matches!(action, Some(ArtifactRecoveryAction::Continue { .. })) && current_reply_tool_calls == 0
}

pub(super) fn task_contract_action_completion_exit(
    action: Option<&ArtifactRecoveryAction>,
) -> Option<(ExitReason, String)> {
    match action {
        None | Some(ArtifactRecoveryAction::Done) => None,
        Some(
            ArtifactRecoveryAction::Continue { .. }
            | ArtifactRecoveryAction::RunVerifier
            | ArtifactRecoveryAction::RepairArtifact { .. },
        ) => Some((
            ExitReason::MissingRepoEdits,
            "task contract did not produce evidence-based completion".to_string(),
        )),
        Some(ArtifactRecoveryAction::SafeStop { reason, .. }) => {
            let (exit_reason, _) = task_contract_verifier_safe_stop_mapping(*reason);
            Some((exit_reason, exit_reason.default_error_text().to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::task_contract::ArtifactRole;

    #[test]
    fn completion_gate_allows_only_done_or_no_contract() {
        assert!(task_contract_action_completion_exit(None).is_none());
        assert!(
            task_contract_action_completion_exit(Some(&ArtifactRecoveryAction::Done)).is_none()
        );

        let continue_action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::Implementation],
            target_hint: None,
        };
        let (reason, text) = task_contract_action_completion_exit(Some(&continue_action))
            .expect("non-done contract action must block completion");
        assert_eq!(reason, ExitReason::MissingRepoEdits);
        assert!(text.contains("evidence-based completion"));
    }

    #[test]
    fn completion_gate_preserves_safe_stop_reason() {
        let action = ArtifactRecoveryAction::SafeStop {
            reason: SafeStopReason::VerifierMissing,
            weak_reason: None,
        };
        let (reason, text) = task_contract_action_completion_exit(Some(&action))
            .expect("safe stop must terminate completion");
        assert_eq!(reason, ExitReason::SafeStopVerifierMissing);
        assert_eq!(
            text,
            ExitReason::SafeStopVerifierMissing.default_error_text()
        );
    }

    #[test]
    fn safe_stop_tags_match_existing_payload_values() {
        assert_eq!(
            task_contract_safe_stop_clear_tag(SafeStopReason::VerifierWeak),
            "task_contract_safe_stop_verifier_weak"
        );
        assert_eq!(
            task_contract_safe_stop_clear_tag(SafeStopReason::VerifierMissing),
            "task_contract_safe_stop_verifier_missing"
        );
    }
}
