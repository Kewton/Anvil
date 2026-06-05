//! Issue #979 (parent #974, Issue E): controller-side ToolFailure recovery.
//!
//! A tool *protocol* failure — a malformed / truncated / unparseable tool call,
//! or a native-tool parser failure — is a controller/protocol problem, distinct
//! from a missing-deliverable or evidence failure. The summary projection
//! already routes `ExitReason::ToolCallFormatError` to
//! `GenericTerminalState::ModelOutputFailure` / `RecoveryJobKind::ToolFailureJob`
//! (see `summary.rs`), so the *classification* never confuses a protocol failure
//! with a deliverable/evidence failure.
//!
//! This module adds the *behavioral* guard that keeps a zero-file protocol
//! failure from terminating on assistant prose. When the reply-retry budget is
//! exhausted but no deliverable has been produced and the task still owes one,
//! the controller escalates one bounded round back into the normal tool/action
//! path. That re-entry is the second step of the two-step protocol recovery
//! (step 1 = the stricter minimal tool-call retry / native→tagged downgrade in
//! `reply_retry.rs`; step 2 = the no-tool deliverable fallback reached here), so
//! the deterministic deliverable recovery / MissingDeliverable path gets a
//! chance before any terminal.
//!
//! Pure decision logic only — no `Agent` access — so it is unit-testable
//! without Ollama. `pub(super)` / no facade re-export (DR3-001).

use crate::agent::recovery::ActionExpectation;

/// True when `error` is a *tool protocol* failure the controller can recover by
/// forcing a cleaner tool call: a malformed/truncated tool-call parse error or a
/// native-tool parser failure. Native-tool *transport* (5xx) and generic
/// transport errors are deliberately excluded — they are not fixed by
/// reformatting a tool call and keep the existing transport-retry path.
pub(super) fn is_tool_protocol_failure(error: &str) -> bool {
    super::lifecycle::is_tool_call_format_error(error)
        || super::lifecycle::is_native_tool_parser_failure(error)
}

/// Decision for a tool protocol failure surfaced at the pre-reply boundary after
/// the reply-retry budget is exhausted.
///
/// This is the *second* step of the two-step protocol recovery. Step 1 — the
/// stricter minimal tool-call retry / native→tagged protocol downgrade — runs
/// inside `reply_retry.rs` while retry budget remains; only once that is
/// exhausted does the error surface here for the step-2 decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolProtocolRecoveryDecision {
    /// Re-enter the actor loop one bounded round (step 2: no-tool deliverable
    /// fallback), forcing the next tool/action path instead of a zero-file
    /// terminal on assistant prose.
    EscalateToDeliverableRecovery,
    /// No escalation available — terminate with the protocol-failure terminal
    /// (`ExitReason::ToolCallFormatError` → `model_output_failure`) so a genuine
    /// protocol failure is never re-classified as a deliverable/evidence
    /// failure.
    Terminal,
}

/// Inputs for the zero-file protocol-failure escalation guard. Every field is
/// derived deterministically by the caller (no LLM).
#[derive(Debug, Clone, Copy)]
pub(super) struct ToolProtocolRecoveryInputs {
    /// `is_tool_protocol_failure(err)` for the surfaced error.
    pub(super) is_tool_protocol_failure: bool,
    /// What the active request expects the agent to do this turn.
    pub(super) action_expectation: ActionExpectation,
    /// Count of successful non-plan repo edits observed this session.
    pub(super) repo_edits_this_session: usize,
    /// Whether the per-turn escalation budget was already spent.
    pub(super) already_escalated_this_turn: bool,
}

impl ToolProtocolRecoveryInputs {
    /// The active request still owes a repository deliverable.
    fn deliverable_expected(self) -> bool {
        matches!(self.action_expectation, ActionExpectation::RepoChange)
    }

    /// No deliverable has been produced yet — a terminal here would be a
    /// zero-file terminal.
    fn zero_file(self) -> bool {
        self.repo_edits_this_session == 0
    }
}

