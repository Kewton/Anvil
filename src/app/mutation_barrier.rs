//! Pre-mutation plan barrier (Issue #303).
//!
//! Blocks mutation tools (file.write, file.edit, file.edit_anchor) when no
//! execution plan has been registered.  The barrier fires at most
//! `MUTATION_BARRIER_MAX` times to avoid infinite loops with models that
//! ignore the guidance.

use crate::tooling::{
    ToolExecutionPayload, ToolExecutionRequest, ToolExecutionResult, ToolExecutionStatus,
};

/// Maximum number of barrier blocks before auto-release.
pub const MUTATION_BARRIER_MAX: u8 = 2;

/// Message returned to the LLM when the barrier blocks a mutation tool.
///
/// Issue #391: reiterate the ANVIL_PLAN single-emission constraint to stop
/// local-model drift where the barrier repeatedly triggers ANVIL_PLAN
/// restatements instead of the expected file.write / file.edit /
/// file.edit_anchor / file.rewrite call.
pub const MUTATION_BARRIER_MESSAGE: &str = "Before modifying files, you must output an ANVIL_PLAN block with your implementation plan. \
     Use the ANVIL_PLAN format (markdown checklist), NOT the agent.plan tool. \
     Output ANVIL_PLAN exactly once; do not repeat it. After ANVIL_PLAN, your next action must be file.write, file.edit, file.edit_anchor, or file.rewrite. \
     Example:\n\
     ANVIL_PLAN\n\
     - [ ] path/to/file.rs: description of change\n\
     END_ANVIL_PLAN";

/// Tracks how many times mutation tools have been blocked.
#[derive(Default)]
pub struct MutationBarrier {
    block_count: u8,
}

/// Result of a barrier check-and-filter operation.
pub struct MutationBarrierResult {
    /// Requests that passed the barrier (non-mutation, or barrier inactive).
    pub passed_requests: Vec<(usize, ToolExecutionRequest)>,
    /// Results for blocked mutation tools.
    pub blocked_results: Vec<(usize, ToolExecutionResult)>,
    /// Number of mutations blocked in this batch.
    pub blocked_count: usize,
}

impl MutationBarrier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check a batch of validated requests and filter out mutation tools
    /// when no execution plan is registered.
    ///
    /// When `execution_plan_empty` is true and `block_count < MUTATION_BARRIER_MAX`,
    /// mutation tools are moved to `blocked_results` with a `[plan_barrier]` prefix
    /// and `ToolExecutionStatus::Blocked` status.  Non-mutation tools always pass.
    pub fn check_and_filter(
        &mut self,
        validated_requests: Vec<(usize, ToolExecutionRequest)>,
        execution_plan_empty: bool,
    ) -> MutationBarrierResult {
        // Fast path: barrier inactive (plan exists or max reached).
        if !execution_plan_empty || self.block_count >= MUTATION_BARRIER_MAX {
            return MutationBarrierResult {
                passed_requests: validated_requests,
                blocked_results: Vec::new(),
                blocked_count: 0,
            };
        }

        // Check if any request is a mutation tool.
        let has_mutation = validated_requests
            .iter()
            .any(|(_, r)| super::MUTATION_TOOLS.contains(&r.spec.name.as_str()));

        if !has_mutation {
            return MutationBarrierResult {
                passed_requests: validated_requests,
                blocked_results: Vec::new(),
                blocked_count: 0,
            };
        }

        // Barrier fires: split mutation vs non-mutation.
        self.block_count += 1;
        let mut passed = Vec::new();
        let mut blocked = Vec::new();

        for (idx, request) in validated_requests {
            if super::MUTATION_TOOLS.contains(&request.spec.name.as_str()) {
                let result = ToolExecutionResult {
                    tool_call_id: request.tool_call_id.clone(),
                    tool_name: request.spec.name.clone(),
                    status: ToolExecutionStatus::Blocked,
                    summary: format!("[plan_barrier] {MUTATION_BARRIER_MESSAGE}"),
                    payload: ToolExecutionPayload::None,
                    artifacts: Vec::new(),
                    elapsed_ms: 0,
                    diff_summary: None,
                    edit_detail: None,
                    rolled_back: false,
                };
                blocked.push((idx, result));
            } else {
                passed.push((idx, request));
            }
        }

        let blocked_count = blocked.len();
        MutationBarrierResult {
            passed_requests: passed,
            blocked_results: blocked,
            blocked_count,
        }
    }
}
