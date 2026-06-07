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
use super::project_profile::{
    self, PROJECT_PROFILE_CONFIRM_CONFIDENCE_THRESHOLD, PROJECT_PROFILE_CONFIRM_TIMEOUT_SECS,
    ProjectProfileConfirmation,
};
use super::quality::quality_first_pass_observation;
use super::quality_confirm::{
    self, QUALITY_CONFIRM_TIMEOUT_SECS, QualityConfirmInputs, QualityConfirmOutcome,
    QualityConfirmation, QualityConfirmationSource, run_quality_confirm_with_strategy,
    should_request_quality_confirmation,
};
use super::task_contract::{
    ArtifactRole, ProjectIntent, TaskClassification, TaskContract, TaskKind,
};
use super::task_kind_confirm::{
    self, TASK_KIND_CONFIRM_TIMEOUT_SECS, TaskKindConfirmInputs, TaskKindConfirmOutcome,
    TaskKindConfirmationSource, build_task_kind_confirm_log_payload,
    run_task_kind_confirm_with_strategy,
};
use super::turn_helpers::quality_confirm_cache_key;
use super::work_mode_confirm::{
    self, WORK_MODE_CONFIRM_TIMEOUT_SECS, WorkModeConfirmInputs, WorkModeConfirmOutcome,
    WorkModeConfirmation, WorkModeConfirmationSource, run_work_mode_confirm_with_strategy,
};
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ModeClassification, WorkMode, classify_work_mode_json};
use crate::session::feedback::{FeedbackKind, mask_secrets};
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
    let objective_requires_artifact = objective_requires_artifact(agent);
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
    guard_first_pass_for_objective_artifact(&mut classification, objective_requires_artifact);
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
    let outcome = guard_confirmed_work_mode_for_objective_artifact(
        outcome,
        first_pass,
        objective_requires_artifact(agent),
    );

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

fn objective_requires_artifact(agent: &Agent) -> bool {
    super::task_classification::task_contract_authority(agent)
        .is_some_and(|contract| !contract.required_artifacts.is_empty())
}

fn guard_first_pass_for_objective_artifact(
    classification: &mut ModeClassification,
    objective_requires_artifact: bool,
) {
    if objective_requires_artifact && classification.work_mode == WorkMode::AnswerOnly {
        classification.work_mode = WorkMode::GenericCode;
        classification.intent = "generic-edit";
        classification.allows_file_edits = true;
        classification.requires_tests = false;
        classification.reason = "objective contract requires an artifact";
        classification.evidence.push("objective-artifact");
    }
}

fn guard_confirmed_work_mode_for_objective_artifact(
    outcome: WorkModeConfirmOutcome,
    first_pass: &ModeClassification,
    objective_requires_artifact: bool,
) -> WorkModeConfirmOutcome {
    if !objective_requires_artifact {
        return outcome;
    }
    let WorkModeConfirmOutcome::Confirmed(confirmation) = outcome else {
        return outcome;
    };
    if confirmation.mode != WorkMode::AnswerOnly {
        return WorkModeConfirmOutcome::Confirmed(confirmation);
    }
    WorkModeConfirmOutcome::Confirmed(WorkModeConfirmation {
        mode: edit_capable_artifact_mode(first_pass.work_mode),
        confidence: first_pass.confidence,
        source: WorkModeConfirmationSource::FirstPass,
        reason: Some(
            "objective contract requires an artifact; answer-only confirmation ignored".to_string(),
        ),
    })
}

