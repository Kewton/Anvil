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
}
