//! Actor-loop-local state.
//!
//! `LoopState` owns retry counters and verifier evidence collected inside one
//! `run_actor_loop` invocation. It is not persisted and is distinct from
//! per-user-turn `TurnState`.

#[derive(Debug)]
pub(super) struct LoopState {
    pub(super) tool_calls_made_this_turn: usize,
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) empty_retries: usize,
    pub(super) no_tool_retries: usize,
    pub(super) repo_change_retries: usize,
    pub(super) verifier_repair_retries: usize,
    pub(super) focused_policy_retries: usize,
    pub(super) contract_completion_retries: usize,
    pub(super) contract_completion_role_retries:
        std::collections::HashMap<super::task_contract::ArtifactRole, usize>,
    pub(super) contract_verification_retries: usize,
    pub(super) contract_verifier_repair_edit_count: Option<usize>,
    pub(super) task_contract_verifier_passed_in_loop: bool,
    pub(super) task_contract_verify_commands_collected: Vec<String>,
    pub(super) python_test_retries: usize,
    pub(super) node_runner_retries: usize,
    pub(super) plan_progress_retries: usize,
}

impl LoopState {
    pub(super) fn new() -> Self {
        Self {
            tool_calls_made_this_turn: 0,
            repo_edit_calls_made_this_turn: 0,
            empty_retries: 0,
            no_tool_retries: 0,
            repo_change_retries: 0,
            verifier_repair_retries: 0,
            focused_policy_retries: 0,
            contract_completion_retries: 0,
            contract_completion_role_retries: std::collections::HashMap::new(),
            contract_verification_retries: 0,
            contract_verifier_repair_edit_count: None,
            task_contract_verifier_passed_in_loop: false,
            task_contract_verify_commands_collected: Vec::new(),
            python_test_retries: 0,
            node_runner_retries: 0,
            plan_progress_retries: 0,
        }
    }

    pub(super) fn reset_after_executed_tool_call(&mut self, repo_edit_calls_before_tool: usize) {
        self.empty_retries = 0;
        self.no_tool_retries = 0;
        self.focused_policy_retries = 0;
        if self.repo_edit_calls_made_this_turn != repo_edit_calls_before_tool {
            self.repo_change_retries = 0;
        }
    }
}

impl Default for LoopState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_after_executed_tool_call_clears_retry_counters_and_only_resets_repo_on_edit() {
        let mut state = LoopState::new();
        state.empty_retries = 2;
        state.no_tool_retries = 2;
        state.focused_policy_retries = 2;
        state.repo_change_retries = 2;
        state.repo_edit_calls_made_this_turn = 1;

        state.reset_after_executed_tool_call(0);

        assert_eq!(state.empty_retries, 0);
        assert_eq!(state.no_tool_retries, 0);
        assert_eq!(state.focused_policy_retries, 0);
        assert_eq!(state.repo_change_retries, 0);

        state.repo_change_retries = 2;
        state.reset_after_executed_tool_call(1);
        assert_eq!(state.repo_change_retries, 2);
    }
}
