//! Issue #576 / #579 / #580 confirmation-flow plumbing extracted
//! from `turn.rs` (parent #680).
//!
//! Hosts:
//!
//! * `should_writeback_first_pass` — predicate that gates whether
//!   `classify_with_confirmation` overwrites the previously-resolved
//!   `session.mode_state.work_mode`.
//! * `effective_turn_index_for_stage` — compensates for
//!   `auto_plan_precheck` running before `handle_user_message`
//!   increments `current_turn_index`.
//! * `preflight_{work_mode,feedback_kind,quality_confirm}_skip_reason`
//!   — Plan-mode / env-disabled / per-turn-cap-consumed checks.
//! * `quality_confirm_cached_result` — per-turn memo cache lookup.
//! * `work_mode_confirm_parse_status` — outcome → parse status enum
//!   projection.
//! * `log_{work_mode_confirm,feedback_kind_confirm,quality_confirm}_outcome`
//!   — wrappers around the `build_*_log_payload` SSOTs that emit
//!   `agent.*.{classified,confirmed,skipped,fallback}` events.
//! * `override_feedback_kind_from_outcome` — extract the corrected
//!   `FeedbackKind` from a `SecondPassOverridden` confirmation.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use super::feedback_kind_confirm::{
    self, FeedbackKindConfirmOutcome, build_feedback_kind_confirm_log_payload,
};
use super::quality::QualityFirstPassObservation;
use super::quality_confirm::{
    self, QualityConfirmOutcome, QualityConfirmation, build_quality_confirm_log_payload,
};
use super::work_mode_confirm::{
    self, ParseStatus as WorkModeConfirmParseStatus, WorkModeConfirmOutcome,
    build_work_mode_confirm_log_payload,
};
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ExecutionMode, ModeClassification};
use crate::session::feedback::FeedbackKind;

/// CB-001 (Issue #576 follow-up): pure predicate that decides whether
/// `classify_with_confirmation` should overwrite `session.mode_state.work_mode`
/// with the freshly-computed first-pass result.
///
/// Returns `true` only when the per-turn confirmation cap has NOT yet been
/// consumed for this user input.
pub(super) fn should_writeback_first_pass(work_mode_confirm_called_this_turn: bool) -> bool {
    !work_mode_confirm_called_this_turn
}

/// CB-004 (Issue #576 follow-up): pure helper that maps a classification
/// `stage_label` and the current value of `Agent.current_turn_index` to the
/// `turn_index` to record in `agent.work_mode.{classified,confirmed,skipped,fallback}`
/// events.
pub(super) fn effective_turn_index_for_stage(
    stage_label: &str,
    current_turn_index: usize,
) -> usize {
    if stage_label == "auto_plan_precheck" {
        current_turn_index.saturating_add(1)
    } else {
        current_turn_index
    }
}

pub(super) fn preflight_work_mode_skip_reason(
    work_mode_confirm_called_this_turn: bool,
    mode: ExecutionMode,
    env_disabled: bool,
) -> Option<work_mode_confirm::WorkModeSkipReason> {
    if work_mode_confirm_called_this_turn {
        Some(work_mode_confirm::WorkModeSkipReason::PerTurnCapConsumed)
    } else if mode == ExecutionMode::Plan {
        Some(work_mode_confirm::WorkModeSkipReason::PlanMode)
    } else if env_disabled {
        Some(work_mode_confirm::WorkModeSkipReason::EnvDisabled)
    } else {
        None
    }
}

pub(super) fn preflight_feedback_kind_skip_reason(
    feedback_kind_confirm_called_this_turn: bool,
    mode: ExecutionMode,
    env_disabled: bool,
) -> Option<feedback_kind_confirm::FeedbackKindSkipReason> {
    if mode == ExecutionMode::Plan {
        Some(feedback_kind_confirm::FeedbackKindSkipReason::PlanMode)
    } else if env_disabled {
        Some(feedback_kind_confirm::FeedbackKindSkipReason::EnvDisabled)
    } else if feedback_kind_confirm_called_this_turn {
        Some(feedback_kind_confirm::FeedbackKindSkipReason::PerTurnCapConsumed)
    } else {
        None
    }
}

