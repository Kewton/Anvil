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

use super::Agent;
use super::interrupt::InterruptFlag;
use super::reminder::{
    self, ReminderInputs, ReminderOutcome, build_log_payload as build_reminder_log_payload,
};
use crate::logging::log_llm_event;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{OllamaClient, SIDECAR_SUMMARY_TIMEOUT_SECS};
use crate::session::feedback::{FeedbackFrame, FeedbackKind};
use crate::session::precaution::PrecautionStatus;
use crate::session::store::ConversationMessage;

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

pub(super) fn maybe_invoke_reminder(agent: &mut Agent, interrupt_flag: &InterruptFlag) {
    let Some(kind) = reminder_feedback_kind(agent) else {
        return;
    };
    let Some(context) = prepare_reminder_context(agent, kind, interrupt_flag) else {
        return;
    };
    let sidecar_model = context
        .model
        .clone()
        .expect("sidecar_available was checked by reminder gate");
    let Some(reminder_client) = try_clone_reminder_client(agent, &context) else {
        return;
    };
    let outcome = run_reminder_sidecar(agent, &context, &reminder_client, &sidecar_model);
    agent.reminder_called_this_turn = true;
    context.log_outcome(agent.current_turn_index, &outcome, true);
}

pub(super) fn reminder_feedback_kind(agent: &Agent) -> Option<FeedbackKind> {
    match &agent.session.last_feedback {
        Some(frame) if reminder::kind_eligible(&frame.kind) => Some(frame.kind.clone()),
        _ => None,
    }
}

pub(super) fn reminder_gate(
    agent: &Agent,
    interrupt_flag: &InterruptFlag,
) -> reminder::ReminderGate {
    reminder::ReminderGate {
        disabled_by_env: reminder::reminder_disabled(|key| std::env::var_os(key)),
        sidecar_available: agent.models.sidecar.is_some(),
        kind_eligible: true,
        plan_mode: agent.session.mode_state.mode == ExecutionMode::Plan,
        interrupted: interrupt_flag.is_set(),
        per_turn_already_called: agent.reminder_called_this_turn,
    }
}

pub(super) fn prepare_reminder_context(
    agent: &Agent,
    kind: FeedbackKind,
    interrupt_flag: &InterruptFlag,
) -> Option<ReminderCallContext> {
    let session_id = agent.session_store.session_id().to_string();
    let model = agent.models.sidecar.clone();
    let gate = reminder_gate(agent, interrupt_flag);
    if let Some(skip_reason) = gate.skip_reason() {
        ReminderCallContext {
            session_id,
            model,
            kind: kind.clone(),
            frame: FeedbackFrame::default(),
            mode_label: "act",
            active_precautions_summary: String::new(),
            touched_files: Vec::new(),
            user_task: String::new(),
            workspace_root: agent.work_root.clone(),
            active_precautions_at_call_time: Vec::new(),
            anvil_score: None,
            anvil_score_from_current_turn: false,
        }
        .log_outcome(
            agent.current_turn_index,
            &ReminderOutcome::Skipped {
                skip_reason,
                feedback_kind: Some(kind),
            },
            false,
        );
        return None;
    }
    let frame = agent
        .session
        .last_feedback
        .clone()
        .expect("kind_eligible implies last_feedback is Some");
    Some(ReminderCallContext {
        session_id,
        model,
        kind,
        frame,
        mode_label: reminder_mode_label(agent),
        active_precautions_summary: agent
            .session
            .working_memory
            .format_for_prompt()
            .unwrap_or_else(|| "(none)".to_string()),
        touched_files: agent.session.working_memory.touched_files.clone(),
        user_task: agent
            .session
            .working_memory
            .active_task
            .clone()
            .unwrap_or_default(),
        workspace_root: agent.work_root.clone(),
        active_precautions_at_call_time: active_precautions_at_call_time(agent),
        anvil_score: agent.session.last_anvil_score.clone(),
        anvil_score_from_current_turn: agent.anvil_score_computed_this_turn,
    })
}

pub(super) fn reminder_mode_label(agent: &Agent) -> &'static str {
    match agent.session.mode_state.mode {
        ExecutionMode::Act => "act",
        ExecutionMode::Plan => "plan",
    }
}

pub(super) fn active_precautions_at_call_time(agent: &Agent) -> Vec<String> {
    agent
        .session
        .working_memory
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .map(|p| p.text.clone())
        .collect()
}

pub(super) fn try_clone_reminder_client(
    agent: &mut Agent,
    context: &ReminderCallContext,
) -> Option<OllamaClient> {
    match agent
        .client
        .clone_with_overrides(SIDECAR_SUMMARY_TIMEOUT_SECS, 384)
    {
        Ok(client) => Some(client),
        Err(err) => {
            agent.reminder_called_this_turn = true;
            context.log_outcome(
                agent.current_turn_index,
                &ReminderOutcome::Failed {
                    reason: reminder::FailureReason::LlmCall(format!(
                        "clone_with_overrides: {err}"
                    )),
                    latency_ms: 0,
                    prompt_log: String::new(),
                    response_raw_log: String::new(),
                    feedback_kind: context.kind.clone(),
                },
                false,
            );
            None
        }
    }
}

pub(super) fn run_reminder_sidecar(
    agent: &mut Agent,
    context: &ReminderCallContext,
    reminder_client: &OllamaClient,
    sidecar_model: &str,
) -> ReminderOutcome {
    reminder::run_reminder_with_strategy(
        context.inputs(),
        &mut agent.session.working_memory,
        &context.workspace_root,
        |prompt| {
            reminder_client.chat_text(
                sidecar_model,
                &[ConversationMessage::user(prompt.to_string())],
            )
        },
    )
}
