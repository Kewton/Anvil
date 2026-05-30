//! Anti-pattern extraction + retrieval flow extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the two production entry points and their private helpers:
//!
//! - `maybe_extract_anti_pattern` — post-loop upsert of an AntiPattern
//!   record keyed by `(workspace_key, task_signature, feedback_kind)`
//!   (Issue #464).
//! - `try_inject_anti_pattern_message` — pre-prompt retrieval that
//!   builds and (when applicable) injects an `Avoid Patterns:` system
//!   message into the next prompt (Issue #464).
//!
//! Methods previously hung off `impl Agent` in `turn.rs`; converted to
//! free functions taking `&Agent` / `&mut Agent`, matching the
//! `actor_loop_flow` extraction pattern. `pub(super)` limited / no
//! facade re-export (DR3-001).

use super::Agent;
use super::turn_helpers::RetrievalInjection;
use crate::logging::log_llm_event;
use crate::modes::plan_act::ExecutionMode;
use crate::session::feedback::{FeedbackFrame, FeedbackKind};
use crate::session::store::ConversationMessage;

use super::case_record_extract::derive_language_stack;
use super::small_helpers::anti_pattern_failed_action_summary;

/// Issue #464: post-loop AntiPattern extraction. Mirrors
/// `maybe_extract_case_record` but triggers on **failure** turns instead
/// of success. Upserts a record keyed by (workspace_key, task_signature,
/// feedback_kind); the second + N-th occurrence increments `repeat_count`.
/// Pure upsert / scrub / persist; no sidecar / LLM calls.
pub(super) fn maybe_extract_anti_pattern(agent: &mut Agent) {
    use crate::session::anti_pattern;

    let Some(frame) = anti_pattern_extraction_feedback(agent) else {
        return;
    };
    let kind = frame.kind.clone();

    if anti_pattern::anti_pattern_dry_run(|k| std::env::var(k)) {
        log_anti_pattern_skip(agent, "dry_run", Some(&kind), None);
        finish_anti_pattern_extraction(agent);
        return;
    }

    let summary_owned = anti_pattern_failed_action_summary(&frame);
    let language_stack = derive_language_stack(&agent.work_root);
    let active_task = agent.session.working_memory.active_task.clone();
    let workspace_key = agent.session.workspace_key.clone();
    let touched_files = agent.session.working_memory.touched_files.clone();
    let inputs = anti_pattern::AntiPatternRecordInputs {
        workspace_key: &workspace_key,
        work_root: &agent.work_root,
        active_task: active_task.as_deref(),
        language_stack: &language_stack,
        touched_files: &touched_files,
        feedback_kind: kind.clone(),
        failed_action_summary: &summary_owned,
    };

    let started = std::time::Instant::now();
    let state_root = agent.session_store.state_root().to_path_buf();
    log_anti_pattern_extract_result(
        agent,
        anti_pattern::extract_or_increment(&state_root, &inputs),
        started,
    );
    finish_anti_pattern_extraction(agent);
}

fn anti_pattern_extraction_feedback(agent: &mut Agent) -> Option<FeedbackFrame> {
    use crate::session::anti_pattern;

    if agent.session.mode_state.mode == ExecutionMode::Plan {
        return None;
    }
    if agent.session.anti_pattern_extracted_this_turn {
        return None;
    }
    if anti_pattern::anti_pattern_disabled(|k| std::env::var(k)) {
        log_llm_event(
            "agent.anti_pattern.disabled",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
            }),
        );
        finish_anti_pattern_extraction(agent);
        return None;
    }
    let Some(frame) = agent.session.last_feedback.clone() else {
        finish_anti_pattern_extraction(agent);
        return None;
    };
    if !anti_pattern::is_repeat_eligible_kind(&frame.kind) {
        finish_anti_pattern_extraction(agent);
        return None;
    }
    Some(frame)
}

fn finish_anti_pattern_extraction(agent: &mut Agent) {
    agent.session.anti_pattern_extracted_this_turn = true;
}

fn log_anti_pattern_skip(
    agent: &Agent,
    reason: &str,
    feedback_kind: Option<&FeedbackKind>,
    bytes: Option<usize>,
) {
    let mut payload = serde_json::json!({
        "session_id": agent.session_store.session_id(),
        "reason": reason,
    });
    if let Some(kind) = feedback_kind {
        payload["feedback_kind"] = serde_json::to_value(kind).unwrap_or_default();
    }
    if let Some(bytes) = bytes {
        payload["bytes"] = serde_json::json!(bytes);
    }
    log_llm_event("agent.anti_pattern.skipped", payload);
}

fn log_anti_pattern_extracted(
    agent: &Agent,
    record: &crate::session::anti_pattern::AntiPatternRecord,
    outcome: &str,
    started: std::time::Instant,
) {
    let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
    log_llm_event(
        "agent.anti_pattern.extracted",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "anti_pattern_id": record.anti_pattern_id,
            "outcome": outcome,
            "repeat_count": record.repeat_count,
            "feedback_kind": serde_json::to_value(&record.feedback_kind).unwrap_or_default(),
            "compute_ms": compute_ms,
        }),
    );
}

