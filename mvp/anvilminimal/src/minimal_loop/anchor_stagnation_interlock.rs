use std::path::Path;

use serde_json::json;

use crate::eval_events;

use super::edit_anchor_recovery::EditAnchorFailureSummary;
use super::repair_pressure::{PressureInputs, PressureLevel, PressureState, transition};
use super::stagnation_escalation::{
    ReadOnlyStagnationStage, WriteRequiredSelectionReason, WriteRequiredTargetSelection,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchorStagnationDecision {
    pub(crate) stage: ReadOnlyStagnationStage,
    pressure_state: PressureState,
    pub(crate) anchor_failure: Option<EditAnchorFailureSummary>,
}

impl AnchorStagnationDecision {
    pub(crate) fn anchor_interlocked(&self) -> bool {
        self.anchor_failure.is_some()
    }

    pub(crate) fn write_required_selection(&self) -> Option<WriteRequiredTargetSelection> {
        if self.stage != ReadOnlyStagnationStage::WriteRequired {
            return None;
        }
        let anchor = self.anchor_failure.as_ref()?;
        Some(WriteRequiredTargetSelection {
            selected_targets: vec![anchor.path.clone()],
            selection_reason: WriteRequiredSelectionReason::AnchorFailure,
        })
    }

    pub(crate) fn full_file_write_feedback(&self, objective: &str) -> Option<String> {
        let anchor = self.anchor_failure.as_ref()?;
        Some(format!(
            "Edit anchors already failed for `{}` (anchor_failures={}). Do not keep inspecting or retrying anchored Edit. Update `{}` with a full-file Write now using the complete corrected file content. Objective: {objective}. read_only_streak={}; effective_read_only_streak={}",
            anchor.path,
            anchor.failure_count,
            anchor.path,
            self.pressure_state.counters.read_only_streak,
            self.pressure_state.effective_read_only_streak
        ))
    }

    pub(crate) fn write_required_feedback(&self, attempt_limit: usize) -> Option<String> {
        let anchor = self.anchor_failure.as_ref()?;
        Some(format!(
            "Read-only stagnation is interlocked with edit anchor failures. Use a full-file Write for `{}` now; do not use another anchored Edit because anchors already failed. Read, Grep, Glob, Bash, and prose-only responses are suspended until `{}` is written. read_only_streak={}; anchor_failures={}; write_required_no_write_limit={attempt_limit}",
            anchor.path,
            anchor.path,
            self.pressure_state.counters.read_only_streak,
            anchor.failure_count
        ))
    }

    pub(crate) fn diagnostic_feedback(&self) -> String {
        self.anchor_failure
            .as_ref()
            .map(|anchor| {
                format!(
                    "Anchor interlock: `{}` has {} edit_anchor_not_found failure(s); update it with full-file Write content instead of another anchored Edit.",
                    anchor.path, anchor.failure_count
                )
            })
            .unwrap_or_default()
    }
}

pub(crate) fn read_only_stagnation_decision(
    read_only_streak: usize,
    anchor_failure: Option<EditAnchorFailureSummary>,
) -> Option<AnchorStagnationDecision> {
    let anchor_failure = anchor_failure
        .filter(|failure| failure.failure_count > 0 && !failure.path.trim().is_empty());
    let pressure_state = transition(PressureInputs {
        read_only_streak,
        anchor_failures: anchor_failure
            .as_ref()
            .map(|failure| failure.failure_count)
            .unwrap_or_default(),
        anchor_target: anchor_failure.as_ref().map(|failure| failure.path.clone()),
        ..PressureInputs::default()
    });
    let stage = match pressure_state.feedback_level {
        Some(PressureLevel::Intervention) => ReadOnlyStagnationStage::Intervention,
        Some(PressureLevel::CompactRestatement) => ReadOnlyStagnationStage::CompactRestatement,
        Some(PressureLevel::WriteRequired) => ReadOnlyStagnationStage::WriteRequired,
        _ => return None,
    };
    Some(AnchorStagnationDecision {
        stage,
        pressure_state,
        anchor_failure,
    })
}

pub(crate) fn emit_interlock_event(
    eval_events_path: Option<&Path>,
    decision: &AnchorStagnationDecision,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
) {
    let Some(anchor) = decision.anchor_failure.as_ref() else {
        return;
    };
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "anchor_stagnation_interlock",
            "stage": decision.stage.as_str(),
            "path": anchor.path,
            "anchor_failures": anchor.failure_count,
            "streak": decision.pressure_state.counters.read_only_streak,
            "effective_streak": decision.pressure_state.effective_read_only_streak,
            "session_scope": session_scope,
            "step_kind": step_kind,
            "phase_scope": phase_scope.unwrap_or(""),
        }),
    );
}
