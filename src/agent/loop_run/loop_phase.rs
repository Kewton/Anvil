//! Actor loop phase telemetry.
//!
//! This module is intentionally observability-only. `LoopPhase` names the
//! controller boundary currently executing, while terminal authority remains in
//! the existing actor-loop flow.

use crate::logging::log_llm_event;
use crate::modes::plan_act::{ExecutionMode, TaskProfile};

use super::Agent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopPhase {
    ContractAdmitted,
    GenerationPrepared,
    ModelRequestPrepared,
    ToolExecution,
    ArtifactEvidenceReconciliation,
    VerifierRepairDispatch,
    DoneOrSafeStop,
}

impl LoopPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ContractAdmitted => "contract_admitted",
            Self::GenerationPrepared => "generation_prepared",
            Self::ModelRequestPrepared => "model_request_prepared",
            Self::ToolExecution => "tool_execution",
            Self::ArtifactEvidenceReconciliation => "artifact_evidence_reconciliation",
            Self::VerifierRepairDispatch => "verifier_repair_dispatch",
            Self::DoneOrSafeStop => "done_safe_stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopPhaseTransition {
    Enter,
    Exit,
}

impl LoopPhaseTransition {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Exit => "exit",
        }
    }
}

pub(super) fn emit_loop_phase(
    agent: &Agent,
    phase: LoopPhase,
    transition: LoopPhaseTransition,
    iter_count: usize,
    detail: serde_json::Value,
) {
    log_llm_event(
        "agent.loop_phase.transition",
        build_loop_phase_payload(
            agent.session_store.session_id(),
            agent.current_turn_index,
            iter_count,
            agent.session.mode_state.mode,
            agent.session.mode_state.task_profile,
            phase,
            transition,
            detail,
        ),
    );
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LoopPhaseEvent {
    phase: LoopPhase,
    transition: LoopPhaseTransition,
    iter_count: usize,
    detail: serde_json::Value,
}

impl LoopPhaseEvent {
    fn new(
        phase: LoopPhase,
        transition: LoopPhaseTransition,
        iter_count: usize,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            phase,
            transition,
            iter_count,
            detail,
        }
    }

    pub(super) fn emit(self, agent: &Agent) {
        emit_loop_phase(
            agent,
            self.phase,
            self.transition,
            self.iter_count,
            self.detail,
        );
    }

    #[cfg(test)]
    fn payload_for_test(
        &self,
        session_id: &str,
        turn_index: usize,
        mode: ExecutionMode,
        task_profile: TaskProfile,
    ) -> serde_json::Value {
        build_loop_phase_payload(
            session_id,
            turn_index,
            self.iter_count,
            mode,
            task_profile,
            self.phase,
            self.transition,
            self.detail.clone(),
        )
    }
}

pub(super) fn contract_admitted_exit_event(contract_present: bool) -> LoopPhaseEvent {
    LoopPhaseEvent::new(
        LoopPhase::ContractAdmitted,
        LoopPhaseTransition::Exit,
        0,
        serde_json::json!({
            "contract_present": contract_present,
        }),
    )
}

pub(super) fn generation_prepared_enter_event(
    iter_count: usize,
    reply_tool_call_count: usize,
) -> LoopPhaseEvent {
    LoopPhaseEvent::new(
        LoopPhase::GenerationPrepared,
        LoopPhaseTransition::Enter,
        iter_count,
        serde_json::json!({
            "reply_tool_call_count": reply_tool_call_count,
        }),
    )
}

pub(super) fn generation_prepared_exit_event(
    iter_count: usize,
    current_reply_tool_call_count: usize,
    prepared_tool_call_count: usize,
) -> LoopPhaseEvent {
    LoopPhaseEvent::new(
        LoopPhase::GenerationPrepared,
        LoopPhaseTransition::Exit,
        iter_count,
        serde_json::json!({
            "current_reply_tool_call_count": current_reply_tool_call_count,
            "prepared_tool_call_count": prepared_tool_call_count,
        }),
    )
}

