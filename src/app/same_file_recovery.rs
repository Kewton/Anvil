//! Same-file recovery state machine for file.edit failure recovery (Issue #276).
//!
//! Manages the lifecycle of a recovery attempt after a `file.edit` failure:
//! edit failure -> recovery read -> retry edit -> resolved/escalated.
//!
//! The state machine is single-shot per recovery attempt and resets at turn boundaries.

use crate::tooling::EditFailureKind;

/// Immutable classification for a completed `file.read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadClassification {
    NormalRead,
    RecoveryRead,
}

/// file.edit failure recovery state machine.
///
/// Owned by `App` and driven by `record_tool_result()` in agentic.rs.
/// Per-path, single-shot: tracks the most recent edit failure only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum SameFileRecoveryState {
    /// No active recovery.
    #[default]
    Normal,

    /// file.edit just failed. Kind determines next action.
    EditFailed { path: String, kind: EditFailureKind },

    /// FinalNotFound: waiting for a recovery file.read of the same path.
    /// Guard exemption applies to this path only.
    PendingRecoveryRead(String),

    /// Recovery read completed; waiting for retry edit.
    PendingRecoveryRetry(String),

    /// Recovery retry succeeded.
    Resolved,

    /// Retry failed or FinalMultipleMatches escalated to write fallback.
    EscalatedToWriteFallback(String),

    /// IoFailure or recovery read failed. No automatic recovery possible.
    Abandoned,
}

/// Immutable disposition for a completed `file.read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedReadDisposition {
    pub normalized_path: String,
    pub read_classification: ReadClassification,
    pub next_state: SameFileRecoveryState,
}

impl CompletedReadDisposition {
    pub fn is_recovery_read(&self) -> bool {
        matches!(self.read_classification, ReadClassification::RecoveryRead)
    }
}

impl SameFileRecoveryState {
    /// Transition on file.edit failure.
    ///
    /// Consumes self so that PendingRecoveryRetry re-failure can transition
    /// to EscalatedToWriteFallback.
    pub fn on_edit_failure(self, path: String, kind: EditFailureKind) -> Self {
        match (&self, kind) {
            (Self::PendingRecoveryRetry(p), _) if *p == path => {
                Self::EscalatedToWriteFallback(path)
            }
            (_, EditFailureKind::FinalNotFound) => Self::PendingRecoveryRead(path),
            (_, EditFailureKind::FinalMultipleMatches) | (_, EditFailureKind::IdenticalContent) => {
                Self::EditFailed { path, kind }
            }
            (_, EditFailureKind::IoFailure) => Self::Abandoned,
        }
    }

    /// Classify a successful file.read exactly once from the pre-transition state.
    ///
    /// This decouples the immutable fact ("this read was a recovery read") from
    /// the next mutable state so downstream consumers do not need to re-read
    /// live state after a transition.
    pub fn classify_completed_read(self, read_path: &str) -> CompletedReadDisposition {
        match self {
            Self::PendingRecoveryRead(path) if path == read_path => CompletedReadDisposition {
                normalized_path: read_path.to_string(),
                read_classification: ReadClassification::RecoveryRead,
                next_state: Self::PendingRecoveryRetry(path),
            },
            other => CompletedReadDisposition {
                normalized_path: read_path.to_string(),
                read_classification: ReadClassification::NormalRead,
                next_state: other,
            },
        }
    }

    /// Transition on file.read failure.
    pub fn on_read_failed(self) -> Self {
        match self {
            Self::PendingRecoveryRead(_) => Self::Abandoned,
            other => other,
        }
    }

    /// Transition on file.edit success.
    pub fn on_edit_success(self, success_path: &str) -> Self {
        match self {
            Self::PendingRecoveryRetry(path) if path == success_path => Self::Resolved,
            _ => Self::Normal,
        }
    }

    /// Whether the state should be reset to Normal after processing.
    pub fn should_reset(&self) -> bool {
        matches!(self, Self::Resolved | Self::Abandoned)
    }
}

#[cfg(test)]
mod recovery_state_tests {
    use super::*;
    use crate::tooling::EditFailureKind;

