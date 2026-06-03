//! Work-mode / feedback-kind / quality second-pass confirmation flow
//! extracted from `turn.rs` (parent #680).
//!
//! Hosts four production entry points and four private attempt /
//! resolution helpers, originally `impl Agent` methods. Converted to
//! free functions taking `&mut Agent`, matching the
//! `actor_loop_flow` / `anti_pattern_flow` / `case_record_flow` pattern.
//!
//! Entry points:
//! - `classify_with_confirmation` (Issue #576 WorkMode wrapper)
//! - `maybe_invoke_work_mode_confirm` (Issue #576 gate + dispatch)
//! - `classify_with_feedback_confirm` (Issue #579 FeedbackKind wrapper)
//! - `implementation_quality_issue_with_confirm` (Issue #580 Quality wrapper)
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::time::Instant;

use super::Agent;
use super::confirmation_flow::{
    effective_turn_index_for_stage, log_feedback_kind_confirm_outcome, log_quality_confirm_outcome,
    log_work_mode_confirm_outcome, override_feedback_kind_from_outcome,
    preflight_feedback_kind_skip_reason, preflight_quality_confirm_skip_reason,
    preflight_work_mode_skip_reason, quality_confirm_cached_result, should_writeback_first_pass,
};
use super::feedback_kind_confirm::{
    self, FEEDBACK_KIND_CONFIRM_TIMEOUT_SECS, FeedbackKindConfirmInputs,
    FeedbackKindConfirmOutcome, run_feedback_kind_confirm_with_strategy,
};
use super::quality::quality_first_pass_observation;
use super::quality_confirm::{
    self, QUALITY_CONFIRM_TIMEOUT_SECS, QualityConfirmInputs, QualityConfirmOutcome,
    QualityConfirmation, QualityConfirmationSource, run_quality_confirm_with_strategy,
    should_request_quality_confirmation,
};
use super::turn_helpers::quality_confirm_cache_key;
use super::work_mode_confirm::{
    self, WORK_MODE_CONFIRM_TIMEOUT_SECS, WorkModeConfirmInputs, WorkModeConfirmOutcome,
    run_work_mode_confirm_with_strategy,
};
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ModeClassification, WorkMode, classify_work_mode_json};
use crate::session::feedback::FeedbackKind;
use crate::session::store::ConversationMessage;