fn edit_capable_artifact_mode(first_pass_mode: WorkMode) -> WorkMode {
    match first_pass_mode {
        WorkMode::AnswerOnly | WorkMode::Unknown => WorkMode::GenericCode,
        mode => mode,
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

// ===========================================================================
// Issue #926 (P0.5b): TaskKind second-pass confirm dispatch.
// ===========================================================================

/// Skip reason for the TaskKind confirm before any LLM dispatch (cap / Plan /
/// env). Mirrors `preflight_work_mode_skip_reason`; the high-confidence gate
/// (`!needs_confirm()`) is checked by the populate caller and, defensively, by
/// the orchestrator (`Skip(HighConfidence)`).
fn preflight_task_kind_skip_reason(
    task_kind_confirm_called_this_turn: bool,
    mode: crate::modes::plan_act::ExecutionMode,
    env_disabled: bool,
) -> Option<task_kind_confirm::TaskKindSkipReason> {
    if task_kind_confirm_called_this_turn {
        Some(task_kind_confirm::TaskKindSkipReason::PerTurnCapConsumed)
    } else if mode == crate::modes::plan_act::ExecutionMode::Plan {
        Some(task_kind_confirm::TaskKindSkipReason::PlanMode)
    } else if env_disabled {
        Some(task_kind_confirm::TaskKindSkipReason::EnvDisabled)
    } else {
        None
    }
}

/// Emit the `agent.task_kind.classified` event for one confirm outcome.
fn log_task_kind_confirm_outcome(
    outcome: &TaskKindConfirmOutcome,
    session_id: &str,
    sidecar_model: Option<&str>,
    turn_index: usize,
    first_pass: &TaskClassification,
    latency_ms: Option<u64>,
) {
    let (event, payload) = build_task_kind_confirm_log_payload(
        outcome,
        session_id,
        sidecar_model,
        turn_index,
        first_pass,
        latency_ms,
    );
    log_llm_event(event, payload);
}

/// Issue #926: gate + dispatch the TaskKind second-pass confirmation.
///
/// Returns `Some(kind)` ONLY when the confirm overrode the first-pass kind with
/// a *different* allowlisted kind (`SecondPassOverridden`); every other outcome
/// (agree / skip / fallback) returns `None`, leaving the first-pass contract
/// untouched (DR2-002). The caller (`populate_task_contract_authority`) rebuilds
/// the contract via `from_request_with_kind(req, Some(kind))` on `Some`.
///
/// Must NOT be re-entrant into `task_contract_authority` / `populate_*`
/// (DR3-001): all inputs are threaded in by the caller.
pub(super) fn maybe_invoke_task_kind_confirm(
    agent: &mut Agent,
    first_pass: &TaskClassification,
    raw_input: &str,
    turn_index: usize,
) -> Option<TaskKind> {
    let session_id = agent.session_store.session_id().to_string();
    let sidecar_model = agent.models.sidecar.clone();
    let env_disabled = task_kind_confirm::task_kind_confirm_disabled(|k: &str| std::env::var(k));
    if let Some(reason) = preflight_task_kind_skip_reason(
        agent.task_kind_confirm_called_this_turn,
        agent.session.mode_state.mode,
        env_disabled,
    ) {
        let outcome = TaskKindConfirmOutcome::Skipped { reason };
        log_task_kind_confirm_outcome(
            &outcome,
            &session_id,
            sidecar_model.as_deref(),
            turn_index,
            first_pass,
            None,
        );
        return None;
    }

    let inputs = TaskKindConfirmInputs {
        first_pass,
        raw_input,
        model: sidecar_model.as_deref(),
    };

    let attempt_started = Instant::now();
    let outcome = run_task_kind_confirm_attempt(agent, inputs, &sidecar_model);
    let latency_ms = attempt_started.elapsed().as_millis() as u64;

    let forced = match &outcome {
        TaskKindConfirmOutcome::Confirmed(c)
            if c.source == TaskKindConfirmationSource::SecondPassOverridden =>
        {
            Some(c.task_kind)
        }
        _ => None,
    };

    log_task_kind_confirm_outcome(
        &outcome,
        &session_id,
        sidecar_model.as_deref(),
        turn_index,
        first_pass,
        Some(latency_ms),
    );
    forced
}

/// Inner attempt: consumes the per-turn cap and drives the sidecar client when
/// a model is configured; otherwise returns `Fallback(SidecarUnavailable)`
/// without consuming the cap (D3 — the `sidecar: None` no-op that keeps the
/// ~3400 lib tests deterministic).
fn run_task_kind_confirm_attempt(
    agent: &mut Agent,
    inputs: TaskKindConfirmInputs<'_>,
    sidecar_model: &Option<String>,
) -> TaskKindConfirmOutcome {
    if let Some(sidecar_name) = sidecar_model.as_ref() {
        // We are about to dispatch — consume the per-turn cap regardless of
        // success/failure so a timeout / malformed / oversized response cannot
        // re-trigger another dispatch in the same user-input.
        agent.task_kind_confirm_called_this_turn = true;
        let confirm_client = agent
            .client
            .clone_with_overrides(TASK_KIND_CONFIRM_TIMEOUT_SECS, 384)
            .ok();
        run_task_kind_confirm_with_strategy(inputs, |prompt| match confirm_client.as_ref() {
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
        // without invoking the closure and without consuming the cap.
        run_task_kind_confirm_with_strategy(inputs, |_| Err("sidecar unavailable".to_string()))
    }
}

// ===========================================================================
// ProjectProfile second-pass confirm dispatch.
// ===========================================================================

/// Confirm the objective profile behind the already-built first-pass
/// `TaskContract`. This is intentionally narrower than WorkMode/TaskKind:
/// WorkMode still decides tool access, TaskKind still decides the coarse task
/// class, and ProjectProfile only adjusts objective-level deliverable/evidence
/// details such as "README section, not environment setup".
pub(super) fn maybe_invoke_project_profile_confirm(
    agent: &mut Agent,
    first_pass: &TaskContract,
    raw_input: &str,
    turn_index: usize,
) -> Option<ProjectProfileConfirmation> {
    let session_id = agent.session_store.session_id().to_string();
    let sidecar_model = agent.models.sidecar.clone();
    let env_disabled =
        project_profile::project_profile_confirm_disabled(|k: &str| std::env::var(k));

    if agent.project_profile_confirm_called_this_turn {
        log_project_profile_confirm_outcome(ProjectProfileConfirmLogArgs {
            status: "skipped",
            session_id: &session_id,
            sidecar_model: sidecar_model.as_deref(),
            turn_index,
            first_pass,
            latency_ms: None,
            reason: Some("per_turn_cap_consumed"),
            profile: None,
        });
        return None;
    }
    if agent.session.mode_state.mode == crate::modes::plan_act::ExecutionMode::Plan {
        log_project_profile_confirm_outcome(ProjectProfileConfirmLogArgs {
            status: "skipped",
            session_id: &session_id,
            sidecar_model: sidecar_model.as_deref(),
            turn_index,
            first_pass,
            latency_ms: None,
            reason: Some("plan_mode"),
            profile: None,
        });
        return None;
    }
    if env_disabled {
        log_project_profile_confirm_outcome(ProjectProfileConfirmLogArgs {
            status: "skipped",
            session_id: &session_id,
            sidecar_model: sidecar_model.as_deref(),
            turn_index,
            first_pass,
            latency_ms: None,
            reason: Some("env_disabled"),
            profile: None,
        });
        return None;
    }
    if !should_request_project_profile_confirm(first_pass, raw_input) {
        return None;
    }

    let Some(sidecar_name) = sidecar_model.as_ref() else {
        log_project_profile_confirm_outcome(ProjectProfileConfirmLogArgs {
            status: "fallback",
            session_id: &session_id,
            sidecar_model: None,
            turn_index,
            first_pass,
            latency_ms: None,
            reason: Some("sidecar_unavailable"),
            profile: None,
        });
        return None;
    };

    agent.project_profile_confirm_called_this_turn = true;
    let masked_input = mask_secrets(raw_input);
    let project_intent = ProjectIntent::from_request(&masked_input);
    let prompt = project_profile::build_project_profile_confirm_prompt(
        &masked_input,
        project_intent
            .language
            .unwrap_or(super::task_contract::ProjectLanguage::Unknown),
        project_intent
            .shape
            .unwrap_or(super::task_contract::ProjectShape::Unknown),
        &project_profile_first_pass_summary(first_pass),
    );
    let confirm_client = agent
        .client
        .clone_with_overrides(PROJECT_PROFILE_CONFIRM_TIMEOUT_SECS, 768)
        .ok();
    let attempt_started = Instant::now();
    let raw_reply = match confirm_client.as_ref() {
        Some(c) => c
            .chat_text(
                sidecar_name,
                &[ConversationMessage::user(prompt.to_string())],
            )
            .map(|reply| reply.content),
        None => Err("client clone_with_overrides failed".to_string()),
    };
    let latency_ms = attempt_started.elapsed().as_millis() as u64;
    let profile = raw_reply
        .ok()
        .and_then(|reply| project_profile::parse_project_profile_confirmation(&reply));
    let adopted = profile
        .as_ref()
        .is_some_and(project_profile_confirmation_authoritative);
    log_project_profile_confirm_outcome(ProjectProfileConfirmLogArgs {
        status: if adopted { "confirmed" } else { "fallback" },
        session_id: &session_id,
        sidecar_model: sidecar_model.as_deref(),
        turn_index,
        first_pass,
        latency_ms: Some(latency_ms),
        reason: if adopted {
            None
        } else {
            Some("unusable_or_low_confidence")
        },
        profile: profile.as_ref(),
    });
    profile.filter(project_profile_confirmation_authoritative)
}

fn should_request_project_profile_confirm(first_pass: &TaskContract, raw_input: &str) -> bool {
    if first_pass.required_artifacts.is_empty() {
        return false;
    }
    if raw_input.contains("STATE_CONTROL_PACKET") {
        return false;
    }
    if first_pass.task_kind != TaskKind::Coding {
        return true;
    }
    first_pass
        .required_artifacts
        .iter()
        .chain(first_pass.optional_artifacts.iter())
        .any(|role| matches!(role, ArtifactRole::UsageDocs | ArtifactRole::DataOutput))
}

fn project_profile_confirmation_authoritative(profile: &ProjectProfileConfirmation) -> bool {
    profile.confidence >= PROJECT_PROFILE_CONFIRM_CONFIDENCE_THRESHOLD
}

fn project_profile_first_pass_summary(first_pass: &TaskContract) -> String {
    format!(
        "task_kind={}, intent={:?}, required_artifacts={:?}, optional_artifacts={:?}, verification_required={}",
        first_pass.task_kind.as_str(),
        first_pass.intent,
        first_pass.required_artifacts,
        first_pass.optional_artifacts,
        first_pass.verification_required
    )
}

struct ProjectProfileConfirmLogArgs<'a> {
    status: &'static str,
    session_id: &'a str,
    sidecar_model: Option<&'a str>,
    turn_index: usize,
    first_pass: &'a TaskContract,
    latency_ms: Option<u64>,
    reason: Option<&'a str>,
    profile: Option<&'a ProjectProfileConfirmation>,
}

fn log_project_profile_confirm_outcome(args: ProjectProfileConfirmLogArgs<'_>) {
    log_llm_event(
        "agent.project_profile.classified",
        serde_json::json!({
            "session_id": args.session_id,
            "turn_index": args.turn_index,
            "status": args.status,
            "model": args.sidecar_model,
            "first_pass_task_kind": args.first_pass.task_kind.as_str(),
            "first_pass_required_artifacts": args.first_pass.required_artifacts.iter().map(|role| role.label()).collect::<Vec<_>>(),
            "latency_ms": args.latency_ms,
            "reason": args.reason,
            "profile": args.profile.map(|profile| serde_json::json!({
                "language": profile.language.map(|language| format!("{language:?}")),
                "shape": profile.shape.map(|shape| format!("{shape:?}")),
                "deliverable_kind": profile.deliverable_kind.map(|kind| format!("{kind:?}")),
                "primary_artifacts": profile.primary_artifacts.clone(),
                "forbidden_artifacts": profile.forbidden_artifacts.iter().map(|artifact| format!("{artifact:?}")).collect::<Vec<_>>(),
                "evidence_kind": profile.evidence_kind.map(|kind| format!("{kind:?}")),
                "needs_environment_setup": profile.needs_environment_setup,
                "preferred_runner": profile.preferred_runner.clone(),
                "confidence": profile.confidence,
                "reason": profile.reason.clone(),
            })),
        }),
    );
}

#[cfg(test)]
mod objective_artifact_work_mode_tests {
    use super::*;

    fn make_classification(mode: WorkMode) -> ModeClassification {
        ModeClassification {
            work_mode: mode,
            intent: "test",
            allows_file_edits: mode != WorkMode::AnswerOnly,
            requires_tests: false,
            confidence: 0.7,
            ambiguity: false,
            alternative_gap: 0.2,
            reason: "test",
            evidence: vec![],
            alternatives: vec![],
        }
    }

    #[test]
    fn first_pass_answer_only_is_guarded_when_objective_requires_artifact() {
        let mut classification = make_classification(WorkMode::AnswerOnly);

        guard_first_pass_for_objective_artifact(&mut classification, true);

        assert_eq!(classification.work_mode, WorkMode::GenericCode);
        assert_eq!(classification.intent, "generic-edit");
        assert!(classification.allows_file_edits);
        assert!(classification.evidence.contains(&"objective-artifact"));
    }

    #[test]
    fn confirmed_answer_only_is_guarded_when_objective_requires_artifact() {
        let first_pass = make_classification(WorkMode::GenericCode);
        let outcome = WorkModeConfirmOutcome::Confirmed(WorkModeConfirmation {
            mode: WorkMode::AnswerOnly,
            confidence: 0.95,
            source: WorkModeConfirmationSource::SecondPassOverridden,
            reason: Some("sidecar said answer-only".to_string()),
        });

        let guarded = guard_confirmed_work_mode_for_objective_artifact(outcome, &first_pass, true);

        match guarded {
            WorkModeConfirmOutcome::Confirmed(c) => {
                assert_eq!(c.mode, WorkMode::GenericCode);
                assert_eq!(c.source, WorkModeConfirmationSource::FirstPass);
                assert!(
                    c.reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains("answer-only confirmation ignored"))
                );
            }
            other => panic!("expected guarded confirmation, got {other:?}"),
        }
    }

    #[test]
    fn confirmed_answer_only_is_preserved_without_artifact_objective() {
        let first_pass = make_classification(WorkMode::GenericCode);
        let outcome = WorkModeConfirmOutcome::Confirmed(WorkModeConfirmation {
            mode: WorkMode::AnswerOnly,
            confidence: 0.95,
            source: WorkModeConfirmationSource::SecondPassOverridden,
            reason: Some("read-only".to_string()),
        });

        let guarded = guard_confirmed_work_mode_for_objective_artifact(outcome, &first_pass, false);

        match guarded {
            WorkModeConfirmOutcome::Confirmed(c) => {
                assert_eq!(c.mode, WorkMode::AnswerOnly);
                assert_eq!(c.source, WorkModeConfirmationSource::SecondPassOverridden);
            }
            other => panic!("expected unguarded confirmation, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod task_kind_preflight_tests {
    use super::preflight_task_kind_skip_reason;
    use super::task_kind_confirm::TaskKindSkipReason;
    use crate::modes::plan_act::ExecutionMode;

    /// Issue #926 (AC7 / AC2 per-turn cap / DR2-001): the preflight gate skips
    /// (before any sidecar dispatch) on a consumed cap, Plan mode, or the env
    /// disable, and only otherwise allows the attempt. Cap takes precedence over
    /// Plan over env (the order the WorkMode mirror uses).
    #[test]
    fn preflight_skip_reason_precedence_and_pass_through() {
        // Cap consumed wins regardless of mode/env.
        assert_eq!(
            preflight_task_kind_skip_reason(true, ExecutionMode::Act, false),
            Some(TaskKindSkipReason::PerTurnCapConsumed)
        );
        assert_eq!(
            preflight_task_kind_skip_reason(true, ExecutionMode::Plan, true),
            Some(TaskKindSkipReason::PerTurnCapConsumed)
        );
        // AC7: Plan mode skips (cap not yet consumed).
        assert_eq!(
            preflight_task_kind_skip_reason(false, ExecutionMode::Plan, false),
            Some(TaskKindSkipReason::PlanMode)
        );
        // Env disable skips in Act mode.
        assert_eq!(
            preflight_task_kind_skip_reason(false, ExecutionMode::Act, true),
            Some(TaskKindSkipReason::EnvDisabled)
        );
        // Otherwise: no skip → the attempt proceeds.
        assert_eq!(
            preflight_task_kind_skip_reason(false, ExecutionMode::Act, false),
            None
        );
    }
}

#[cfg(test)]
mod project_profile_confirm_tests {
    use super::*;

    #[test]
    fn project_profile_confirm_requested_for_document_contract() {
        let contract = TaskContract::from_request("Write README.md with setup and usage sections.");

        assert!(should_request_project_profile_confirm(
            &contract,
            "Write README.md with setup and usage sections."
        ));
    }

    #[test]
    fn project_profile_confirm_skips_plain_coding_contract() {
        let contract = TaskContract::from_request(
            "Create a Rust library in src/lib.rs with tests and run cargo test.",
        );

        assert!(!should_request_project_profile_confirm(
            &contract,
            "Create a Rust library in src/lib.rs with tests and run cargo test."
        ));
    }

    #[test]
    fn project_profile_confirm_skips_state_packet_requests() {
        let contract = TaskContract::from_request(
            "STATE_CONTROL_PACKET {\"required_artifacts\":[]} Write README.md",
        );

        assert!(!should_request_project_profile_confirm(
            &contract,
            "STATE_CONTROL_PACKET {\"required_artifacts\":[]} Write README.md"
        ));
    }
}