pub(super) fn tool_execution_enter_event(
    iter_count: usize,
    prepared_tool_call_count: usize,
) -> LoopPhaseEvent {
    LoopPhaseEvent::new(
        LoopPhase::ToolExecution,
        LoopPhaseTransition::Enter,
        iter_count,
        serde_json::json!({
            "prepared_tool_call_count": prepared_tool_call_count,
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_loop_phase_payload(
    session_id: &str,
    turn_index: usize,
    iter_count: usize,
    mode: ExecutionMode,
    task_profile: TaskProfile,
    phase: LoopPhase,
    transition: LoopPhaseTransition,
    detail: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "iteration_seq": iter_count,
        "mode": mode_label(mode),
        "task_profile": task_profile.as_str(),
        "phase": phase.label(),
        "transition": transition.label(),
        "detail": detail,
    })
}

fn mode_label(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Plan => "plan",
        ExecutionMode::Act => "act",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_phase_labels_cover_public_telemetry_contract() {
        let labels = [
            LoopPhase::ContractAdmitted.label(),
            LoopPhase::GenerationPrepared.label(),
            LoopPhase::ModelRequestPrepared.label(),
            LoopPhase::ToolExecution.label(),
            LoopPhase::ArtifactEvidenceReconciliation.label(),
            LoopPhase::VerifierRepairDispatch.label(),
            LoopPhase::DoneOrSafeStop.label(),
        ];

        assert_eq!(
            labels,
            [
                "contract_admitted",
                "generation_prepared",
                "model_request_prepared",
                "tool_execution",
                "artifact_evidence_reconciliation",
                "verifier_repair_dispatch",
                "done_safe_stop",
            ]
        );
    }

    #[test]
    fn loop_phase_payload_contains_only_structured_labels() {
        let payload = build_loop_phase_payload(
            "session-1",
            7,
            3,
            ExecutionMode::Act,
            TaskProfile::Coding,
            LoopPhase::ToolExecution,
            LoopPhaseTransition::Enter,
            serde_json::json!({
                "tool_count": 2,
            }),
        );

        assert_eq!(payload["session_id"], "session-1");
        assert_eq!(payload["turn_index"], 7);
        assert_eq!(payload["iteration_seq"], 3);
        assert_eq!(payload["mode"], "act");
        assert_eq!(payload["task_profile"], "coding");
        assert_eq!(payload["phase"], "tool_execution");
        assert_eq!(payload["transition"], "enter");
        assert_eq!(payload["detail"]["tool_count"], 2);
    }

    #[test]
    fn actor_loop_phase_event_builders_pin_snapshot_payloads() {
        let contract = contract_admitted_exit_event(true).payload_for_test(
            "session-1",
            2,
            ExecutionMode::Act,
            TaskProfile::Generic,
        );
        assert_eq!(contract["phase"], "contract_admitted");
        assert_eq!(contract["transition"], "exit");
        assert_eq!(contract["iteration_seq"], 0);
        assert_eq!(contract["detail"]["contract_present"], true);

        let generation = generation_prepared_exit_event(4, 3, 2).payload_for_test(
            "session-1",
            2,
            ExecutionMode::Act,
            TaskProfile::Generic,
        );
        assert_eq!(generation["phase"], "generation_prepared");
        assert_eq!(generation["transition"], "exit");
        assert_eq!(generation["iteration_seq"], 4);
        assert_eq!(generation["detail"]["current_reply_tool_call_count"], 3);
        assert_eq!(generation["detail"]["prepared_tool_call_count"], 2);

        let tool = tool_execution_enter_event(5, 1).payload_for_test(
            "session-1",
            2,
            ExecutionMode::Act,
            TaskProfile::Generic,
        );
        assert_eq!(tool["phase"], "tool_execution");
        assert_eq!(tool["transition"], "enter");
        assert_eq!(tool["detail"]["prepared_tool_call_count"], 1);
    }
}