/// Issue #979: a tool protocol failure that has produced zero deliverable edits
/// and still owes a deliverable must not terminal on assistant prose. Escalate
/// once per turn into the normal tool/action path so the controller's
/// deterministic deliverable recovery (or a stricter minimal tool-call retry)
/// gets a chance — avoiding the 0-file terminal. Every other case keeps the
/// existing terminal so the protocol-failure classification is preserved.
pub(super) fn decide_tool_protocol_recovery(
    inputs: ToolProtocolRecoveryInputs,
) -> ToolProtocolRecoveryDecision {
    if inputs.is_tool_protocol_failure
        && inputs.deliverable_expected()
        && inputs.zero_file()
        && !inputs.already_escalated_this_turn
    {
        ToolProtocolRecoveryDecision::EscalateToDeliverableRecovery
    } else {
        ToolProtocolRecoveryDecision::Terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT_ERR: &str = "tool call parser failed: malformed tool call markup";
    const TRUNCATED_ERR: &str =
        "tool call parser failed: truncated tool call (generate response hit length limit)";
    const NATIVE_PARSER_ERR: &str = "native tool parser failed: unexpected end element";
    const TRANSPORT_ERR: &str = "failed to contact ollama chat api: connection refused";

    fn inputs(
        is_protocol: bool,
        expectation: ActionExpectation,
        edits: usize,
        escalated: bool,
    ) -> ToolProtocolRecoveryInputs {
        ToolProtocolRecoveryInputs {
            is_tool_protocol_failure: is_protocol,
            action_expectation: expectation,
            repo_edits_this_session: edits,
            already_escalated_this_turn: escalated,
        }
    }

    #[test]
    fn classifies_format_and_native_parser_as_protocol_failure() {
        assert!(is_tool_protocol_failure(FORMAT_ERR));
        assert!(is_tool_protocol_failure(TRUNCATED_ERR));
        assert!(is_tool_protocol_failure(NATIVE_PARSER_ERR));
    }

    #[test]
    fn transport_error_is_not_a_protocol_failure() {
        // Transport failures keep the transport-retry path; reformatting a tool
        // call cannot fix them.
        assert!(!is_tool_protocol_failure(TRANSPORT_ERR));
    }

    #[test]
    fn escalates_zero_file_repo_change_protocol_failure() {
        // The core regression: a malformed-tool-call failure that produced no
        // deliverable, on a task that owes one, escalates instead of going
        // terminal on prose.
        let decision =
            decide_tool_protocol_recovery(inputs(true, ActionExpectation::RepoChange, 0, false));
        assert_eq!(
            decision,
            ToolProtocolRecoveryDecision::EscalateToDeliverableRecovery
        );
    }

    #[test]
    fn does_not_escalate_twice_in_one_turn() {
        // Bounded: once the per-turn escalation budget is spent, the next
        // protocol failure terminates (preserving the protocol classification).
        assert_eq!(
            decide_tool_protocol_recovery(inputs(true, ActionExpectation::RepoChange, 0, true)),
            ToolProtocolRecoveryDecision::Terminal
        );
    }

    #[test]
    fn does_not_escalate_when_a_deliverable_already_landed() {
        // Files already produced — not a zero-file terminal, so no escalation.
        assert_eq!(
            decide_tool_protocol_recovery(inputs(true, ActionExpectation::RepoChange, 1, false)),
            ToolProtocolRecoveryDecision::Terminal
        );
    }

    #[test]
    fn does_not_escalate_when_no_deliverable_is_expected() {
        // Answer-only / non-repo-change turns have no deliverable to protect.
        for expectation in [
            ActionExpectation::None,
            ActionExpectation::ToolAction,
            ActionExpectation::PlanProgress,
        ] {
            assert_eq!(
                decide_tool_protocol_recovery(inputs(true, expectation, 0, false)),
                ToolProtocolRecoveryDecision::Terminal,
                "expectation {expectation:?} must not escalate"
            );
        }
    }

    #[test]
    fn does_not_escalate_a_non_protocol_failure() {
        // Transport / non-protocol errors keep their own terminal/retry path.
        assert_eq!(
            decide_tool_protocol_recovery(inputs(false, ActionExpectation::RepoChange, 0, false)),
            ToolProtocolRecoveryDecision::Terminal
        );
    }
}
