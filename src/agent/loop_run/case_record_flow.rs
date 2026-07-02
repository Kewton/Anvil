//! Case-record extraction + retrieval flow extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the two production entry points and one private helper:
//!
//! - `maybe_extract_case_record` — post-loop success-condition + persist
//!   (Issue #462).
//! - `try_inject_case_retrieval_message` — pre-prompt retrieval that
//!   builds and (when applicable) injects a `Relevant Local Cases:`
//!   system message into the next prompt (Issue #463).
//! - `persist_case_record` / `finish_case_record_extraction` — private
//!   helpers.
//!
//! Methods previously hung off `impl Agent` in `turn.rs`; converted to
//! free functions taking `&Agent` / `&mut Agent`, matching the
//! `anti_pattern_flow` / `actor_loop_flow` pattern. `pub(super)` limited
//! / no facade re-export (DR3-001).

use super::Agent;
use super::case_record_extract::{
    case_record_auto_test_active, case_record_extraction_succeeded, case_record_initial_feedback,
    derive_language_stack,
};
use super::turn_helpers::RetrievalInjection;
use crate::logging::log_llm_event;
use crate::modes::plan_act::ExecutionMode;
use crate::session::precaution::PrecautionStatus;
use crate::session::store::ConversationMessage;

/// Issue #462: post-loop CaseRecord extraction. Pure success-condition,
/// scrub, and persist; never calls Ollama / sidecars. Per-turn cap is
/// `case_record_extracted_this_turn` on `SessionSnapshot` (cleared at
/// `run_turn` head). Failures are logged via `agent.case_record.failed`
/// and never propagate.
///
/// DR3-002: gathers `verify_commands` from the agent layer (turn.rs)
/// and passes them into `case_record::extract` via a borrowed slice.
/// `language_stack` is derived here using `auto_test::has_*` helpers
/// (also agent-layer) for the same reason.
pub(super) fn maybe_extract_case_record(
    agent: &mut Agent,
    stats: &crate::agent::loop_run::summary::LoopStats,
    verify_commands: &[String],
) -> Option<crate::session::case_record::CaseRecord> {
    use crate::session::case_record;

    // Plan-mode gate: never extract in Plan mode.
    if agent.session.mode_state.mode == ExecutionMode::Plan {
        return None;
    }
    // Per-turn cap.
    if agent.session.case_record_extracted_this_turn {
        return None;
    }
    // Disable env.
    if case_record::case_record_disabled(|k| std::env::var(k)) {
        log_llm_event(
            "agent.case_record.disabled",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
            }),
        );
        agent.session.case_record_extracted_this_turn = true;
        return None;
    }

    // Success condition (Issue #462 spec).
    let Some(score) = agent.session.last_anvil_score.clone() else {
        // No AnvilScore computed for this turn (e.g., TransportError) — skip silently.
        return finish_case_record_extraction(agent, None);
    };
    let auto_test_active = case_record_auto_test_active(&score);
    // CB-002 (codex review fix): the auto_test success branch now also
    // requires `unsafe_actions_blocked == 0` to prevent a turn where
    // an unsafe command was blocked in the same turn from extracting
    // a CaseRecord solely because build / tests / artifact were green.
    // This matches the verifier-less fallback and
    // `case_photon_bridge::is_eligible_for_promotion`'s full_pass check.
    if !case_record_extraction_succeeded(
        &score,
        agent.session.repo_edit_succeeded_this_turn,
        agent.session.unsafe_blocks_this_turn,
    ) {
        return finish_case_record_extraction(agent, None);
    }

    // language_stack derivation (agent layer; reuses `auto_test::has_*`).
    let language_stack = derive_language_stack(&agent.work_root);

    // Build inputs.
    let active_task = agent.session.working_memory.active_task.clone();
    let active_precautions: Vec<crate::session::precaution::Precaution> =
        agent.session.working_memory.active_precautions.to_vec();
    // initial_feedback: take the kind of the latest recorded feedback as a
    // single-item list (Issue Out of Scope: rich N-frame history is for
    // CBR follow-up Issue).
    let initial_feedback = case_record_initial_feedback(agent.session.last_feedback.as_ref());
    let workspace_key = agent.session.workspace_key.clone();

    let inputs = case_record::CaseRecordInputs {
        workspace_key: &workspace_key,
        work_root: &agent.work_root,
        active_task: active_task.as_deref(),
        language_stack: &language_stack,
        initial_feedback: &initial_feedback,
        active_precautions: &active_precautions,
        changed_files: &stats.changed_files,
        verify_commands,
        anvil_score: &score,
        repo_edit_succeeded_this_turn: agent.session.repo_edit_succeeded_this_turn,
        unsafe_blocks_this_turn: agent.session.unsafe_blocks_this_turn,
        auto_test_active,
    };

    let started = std::time::Instant::now();
    let Some(record) = case_record::extract(&inputs) else {
        log_llm_event(
            "agent.case_record.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "extract_returned_none",
            }),
        );
        return finish_case_record_extraction(agent, None);
    };

    // Dry-run gate (DR3-002 / Issue): extract still runs so log payloads
    // can confirm the would-be case_id.
    if case_record::case_record_dry_run(|k| std::env::var(k)) {
        log_llm_event(
            "agent.case_record.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "dry_run",
                "case_id": record.case_id,
            }),
        );
        // DR2-008: Issue #604 — return the extracted record even on
        // dry_run so the post-loop auto-promote hook can still see what
        // would have been promoted (dry_run is observability-only).
        return finish_case_record_extraction(agent, Some(record));
    }

    let persist_ok = persist_case_record(agent, &record, started);
    finish_case_record_extraction(agent, persist_ok.then_some(record))
}