/// Issue #576: SSoT wrapper that classifies user input with
/// `classify_work_mode_json`, emits the existing
/// `agent.work_mode.classified` event (now with `turn_index`), then drives
/// the LLM second-pass confirmation via `maybe_invoke_work_mode_confirm`.
///
/// Caller contract (CB-001 / Issue #576 follow-up):
///   * The first-pass `work_mode` is written back to
///     `session.mode_state.work_mode` ONLY when the per-turn cap has not
///     yet been consumed. This protects against `auto_plan_precheck` →
///     `handle_user_message` double-dispatch, where the latter would
///     otherwise overwrite the orchestrator-resolved final mode with a
///     fresh first-pass guess. `maybe_invoke_work_mode_confirm` will
///     then early-return as `Skipped(PerTurnCapConsumed)` and the final
///     mode persists.
///
/// `stage_label` distinguishes call sites in the `agent.work_mode.classified`
/// event payload (e.g. `"auto_plan_precheck"` vs `"turn_start"`). The event
/// shares the same `(session_id, turn_index)` join key as the matching
/// `turn_start` event and the post-loop AnvilScore event.
pub(super) fn classify_with_confirmation(
    agent: &mut Agent,
    input: &str,
    stage_label: &'static str,
) -> ModeClassification {
    let mut classification = classify_work_mode_json(input);
    // Issue #922 (PR-002 / DR3-004): a report-intended research request must not
    // stay `AnswerOnly` — that mode blocks Write/Edit and `ProtocolKind::AnswerOnly`
    // rejects the report's RepoEdit / ReportCompletenessPass evidence, so the
    // contract-side report obligation could never be satisfied. Map AnswerOnly ->
    // Docs (`ProtocolKind::Docs` accepts the report and allows edits) for this
    // case ONLY; every other mode/request is untouched. The trigger reuses the
    // contract-side SSOT so it stays in lock-step with the completion gates.
    if classification.work_mode == WorkMode::AnswerOnly
        && super::task_contract::report_intended_research(input)
    {
        classification.work_mode = WorkMode::Docs;
        classification.allows_file_edits = true;
        classification.requires_tests = false;
    }
    // CB-001: only write back the first-pass result when the per-turn cap
    // has NOT yet been consumed. Otherwise the previous call already
    // resolved the final mode and we must keep it.
    if should_writeback_first_pass(agent.work_mode_confirm_called_this_turn) {
        agent.session.mode_state.work_mode = classification.work_mode;
    }
    // CB-004: align `turn_index` with the upcoming `handle_user_message`
    // turn for the pre-`handle_user_message` precheck event.
    let event_turn_index = effective_turn_index_for_stage(stage_label, agent.current_turn_index);
    log_llm_event(
        "agent.work_mode.classified",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": event_turn_index,
            "input": input,
            "stage": stage_label,
            "work_mode": classification.work_mode.as_str(),
            "intent": classification.intent,
            "confidence": classification.confidence,
            "ambiguity": classification.ambiguity,
            "alternative_gap": classification.alternative_gap,
            "allows_file_edits": classification.allows_file_edits,
            "requires_tests": classification.requires_tests,
            "reason": classification.reason,
            "evidence": &classification.evidence,
            "alternatives": &classification.alternatives,
        }),
    );
    maybe_invoke_work_mode_confirm(agent, &classification, input, event_turn_index);
    classification
}

/// Issue #576: gate + dispatch the WorkMode second-pass confirmation.
pub(super) fn maybe_invoke_work_mode_confirm(
    agent: &mut Agent,
    first_pass: &ModeClassification,
    raw_input: &str,
    turn_index: usize,
) {
    let session_id = agent.session_store.session_id().to_string();
    let sidecar_model = agent.models.sidecar.clone();
    let env_disabled = work_mode_confirm::work_mode_confirm_disabled(|k: &str| std::env::var(k));
    if let Some(reason) = preflight_work_mode_skip_reason(
        agent.work_mode_confirm_called_this_turn,
        agent.session.mode_state.mode,
        env_disabled,
    ) {
        let outcome = WorkModeConfirmOutcome::Skipped { reason };
        log_work_mode_confirm_outcome(
            &outcome,
            &session_id,
            sidecar_model.as_deref(),
            turn_index,
            first_pass,
            None,
        );
        return;
    }

    // Build inputs + invoke orchestrator. The orchestrator handles the
    // remaining skip / fallback branches.
    let inputs = WorkModeConfirmInputs {
        first_pass,
        raw_input,
        session_id: &session_id,
        turn_index,
        model: sidecar_model.as_deref(),
    };

    let attempt_started = Instant::now();
    let outcome = run_work_mode_confirm_attempt(agent, inputs, &sidecar_model);
    let latency_ms = attempt_started.elapsed().as_millis() as u64;

    if let WorkModeConfirmOutcome::Confirmed(c) = &outcome {
        agent.session.mode_state.work_mode = c.mode;
    }

    log_work_mode_confirm_outcome(
        &outcome,
        &session_id,
        sidecar_model.as_deref(),
        turn_index,
        first_pass,
        Some(latency_ms),
    );
}

