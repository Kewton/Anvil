//! Issue #684 (parent #680, Phase 4): reminder dispatch orchestration
//! extracted from `turn.rs`.
//!
//! Hosts:
//!
//! * `ReminderCallContext` — bounded snapshot of the state needed to
//!   call the reminder sidecar (session_id / model / kind / frame /
//!   mode label / per-turn touched files + working memory snapshot /
//!   active precautions / current-turn AnvilScore view). Built once
//!   per turn by the `prepare_reminder_context` helper on `impl Agent`
//!   and consumed by the dispatch + log path.
//!
//! Phase 4 scope (Issue #684): this PR migrates the **type +
//! `impl ReminderCallContext` only**. The dispatch methods on
//! `impl Agent` (`maybe_invoke_reminder` / `prepare_reminder_context`
//! / `try_clone_reminder_client` / `run_reminder_sidecar` / friends)
//! stay in `turn.rs` for now and will be migrated in follow-up PRs,
//! mirroring the Phase 1 (actor_loop_flow) / Phase 3 (scaffold_pipeline)
//! pattern.
//!
//! The reminder helper sibling (`reminder.rs`) keeps its public SSOT
//! (`ReminderInputs` / `ReminderOutcome` / `build_log_payload`); only
//! the orchestration that assembles the context lives here.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use std::path::PathBuf;

use super::reminder::{
    ReminderInputs, ReminderOutcome, build_log_payload as build_reminder_log_payload,
};
use crate::logging::log_llm_event;
use crate::session::feedback::{FeedbackFrame, FeedbackKind};

pub(super) struct ReminderCallContext {
    pub(super) session_id: String,
    pub(super) model: Option<String>,
    pub(super) kind: FeedbackKind,
    pub(super) frame: FeedbackFrame,
    pub(super) mode_label: &'static str,
    pub(super) active_precautions_summary: String,
    pub(super) touched_files: Vec<String>,
    pub(super) user_task: String,
    pub(super) workspace_root: PathBuf,
    pub(super) active_precautions_at_call_time: Vec<String>,
    pub(super) anvil_score: Option<crate::session::anvil_score::AnvilScore>,
    pub(super) anvil_score_from_current_turn: bool,
}

impl ReminderCallContext {
    pub(super) fn inputs(&self) -> ReminderInputs<'_> {
        let anvil_score = self.anvil_score.as_ref().map(|score| {
            if self.anvil_score_from_current_turn {
                crate::session::anvil_score::AnvilScoreSnapshot::CurrentTurn(score)
            } else {
                crate::session::anvil_score::AnvilScoreSnapshot::PreviousTurn(score)
            }
        });
        ReminderInputs {
            user_task: &self.user_task,
            mode_label: self.mode_label,
            plan_summary: None,
            active_precautions_summary: &self.active_precautions_summary,
            frame: &self.frame,
            working_memory_touched: &self.touched_files,
            anvil_score,
            active_precautions_at_call_time: &self.active_precautions_at_call_time,
        }
    }

    pub(super) fn log_outcome(
        &self,
        turn_index: usize,
        outcome: &ReminderOutcome,
        include_inputs: bool,
    ) {
        let maybe_inputs = include_inputs.then(|| self.inputs());
        let (event, payload) = build_reminder_log_payload(
            outcome,
            &self.session_id,
            self.model.as_deref(),
            turn_index,
            maybe_inputs.as_ref(),
        );
        log_llm_event(event, payload);
    }
}
