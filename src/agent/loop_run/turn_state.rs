//! Turn-local state owned by `handle_user_message`.
//!
//! This module keeps reset-only, per-user-turn carriers together so the turn
//! boundary can be audited without scanning every `Agent` field.

#[derive(Debug)]
pub(super) struct TurnState {
    pub(super) safe_stop_report_emitted: std::collections::HashSet<super::repair_job::StopReason>,
    pub(super) last_active_job_selection: Option<super::active_job_arbiter::ActiveJobSelection>,
    pub(super) job_report_dedup_keys: std::collections::HashSet<String>,
    pub(super) last_behavior_contract_projection_event:
        Option<super::required_behavior::BehaviorProjectionEventKey>,
    pub(super) last_pam_decision_this_turn: Option<super::pam_advisory::PamAdvisoryDecision>,
    pub(super) last_pam_unused_reason_this_turn: Option<String>,
}

impl TurnState {
    pub(super) fn new() -> Self {
        Self {
            safe_stop_report_emitted: std::collections::HashSet::new(),
            last_active_job_selection: None,
            job_report_dedup_keys: std::collections::HashSet::new(),
            last_behavior_contract_projection_event: None,
            last_pam_decision_this_turn: None,
            last_pam_unused_reason_this_turn: None,
        }
    }

    pub(super) fn reset_dedup_state(&mut self) {
        self.safe_stop_report_emitted.clear();
        self.last_active_job_selection = None;
        self.job_report_dedup_keys.clear();
        self.last_behavior_contract_projection_event = None;
    }

    pub(super) fn reset_pam_state(&mut self) {
        self.last_pam_decision_this_turn = None;
        self.last_pam_unused_reason_this_turn = None;
    }

    pub(super) fn pam_decision(&self) -> Option<&super::pam_advisory::PamAdvisoryDecision> {
        self.last_pam_decision_this_turn.as_ref()
    }

    pub(super) fn pam_unused_reason(&self) -> Option<&str> {
        self.last_pam_unused_reason_this_turn.as_deref()
    }

    pub(super) fn record_pam_decision(
        &mut self,
        decision: super::pam_advisory::PamAdvisoryDecision,
    ) {
        self.last_pam_decision_this_turn = Some(decision);
        self.last_pam_unused_reason_this_turn = None;
    }

    pub(super) fn record_pam_unused_reason(&mut self, reason: &str) {
        if self.last_pam_decision_this_turn.is_none()
            && self.last_pam_unused_reason_this_turn.is_none()
        {
            self.last_pam_unused_reason_this_turn = Some(reason.to_string());
        }
    }

    #[cfg(test)]
    pub(super) fn dedup_state_is_empty(&self) -> bool {
        self.safe_stop_report_emitted.is_empty()
            && self.last_active_job_selection.is_none()
            && self.job_report_dedup_keys.is_empty()
            && self.last_behavior_contract_projection_event.is_none()
    }
}

impl Default for TurnState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_state_reset_clears_grouped_dedup_state() {
        let mut state = TurnState::new();
        state
            .safe_stop_report_emitted
            .insert(super::super::repair_job::StopReason::RepairExhausted);
        state.last_active_job_selection =
            Some(super::super::active_job_arbiter::ActiveJobSelection {
                selected: None,
                rejected: Vec::new(),
            });
        state
            .job_report_dedup_keys
            .insert("agent.repair.report::x".to_string());
        state.last_behavior_contract_projection_event = Some(
            super::super::required_behavior::BehaviorProjectionEventKey {
                schema_version: 1,
                consumer: "test",
                confidence_bucket: 7,
                fields_used: vec!["behavior_goal"],
            },
        );

        assert!(!state.dedup_state_is_empty());
        state.reset_dedup_state();
        assert!(state.dedup_state_is_empty());
    }

    #[test]
    fn turn_state_reset_clears_pam_state() {
        let mut state = state_with_pam_unused_reason();
        state.reset_pam_state();

        assert!(state.pam_decision().is_none());
        assert!(state.pam_unused_reason().is_none());
    }

    #[test]
    fn pam_unused_reason_is_single_assignment_until_decision() {
        let mut state = TurnState::new();
        state.record_pam_unused_reason("first");
        state.record_pam_unused_reason("second");

        assert_eq!(state.pam_unused_reason(), Some("first"));
    }

    fn state_with_pam_unused_reason() -> TurnState {
        let mut state = TurnState::new();
        state.record_pam_unused_reason("pam_disabled");
        assert_eq!(state.pam_unused_reason(), Some("pam_disabled"));
        state
    }
}