fn run_work_mode_confirm_attempt(
    agent: &mut Agent,
    inputs: WorkModeConfirmInputs<'_>,
    sidecar_model: &Option<String>,
) -> WorkModeConfirmOutcome {
    if let Some(sidecar_name) = sidecar_model.as_ref() {
        // We're about to dispatch — consume the per-turn cap regardless of
        // success/failure (DR4-004) so timeout/malformed/oversized cannot
        // re-trigger another dispatch in the same user-input.
        agent.work_mode_confirm_called_this_turn = true;
        let confirm_client = agent
            .client
            .clone_with_overrides(WORK_MODE_CONFIRM_TIMEOUT_SECS, 384)
            .ok();
        run_work_mode_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
            Some(c) => c
                .chat_text(
                    sidecar_name,
                    &[ConversationMessage::user(prompt.to_string())],
                )
                .map(|reply| reply.content),
            None => Err("client clone_with_overrides failed".to_string()),
        })
    } else {
        // sidecar_model is None — orchestrator returns Fallback(SidecarUnavailable)
        // without invoking the closure. We do not consume the per-turn cap
        // because the user might transition into a state where the sidecar
        // becomes available later in this same turn (defensive design).
        run_work_mode_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
    }
}

/// Issue #579: FeedbackKind second-pass confirmation wrapper. Called from
/// `success.rs` facade immediately before `record_feedback_if_unset(fb)`.
pub(super) fn classify_with_feedback_confirm(
    agent: &mut Agent,
    first_pass: &FeedbackKind,
    combined_output: &str,
) -> Option<FeedbackKind> {
    let session_id = agent.session_store.session_id().to_string();
    let sidecar_model = agent.models.sidecar.clone();
    let turn_index = agent.current_turn_index;
    let combined_bytes = combined_output.len();
    let env_disabled =
        feedback_kind_confirm::feedback_kind_confirm_disabled(|k: &str| std::env::var(k));
    if let Some(reason) = preflight_feedback_kind_skip_reason(
        agent.feedback_kind_confirm_called_this_turn,
        agent.session.mode_state.mode,
        env_disabled,
    ) {
        let outcome = FeedbackKindConfirmOutcome::Skipped { reason };
        log_feedback_kind_confirm_outcome(
            &outcome,
            &session_id,
            turn_index,
            first_pass,
            sidecar_model.as_deref(),
            combined_bytes,
            None,
        );
        return None;
    }
    let inputs = FeedbackKindConfirmInputs {
        first_pass,
        combined_output,
        session_id: &session_id,
        turn_index,
        model: sidecar_model.as_deref(),
    };
    let attempt_started = Instant::now();
    let outcome = run_feedback_kind_confirm_attempt(agent, inputs, &sidecar_model);
    let latency_ms = attempt_started.elapsed().as_millis() as u64;
    let override_kind = override_feedback_kind_from_outcome(&outcome, first_pass);
    log_feedback_kind_confirm_outcome(
        &outcome,
        &session_id,
        turn_index,
        first_pass,
        sidecar_model.as_deref(),
        combined_bytes,
        Some(latency_ms),
    );
    override_kind
}

fn run_feedback_kind_confirm_attempt(
    agent: &mut Agent,
    inputs: FeedbackKindConfirmInputs<'_>,
    sidecar_model: &Option<String>,
) -> FeedbackKindConfirmOutcome {
    if let Some(sidecar_name) = sidecar_model.as_ref() {
        agent.feedback_kind_confirm_called_this_turn = true;
        let confirm_client = agent
            .client
            .clone_with_overrides(FEEDBACK_KIND_CONFIRM_TIMEOUT_SECS, 384)
            .ok();
        run_feedback_kind_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
            Some(c) => c
                .chat_text(
                    sidecar_name,
                    &[ConversationMessage::user(prompt.to_string())],
                )
                .map(|reply| reply.content),
            None => Err("client clone_with_overrides failed".to_string()),
        })
    } else {
        run_feedback_kind_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
    }
}