    #[test]
    fn happy_path_final_not_found_to_resolved() {
        let state = SameFileRecoveryState::Normal;

        // Edit fails with FinalNotFound
        let state = state.on_edit_failure("src/main.rs".into(), EditFailureKind::FinalNotFound);
        assert_eq!(
            state,
            SameFileRecoveryState::PendingRecoveryRead("src/main.rs".into())
        );

        // Recovery read succeeds
        let disposition = state.classify_completed_read("src/main.rs");
        assert_eq!(disposition.normalized_path, "src/main.rs");
        assert_eq!(
            disposition.read_classification,
            ReadClassification::RecoveryRead
        );
        let state = disposition.next_state;
        assert_eq!(
            state,
            SameFileRecoveryState::PendingRecoveryRetry("src/main.rs".into())
        );

        // Retry edit succeeds
        let state = state.on_edit_success("src/main.rs");
        assert_eq!(state, SameFileRecoveryState::Resolved);
        assert!(state.should_reset());
    }

    #[test]
    fn multiple_matches_stays_edit_failed() {
        let state = SameFileRecoveryState::Normal;
        let state =
            state.on_edit_failure("src/lib.rs".into(), EditFailureKind::FinalMultipleMatches);
        assert_eq!(
            state,
            SameFileRecoveryState::EditFailed {
                path: "src/lib.rs".into(),
                kind: EditFailureKind::FinalMultipleMatches,
            }
        );
        assert!(!state.should_reset());
    }

    #[test]
    fn identical_content_stays_edit_failed() {
        let state = SameFileRecoveryState::Normal;
        let state = state.on_edit_failure("src/lib.rs".into(), EditFailureKind::IdenticalContent);
        assert_eq!(
            state,
            SameFileRecoveryState::EditFailed {
                path: "src/lib.rs".into(),
                kind: EditFailureKind::IdenticalContent,
            }
        );
    }

    #[test]
    fn io_failure_to_abandoned() {
        let state = SameFileRecoveryState::Normal;
        let state = state.on_edit_failure("src/bad.rs".into(), EditFailureKind::IoFailure);
        assert_eq!(state, SameFileRecoveryState::Abandoned);
        assert!(state.should_reset());
    }

    #[test]
    fn recovery_read_failure_to_abandoned() {
        let state = SameFileRecoveryState::PendingRecoveryRead("src/main.rs".into());
        let state = state.on_read_failed();
        assert_eq!(state, SameFileRecoveryState::Abandoned);
    }

    #[test]
    fn recovery_retry_failure_to_escalated() {
        let state = SameFileRecoveryState::PendingRecoveryRetry("src/main.rs".into());
        // Retry edit fails again
        let state = state.on_edit_failure("src/main.rs".into(), EditFailureKind::FinalNotFound);
        assert_eq!(
            state,
            SameFileRecoveryState::EscalatedToWriteFallback("src/main.rs".into())
        );
    }

    #[test]
    fn different_path_read_does_not_transition() {
        let state = SameFileRecoveryState::PendingRecoveryRead("src/main.rs".into());
        // Read a different file
        let disposition = state.classify_completed_read("src/other.rs");
        assert_eq!(disposition.normalized_path, "src/other.rs");
        assert_eq!(
            disposition.read_classification,
            ReadClassification::NormalRead
        );
        let state = disposition.next_state;
        assert_eq!(
            state,
            SameFileRecoveryState::PendingRecoveryRead("src/main.rs".into())
        );
    }

    #[test]
    fn pending_retry_reads_are_normal_reads() {
        let state = SameFileRecoveryState::PendingRecoveryRetry("src/main.rs".into());
        let disposition = state.classify_completed_read("src/main.rs");
        assert_eq!(
            disposition.read_classification,
            ReadClassification::NormalRead
        );
        assert_eq!(
            disposition.next_state,
            SameFileRecoveryState::PendingRecoveryRetry("src/main.rs".into())
        );
    }

    #[test]
    fn should_reset_on_resolved_and_abandoned() {
        assert!(SameFileRecoveryState::Resolved.should_reset());
        assert!(SameFileRecoveryState::Abandoned.should_reset());

        assert!(!SameFileRecoveryState::Normal.should_reset());
        assert!(!SameFileRecoveryState::PendingRecoveryRead("x".into()).should_reset());
        assert!(!SameFileRecoveryState::PendingRecoveryRetry("x".into()).should_reset());
        assert!(!SameFileRecoveryState::EscalatedToWriteFallback("x".into()).should_reset());
        assert!(
            !SameFileRecoveryState::EditFailed {
                path: "x".into(),
                kind: EditFailureKind::FinalNotFound,
            }
            .should_reset()
        );
    }
}
