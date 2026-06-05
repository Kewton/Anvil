//! Issue #950: controller policy for delegated local-LLM persistence.
//!
//! The policy is intentionally small and local-first. It records only static
//! strategy labels selected by the controller, never raw commands, paths, tool
//! arguments, verifier output, or approval decisions.

use super::task_contract::ArtifactRecoveryAction;

pub(super) const MIN_DISTINCT_RECOVERY_STRATEGIES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControllerRecoveryStrategy {
    ToolFirstRetry,
    TargetedArtifactRetry,
    DeterministicFallback,
    /// Issue #991: a deterministic Rust binding-mismatch operator fired (lib
    /// name / `CARGO_BIN_EXE`). Distinct from `DeterministicFallback` so the
    /// eval/report operator hit rate is attributable to binding repair, while
    /// still matching the `deterministic` hit-rate marker.
    DeterministicBindingRepair,
    EvidenceAction,
    VerifierRepairEdit,
    MissingVerifierSetup,
    ToolPolicyRetry,
}

impl ControllerRecoveryStrategy {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ToolFirstRetry => "tool_first_retry",
            Self::TargetedArtifactRetry => "targeted_artifact_retry",
            Self::DeterministicFallback => "deterministic_fallback",
            Self::DeterministicBindingRepair => "deterministic_binding_repair",
            Self::EvidenceAction => "evidence_action",
            Self::VerifierRepairEdit => "verifier_repair_edit",
            Self::MissingVerifierSetup => "missing_verifier_setup",
            Self::ToolPolicyRetry => "tool_policy_retry",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ControllerPolicyLedger {
    strategies: Vec<ControllerRecoveryStrategy>,
}

impl ControllerPolicyLedger {
    pub(super) fn clear(&mut self) {
        self.strategies.clear();
    }

    pub(super) fn record(&mut self, strategy: ControllerRecoveryStrategy) {
        if !self.strategies.contains(&strategy) {
            self.strategies.push(strategy);
        }
    }

    pub(super) fn distinct_strategy_count(&self) -> usize {
        self.strategies.len()
    }

    pub(super) fn strategy_labels(&self) -> Vec<String> {
        self.strategies
            .iter()
            .map(|strategy| strategy.label().to_string())
            .collect()
    }

    fn first_untried(&self) -> Option<ControllerRecoveryStrategy> {
        [
            ControllerRecoveryStrategy::ToolFirstRetry,
            ControllerRecoveryStrategy::TargetedArtifactRetry,
            ControllerRecoveryStrategy::EvidenceAction,
            ControllerRecoveryStrategy::VerifierRepairEdit,
            ControllerRecoveryStrategy::DeterministicFallback,
        ]
        .into_iter()
        .find(|strategy| !self.strategies.contains(strategy))
    }

    pub(super) fn record_next_for_prose_block(&mut self) -> ControllerRecoveryStrategy {
        let strategy = self
            .first_untried()
            .unwrap_or(ControllerRecoveryStrategy::ToolFirstRetry);
        self.record(strategy);
        strategy
    }