/// Issue #580: Quality-gate second-pass confirmation wrapper. Returns the
/// final `issue` (None = pass, Some = quality gate fail).
pub(super) fn implementation_quality_issue_with_confirm(
    agent: &mut Agent,
    request: &str,
    content: &str,
) -> Option<String> {
    // SSoT first-pass observation. The wrapper signature `Option<String>`
    // is preserved for the deterministic / non-confirmable paths.
    let observation = quality_first_pass_observation(request, content);

    let session_id = agent.session_store.session_id().to_string();
    let sidecar_model = agent.models.sidecar.clone();
    let turn_index = agent.current_turn_index;
    let content_hash = quality_confirm_cache_key(request, content);
    if let Some(skip_reason) = preflight_quality_confirm_skip_reason(
        agent.session.mode_state.mode,
        quality_confirm::quality_confirm_disabled(|k: &str| std::env::var(k)),
        agent.quality_confirm_called_this_turn,
    ) {
        if skip_reason == quality_confirm::QualityConfirmSkipReason::PerTurnCapConsumed
            && let Some(cached) = quality_confirm_cached_result(
                agent.last_quality_confirm_result.as_ref(),
                content_hash,
            )
        {
            let outcome = QualityConfirmOutcome::Confirmed(cached.clone());
            log_quality_confirm_outcome(
                &outcome,
                &session_id,
                sidecar_model.as_deref(),
                turn_index,
                &observation,
                None,
            );
            return cached.issue;
        }
        let outcome = QualityConfirmOutcome::Skipped {
            reason: skip_reason,
        };
        log_quality_confirm_outcome(
            &outcome,
            &session_id,
            sidecar_model.as_deref(),
            turn_index,
            &observation,
            None,
        );
        return observation.issue;
    }

    let will_dispatch =
        sidecar_model.is_some() && should_request_quality_confirmation(&observation);
    let inputs = QualityConfirmInputs {
        observation: &observation,
        request,
        content,
        session_id: &session_id,
        turn_index,
        model: sidecar_model.as_deref(),
    };
    let attempt_started = Instant::now();
    let outcome =
        run_quality_confirm_attempt(agent, inputs, sidecar_model.as_deref(), will_dispatch);
    let latency_ms = attempt_started.elapsed().as_millis() as u64;
    let final_issue = resolve_quality_confirm_issue(agent, &outcome, content_hash, &observation);
    log_quality_confirm_outcome(
        &outcome,
        &session_id,
        sidecar_model.as_deref(),
        turn_index,
        &observation,
        Some(latency_ms),
    );

    final_issue
}

fn run_quality_confirm_attempt(
    agent: &mut Agent,
    inputs: QualityConfirmInputs<'_>,
    sidecar_model: Option<&str>,
    will_dispatch: bool,
) -> QualityConfirmOutcome {
    if let Some(sidecar_name) = sidecar_model {
        if will_dispatch {
            agent.quality_confirm_called_this_turn = true;
        }
        let confirm_client = agent
            .client
            .clone_with_overrides(QUALITY_CONFIRM_TIMEOUT_SECS, 384)
            .ok();
        run_quality_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
            Some(c) => c
                .chat_text(
                    sidecar_name,
                    &[ConversationMessage::user(prompt.to_string())],
                )
                .and_then(|reply| {
                    if !reply.tool_calls.is_empty() {
                        Err("sidecar reply contained unexpected tool_calls".to_string())
                    } else {
                        Ok(reply.content)
                    }
                }),
            None => Err("client clone_with_overrides failed".to_string()),
        })
    } else {
        run_quality_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
    }
}

fn resolve_quality_confirm_issue(
    agent: &mut Agent,
    outcome: &QualityConfirmOutcome,
    content_hash: u64,
    observation: &super::quality::QualityFirstPassObservation,
) -> Option<String> {
    match outcome {
        QualityConfirmOutcome::Confirmed(c) => {
            agent.last_quality_confirm_result = Some((content_hash, c.clone()));
            c.issue.clone()
        }
        QualityConfirmOutcome::Skipped { .. } | QualityConfirmOutcome::Fallback { .. } => {
            if agent.quality_confirm_called_this_turn {
                agent.last_quality_confirm_result = Some((
                    content_hash,
                    QualityConfirmation {
                        issue: observation.issue.clone(),
                        reason: None,
                        source: QualityConfirmationSource::FirstPass,
                    },
                ));
            }
            observation.issue.clone()
        }
    }
}