pub(super) fn preflight_quality_confirm_skip_reason(
    mode: ExecutionMode,
    env_disabled: bool,
    quality_confirm_called_this_turn: bool,
) -> Option<quality_confirm::QualityConfirmSkipReason> {
    if mode == ExecutionMode::Plan {
        Some(quality_confirm::QualityConfirmSkipReason::PlanMode)
    } else if env_disabled {
        Some(quality_confirm::QualityConfirmSkipReason::EnvDisabled)
    } else if quality_confirm_called_this_turn {
        Some(quality_confirm::QualityConfirmSkipReason::PerTurnCapConsumed)
    } else {
        None
    }
}

pub(super) fn quality_confirm_cached_result(
    last_quality_confirm_result: Option<&(u64, QualityConfirmation)>,
    content_hash: u64,
) -> Option<QualityConfirmation> {
    last_quality_confirm_result
        .and_then(|(hash, cached)| (*hash == content_hash).then(|| cached.clone()))
}

pub(super) fn work_mode_confirm_parse_status(
    outcome: &WorkModeConfirmOutcome,
) -> WorkModeConfirmParseStatus {
    match outcome {
        WorkModeConfirmOutcome::Confirmed(_) => WorkModeConfirmParseStatus::Ok,
        WorkModeConfirmOutcome::Skipped { .. } => WorkModeConfirmParseStatus::NotInvoked,
        WorkModeConfirmOutcome::Fallback { reason, .. } => match reason {
            work_mode_confirm::WorkModeFallbackReason::Timeout => {
                WorkModeConfirmParseStatus::Timeout
            }
            work_mode_confirm::WorkModeFallbackReason::TransportError
            | work_mode_confirm::WorkModeFallbackReason::SidecarUnavailable => {
                WorkModeConfirmParseStatus::TransportError
            }
            work_mode_confirm::WorkModeFallbackReason::Empty => WorkModeConfirmParseStatus::Empty,
            work_mode_confirm::WorkModeFallbackReason::Malformed
            | work_mode_confirm::WorkModeFallbackReason::ResponseTooLarge
            | work_mode_confirm::WorkModeFallbackReason::UnknownMode => {
                WorkModeConfirmParseStatus::Malformed
            }
        },
    }
}

pub(super) fn log_work_mode_confirm_outcome(
    outcome: &WorkModeConfirmOutcome,
    session_id: &str,
    sidecar_model: Option<&str>,
    turn_index: usize,
    first_pass: &ModeClassification,
    latency_ms: Option<u64>,
) {
    let (event, payload) = build_work_mode_confirm_log_payload(
        outcome,
        session_id,
        sidecar_model,
        turn_index,
        first_pass,
        latency_ms,
        work_mode_confirm_parse_status(outcome),
    );
    log_llm_event(event, payload);
}

pub(super) fn log_feedback_kind_confirm_outcome(
    outcome: &FeedbackKindConfirmOutcome,
    session_id: &str,
    turn_index: usize,
    first_pass: &FeedbackKind,
    sidecar_model: Option<&str>,
    combined_bytes: usize,
    latency_ms: Option<u64>,
) {
    let (event, payload) = build_feedback_kind_confirm_log_payload(
        outcome,
        session_id,
        turn_index,
        first_pass,
        sidecar_model,
        combined_bytes,
        latency_ms,
    );
    log_llm_event(event, payload);
}

pub(super) fn override_feedback_kind_from_outcome(
    outcome: &FeedbackKindConfirmOutcome,
    first_pass: &FeedbackKind,
) -> Option<FeedbackKind> {
    match outcome {
        FeedbackKindConfirmOutcome::Confirmed(c)
            if c.source
                == feedback_kind_confirm::FeedbackKindConfirmationSource::SecondPassOverridden
                && &c.kind != first_pass =>
        {
            Some(c.kind.clone())
        }
        _ => None,
    }
}

pub(super) fn log_quality_confirm_outcome(
    outcome: &QualityConfirmOutcome,
    session_id: &str,
    sidecar_model: Option<&str>,
    turn_index: usize,
    observation: &QualityFirstPassObservation,
    latency_ms: Option<u64>,
) {
    let (event, payload) = build_quality_confirm_log_payload(
        outcome,
        session_id,
        turn_index,
        sidecar_model,
        observation,
        latency_ms,
    );
    log_llm_event(event, payload);
}
