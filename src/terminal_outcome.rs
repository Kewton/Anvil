//! Shared terminal lifecycle vocabulary.
//!
//! Legacy terminal labels remain at compatibility boundaries. This module is
//! the crate-level projection point for newer generic lifecycle labels so
//! reporting surfaces do not each grow their own mapping table.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericTerminalState {
    Completed,
    MissingDeliverable,
    MissingEvidence,
    EvidenceFailed,
    EvidenceBindingFailed,
    EvidenceRunnerMissing,
    EvidenceRepairExhausted,
    EvidenceRepairSafeStop,
    ControlLoopExhausted,
    ModelOutputFailure,
    TransportFailure,
    Interrupted,
}

impl GenericTerminalState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            GenericTerminalState::Completed => "completed",
            GenericTerminalState::MissingDeliverable => "missing_deliverable",
            GenericTerminalState::MissingEvidence => "missing_evidence",
            GenericTerminalState::EvidenceFailed => "evidence_failed",
            GenericTerminalState::EvidenceBindingFailed => "evidence_binding_failed",
            GenericTerminalState::EvidenceRunnerMissing => "evidence_runner_missing",
            GenericTerminalState::EvidenceRepairExhausted => "evidence_repair_exhausted",
            GenericTerminalState::EvidenceRepairSafeStop => "evidence_repair_safe_stop",
            GenericTerminalState::ControlLoopExhausted => "control_loop_exhausted",
            GenericTerminalState::ModelOutputFailure => "model_output_failure",
            GenericTerminalState::TransportFailure => "transport_failure",
            GenericTerminalState::Interrupted => "interrupted",
        }
    }

    pub(crate) fn from_legacy_terminal_label(label: &str) -> Self {
        match label {
            "done" => GenericTerminalState::Completed,
            "missing_repo_edits" => GenericTerminalState::MissingDeliverable,
            "missing_verification" => GenericTerminalState::MissingEvidence,
            "verifier_failed" => GenericTerminalState::EvidenceFailed,
            "safe_stop_verifier_weak" => GenericTerminalState::EvidenceBindingFailed,
            "safe_stop_verifier_missing" => GenericTerminalState::EvidenceRunnerMissing,
            "repair_exhausted" => GenericTerminalState::EvidenceRepairExhausted,
            "repair_safe_stop" => GenericTerminalState::EvidenceRepairSafeStop,
            "max_iterations" | "plan_incomplete" => GenericTerminalState::ControlLoopExhausted,
            "tool_call_format_error" | "empty_responses" | "no_tool_calls" => {
                GenericTerminalState::ModelOutputFailure
            }
            "transport_error" => GenericTerminalState::TransportFailure,
            "interrupted" => GenericTerminalState::Interrupted,
            _ => GenericTerminalState::ControlLoopExhausted,
        }
    }
}

pub(crate) fn generic_label_for_legacy_terminal(label: &str) -> &'static str {
    GenericTerminalState::from_legacy_terminal_label(label).label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_terminal_labels_project_to_generic_lifecycle_labels() {
        let cases = [
            ("done", "completed"),
            ("missing_repo_edits", "missing_deliverable"),
            ("missing_verification", "missing_evidence"),
            ("verifier_failed", "evidence_failed"),
            ("safe_stop_verifier_weak", "evidence_binding_failed"),
            ("safe_stop_verifier_missing", "evidence_runner_missing"),
            ("repair_exhausted", "evidence_repair_exhausted"),
            ("repair_safe_stop", "evidence_repair_safe_stop"),
            ("max_iterations", "control_loop_exhausted"),
            ("plan_incomplete", "control_loop_exhausted"),
            ("tool_call_format_error", "model_output_failure"),
            ("empty_responses", "model_output_failure"),
            ("no_tool_calls", "model_output_failure"),
            ("transport_error", "transport_failure"),
            ("interrupted", "interrupted"),
        ];

        for (legacy, generic) in cases {
            assert_eq!(generic_label_for_legacy_terminal(legacy), generic);
        }
    }
}