fn finish_case_record_extraction<T>(agent: &mut Agent, result: Option<T>) -> Option<T> {
    agent.session.case_record_extracted_this_turn = true;
    result
}

fn persist_case_record(
    agent: &Agent,
    record: &crate::session::case_record::CaseRecord,
    started: std::time::Instant,
) -> bool {
    use crate::session::case_record;

    let state_root = agent.session_store.state_root().to_path_buf();
    match case_record::persist(&state_root, record) {
        Ok(bytes) => {
            let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
            log_llm_event(
                "agent.case_record.extracted",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "case_id": record.case_id,
                    "bytes": bytes,
                    "compute_ms": compute_ms,
                }),
            );
            true
        }
        Err(case_record::PersistError::TooLarge { bytes }) => {
            log_llm_event(
                "agent.case_record.skipped",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "reason": "too_large",
                    "bytes": bytes,
                }),
            );
            false
        }
        Err(err) => {
            log_llm_event(
                "agent.case_record.failed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "error": err.to_string(),
                }),
            );
            false
        }
    }
}

/// Issue #463: build and (when applicable) inject a `Relevant Local Cases:`
/// system message into the next prompt. Called from the per-iteration
/// message-build path immediately after `working_memory_message`. Pure-
/// function retrieval; never calls Ollama / sidecars. Failures are logged
/// via `agent.case_retrieval.failed` and never propagate.
pub(super) fn try_inject_case_retrieval_message(agent: &mut Agent) -> Option<RetrievalInjection> {
    use crate::session::case_record::{
        PrecautionSnapshot, build_task_signature, capture_repo_fingerprint,
    };
    use crate::session::case_retrieval::{self, CaseRetrievalInputs, RetrievalOutcome};

    // 1. Plan mode → skipped(plan_mode), do not consume cap.
    if agent.session.mode_state.mode == ExecutionMode::Plan {
        log_llm_event(
            "agent.case_retrieval.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "plan_mode",
            }),
        );
        return None;
    }
    // 2. per-turn cap consumed → skipped(per_turn_cap_consumed).
    if agent.session.case_retrieval_invoked_this_turn {
        log_llm_event(
            "agent.case_retrieval.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "reason": "per_turn_cap_consumed",
            }),
        );
        return None;
    }
    // 3. Env disable → cap=true, disabled event.
    if case_retrieval::case_retrieval_disabled(|k| std::env::var(k)) {
        agent.session.case_retrieval_invoked_this_turn = true;
        log_llm_event(
            "agent.case_retrieval.disabled",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
            }),
        );
        return None;
    }

    // 4. Build inputs from the current SessionSnapshot view.
    let language_stack = derive_language_stack(&agent.work_root);
    let workspace_key = agent.session.workspace_key.clone();
    let active_task = agent.session.working_memory.active_task.clone();
    let task_signature = build_task_signature(active_task.as_deref(), &agent.work_root);
    let repo_fp = capture_repo_fingerprint(&workspace_key, &agent.work_root, &language_stack);
    let touched_files = agent.session.working_memory.touched_files.clone();
    let feedback_kind = agent.session.last_feedback.as_ref().map(|f| f.kind.clone());
    let prec_snapshots: Vec<PrecautionSnapshot> = agent
        .session
        .working_memory
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .map(PrecautionSnapshot::from)
        .collect();

    let dry_run = case_retrieval::case_retrieval_dry_run(|k| std::env::var(k));

    let inputs = CaseRetrievalInputs {
        current_task_signature: &task_signature,
        current_language_stack: &language_stack,
        current_repo_fingerprint: &repo_fp,
        current_touched_files: &touched_files,
        current_feedback_kind: feedback_kind,
        current_active_precautions: &prec_snapshots,
    };

    // 5. Consume the cap before retrieve so the failure path also accounts.
    agent.session.case_retrieval_invoked_this_turn = true;

    let state_root = agent.session_store.state_root().to_path_buf();
    match case_retrieval::retrieve_relevant_cases(&state_root, &inputs, dry_run) {
        Ok(RetrievalOutcome::Completed {
            candidate_count,
            selected,
            skipped_corrupt_count,
            compute_ms,
        }) => {
            let top_score = selected.first().map(|s| s.breakdown.total).unwrap_or(0.0);
            let top_case_id = selected
                .first()
                .map(|s| s.record.case_id.clone())
                .unwrap_or_default();
            let selected_reasons: Vec<&case_retrieval::CaseScoreBreakdown> =
                selected.iter().map(|s| &s.breakdown).collect();
            log_llm_event(
                "agent.case_retrieval.completed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "candidate_count": candidate_count,
                    "selected_count": selected.len(),
                    "top_score": top_score,
                    "top_case_id": top_case_id,
                    "threshold": 0.40_f32,
                    "compute_ms": compute_ms,
                    "skipped_corrupt_count": skipped_corrupt_count,
                    "selected_reasons": selected_reasons,
                }),
            );
            // Issue #471 / DR2-005: build CaseRetrievalSummary before
            // format_for_prompt consumes `selected`.
            agent.last_case_retrieval_summary =
                Some(crate::session::eval_log::CaseRetrievalSummary {
                    selected: selected.len(),
                    scores: selected.iter().map(|s| s.breakdown.clone()).collect(),
                });
            // Issue #555: capture selected IDs for photon mapper before
            // format_for_prompt consumes `selected`.
            let selected_ids: Vec<String> =
                selected.iter().map(|s| s.record.case_id.clone()).collect();
            case_retrieval::format_for_prompt(&selected)
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
                "agent.case_retrieval.skipped",
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
                "agent.case_retrieval.failed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "error": error,
                }),
            );
            None
        }
    }
}