fn log_anti_pattern_extract_result(
    agent: &Agent,
    result: Result<
        crate::session::anti_pattern::ExtractOutcome,
        crate::session::anti_pattern::PersistError,
    >,
    started: std::time::Instant,
) {
    use crate::session::anti_pattern::{ExtractOutcome, PersistError};

    match result {
        Ok(ExtractOutcome::Created(record)) => {
            log_anti_pattern_extracted(agent, &record, "created", started);
        }
        Ok(ExtractOutcome::Incremented(record)) => {
            log_anti_pattern_extracted(agent, &record, "incremented", started);
        }
        Ok(ExtractOutcome::SkippedIneligibleKind) => {
            log_anti_pattern_skip(agent, "ineligible_kind", None, None);
        }
        Ok(ExtractOutcome::SkippedNoActiveTask) => {
            log_anti_pattern_skip(agent, "no_active_task", None, None);
        }
        Err(PersistError::TooLarge { bytes }) => {
            log_anti_pattern_skip(agent, "too_large", None, Some(bytes));
        }
        Err(err) => {
            log_llm_event(
                "agent.anti_pattern.failed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "error": err.to_string(),
                }),
            );
        }
    }
}

/// Issue #464: build and (when applicable) inject an `Avoid Patterns:`
/// system message into the next prompt. Mirrors
/// `try_inject_case_retrieval_message` but pulls from
/// `state_root/anti_patterns/`.
pub(super) fn try_inject_anti_pattern_message(agent: &mut Agent) -> Option<RetrievalInjection> {
    use crate::session::anti_pattern::{self, AntiPatternRetrievalInputs, RetrievalOutcome};
    use crate::session::case_record::{build_task_signature, capture_repo_fingerprint};

    // 1. Plan mode → skipped, do not consume cap.
    if agent.session.mode_state.mode == ExecutionMode::Plan {
        log_llm_event(
            "agent.anti_pattern.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "plan_mode",
            }),
        );
        return None;
    }
    // 2. per-turn cap consumed.
    if agent.session.anti_pattern_retrieval_invoked_this_turn {
        log_llm_event(
            "agent.anti_pattern.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "per_turn_cap_consumed",
            }),
        );
        return None;
    }
    // 3. Env disable.
    if anti_pattern::anti_pattern_disabled(|k| std::env::var(k)) {
        agent.session.anti_pattern_retrieval_invoked_this_turn = true;
        log_llm_event(
            "agent.anti_pattern.disabled",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
            }),
        );
        return None;
    }

    // 4. Build inputs.
    let language_stack = derive_language_stack(&agent.work_root);
    let workspace_key = agent.session.workspace_key.clone();
    let active_task = agent.session.working_memory.active_task.clone();
    let task_signature = build_task_signature(active_task.as_deref(), &agent.work_root);
    let repo_fp = capture_repo_fingerprint(&workspace_key, &agent.work_root, &language_stack);
    let touched_files = agent.session.working_memory.touched_files.clone();
    let feedback_kind = agent.session.last_feedback.as_ref().map(|f| f.kind.clone());

    let dry_run = anti_pattern::anti_pattern_dry_run(|k| std::env::var(k));
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: &task_signature,
        current_language_stack: &language_stack,
        current_repo_fingerprint: &repo_fp,
        current_touched_files: &touched_files,
        current_feedback_kind: feedback_kind,
    };

    agent.session.anti_pattern_retrieval_invoked_this_turn = true;

    let state_root = agent.session_store.state_root().to_path_buf();
    match anti_pattern::retrieve_relevant_anti_patterns(&state_root, &inputs, dry_run) {
        Ok(RetrievalOutcome::Completed {
            candidate_count,
            selected,
            skipped_corrupt_count,
            compute_ms,
        }) => {
            let top_score = selected.first().map(|s| s.breakdown.total).unwrap_or(0.0);
            let top_id = selected
                .first()
                .map(|s| s.record.anti_pattern_id.clone())
                .unwrap_or_default();
            log_llm_event(
                "agent.anti_pattern.retrieved",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "candidate_count": candidate_count,
                    "selected_count": selected.len(),
                    "top_score": top_score,
                    "top_anti_pattern_id": top_id,
                    "threshold": anti_pattern::ANTI_PATTERN_RETRIEVAL_SCORE_THRESHOLD,
                    "compute_ms": compute_ms,
                    "skipped_corrupt_count": skipped_corrupt_count,
                }),
            );
            // Issue #555: capture selected IDs for photon mapper before
            // format_for_prompt consumes `selected`.
            let selected_ids: Vec<String> = selected
                .iter()
                .map(|s| s.record.anti_pattern_id.clone())
                .collect();
            anti_pattern::format_for_prompt(&selected)
                .map(ConversationMessage::system)
                .map(|message| RetrievalInjection {
                    message,
                    selected_ids,
                })
        }
        Ok(RetrievalOutcome::Skipped {
            reason,
            candidate_count,
            skipped_corrupt_count,
            compute_ms,
        }) => {
            log_llm_event(
                "agent.anti_pattern.skipped",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "reason": reason.as_log_str(),
                    "candidate_count": candidate_count,
                    "skipped_corrupt_count": skipped_corrupt_count,
                    "compute_ms": compute_ms,
                }),
            );
            None
        }
        Err(error) => {
            log_llm_event(
                "agent.anti_pattern.failed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "error": error,
                }),
            );
            None
        }
    }
}