    pub(super) fn persistent_failure_allowed(&self) -> bool {
        self.distinct_strategy_count() >= MIN_DISTINCT_RECOVERY_STRATEGIES
    }
}

pub(super) fn recoverable_action_available(action: Option<&ArtifactRecoveryAction>) -> bool {
    matches!(
        action,
        Some(
            ArtifactRecoveryAction::Continue { .. }
                | ArtifactRecoveryAction::RunVerifier
                | ArtifactRecoveryAction::RepairArtifact { .. }
        )
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProseOnlyTerminalDecision {
    BlockForRecovery,
    AllowTerminalReport,
    BoundaryStop,
}

pub(super) fn prose_only_terminal_decision(
    action: Option<&ArtifactRecoveryAction>,
    ledger: &ControllerPolicyLedger,
) -> ProseOnlyTerminalDecision {
    if matches!(action, Some(ArtifactRecoveryAction::SafeStop { .. })) {
        return ProseOnlyTerminalDecision::BoundaryStop;
    }
    if recoverable_action_available(action) && !ledger.persistent_failure_allowed() {
        return ProseOnlyTerminalDecision::BlockForRecovery;
    }
    ProseOnlyTerminalDecision::AllowTerminalReport
}

#[cfg(test)]
pub(super) fn safe_boundary_action(action: Option<&ArtifactRecoveryAction>) -> bool {
    matches!(action, Some(ArtifactRecoveryAction::SafeStop { .. }))
}

pub(super) fn prose_block_recovery_note(
    strategy: ControllerRecoveryStrategy,
    strategy_count: usize,
) -> String {
    let strategy_label = strategy.label();
    format!(
        "[Controller Persistence Policy] A recoverable action is still available, so do not finish with assistant prose only. Emit exactly one valid local tool action or continue the active recovery job now. Preserve approval, permission, secret, destructive-action, and external-state boundaries; if one of those boundaries blocks continuation, report that boundary explicitly. controller_recovery_strategy={strategy_label} controller_recovery_strategy_count={strategy_count}/{MIN_DISTINCT_RECOVERY_STRATEGIES}"
    )
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::{ArtifactRole, RecoveryTargetHint, SafeStopReason};
    use super::*;

    #[test]
    fn recoverable_actions_are_distinct_from_safe_boundaries() {
        let continue_action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::Implementation],
            target_hint: Some(RecoveryTargetHint {
                role: ArtifactRole::Implementation,
                path: "src/lib.rs".to_string(),
                reason: "test target".to_string(),
            }),
        };
        assert!(recoverable_action_available(Some(&continue_action)));
        assert!(!safe_boundary_action(Some(&continue_action)));

        let safe_stop = ArtifactRecoveryAction::SafeStop {
            reason: SafeStopReason::VerifierMissing,
        };
        assert!(!recoverable_action_available(Some(&safe_stop)));
        assert!(safe_boundary_action(Some(&safe_stop)));
    }

    #[test]
    fn three_distinct_strategies_are_required_before_persistent_failure() {
        let mut ledger = ControllerPolicyLedger::default();
        ledger.record(ControllerRecoveryStrategy::ToolFirstRetry);
        ledger.record(ControllerRecoveryStrategy::ToolFirstRetry);
        assert_eq!(ledger.distinct_strategy_count(), 1);
        assert!(!ledger.persistent_failure_allowed());

        ledger.record(ControllerRecoveryStrategy::TargetedArtifactRetry);
        assert_eq!(ledger.distinct_strategy_count(), 2);
        assert!(!ledger.persistent_failure_allowed());

        ledger.record(ControllerRecoveryStrategy::EvidenceAction);
        assert_eq!(ledger.distinct_strategy_count(), 3);
        assert!(ledger.persistent_failure_allowed());
    }

    #[test]
    fn recoverable_prose_only_terminal_is_blocked_until_three_strategies() {
        let action = ArtifactRecoveryAction::RepairArtifact { target_hint: None };
        let mut ledger = ControllerPolicyLedger::default();

        assert_eq!(
            prose_only_terminal_decision(Some(&action), &ledger),
            ProseOnlyTerminalDecision::BlockForRecovery
        );

        ledger.record(ControllerRecoveryStrategy::ToolFirstRetry);
        ledger.record(ControllerRecoveryStrategy::TargetedArtifactRetry);
        assert_eq!(
            prose_only_terminal_decision(Some(&action), &ledger),
            ProseOnlyTerminalDecision::BlockForRecovery
        );

        ledger.record(ControllerRecoveryStrategy::EvidenceAction);
        assert_eq!(
            prose_only_terminal_decision(Some(&action), &ledger),
            ProseOnlyTerminalDecision::AllowTerminalReport
        );
    }

    #[test]
    fn safe_boundary_terminal_is_not_converted_into_recovery() {
        let action = ArtifactRecoveryAction::SafeStop {
            reason: SafeStopReason::VerifierWeak,
        };
        let ledger = ControllerPolicyLedger::default();
        assert_eq!(
            prose_only_terminal_decision(Some(&action), &ledger),
            ProseOnlyTerminalDecision::BoundaryStop
        );
    }

    #[test]
    fn prose_block_recovery_note_exposes_strategy_count_without_paths_or_commands() {
        let note = prose_block_recovery_note(ControllerRecoveryStrategy::EvidenceAction, 2);
        assert!(note.contains("controller_recovery_strategy=evidence_action"));
        assert!(note.contains("controller_recovery_strategy_count=2/3"));
        assert!(note.contains("Preserve approval"));
    }

    #[test]
    fn deterministic_binding_repair_label_feeds_the_operator_hit_rate() {
        // Issue #991: the binding-repair strategy must carry the `deterministic`
        // marker so `analyze_run.py::_deterministic_operator_hit` /
        // `report.py` count it in the deterministic-operator hit rate.
        let label = ControllerRecoveryStrategy::DeterministicBindingRepair.label();
        assert_eq!(label, "deterministic_binding_repair");
        assert!(label.contains("deterministic"));
    }
}
