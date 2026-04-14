//! Post-escalation fix_slice routing barrier (Issue #355, Bug 1).
//!
//! When a `requires_worker_observation` pack has flagged fix_slice
//! escalation but the LLM still emits parent-side mutation tools
//! (`file.edit`, `file.write`, `file.edit_anchor`) without actually
//! invoking `agent.fix_slice`, this barrier blocks those mutations and
//! forces the LLM onto the worker path. Bounded by
//! [`MAX_ESCALATION_BARRIER_BLOCKS`] so a model that refuses to comply
//! still releases after a small number of attempts and lets the existing
//! `finalize_fixslice_outcome` classification fire.

use crate::tooling::{
    ToolExecutionPayload, ToolExecutionRequest, ToolExecutionResult, ToolExecutionStatus,
};

/// Maximum times the barrier will fire per session before auto-releasing.
pub const MAX_ESCALATION_BARRIER_BLOCKS: u8 = 3;

/// Message returned to the LLM when a parent-side mutation tool is
/// blocked by the barrier.
pub const ESCALATION_BARRIER_MESSAGE: &str = "fix_slice escalation is active for this worker-required pack. \
     You MUST call agent.fix_slice on the most relevant target file \
     instead of file.edit / file.write / file.edit_anchor. Parent-side \
     mutation tools are temporarily suspended until agent.fix_slice has \
     been invoked. Emit a single agent.fix_slice call with target_path \
     and goal describing the intended change.";

const BLOCKED_TOOLS: &[&str] = &["file.write", "file.edit", "file.edit_anchor"];

/// Per-session state: tracks how many times the barrier has fired.
#[derive(Default)]
pub struct EscalationBarrier {
    block_count: u8,
}

/// Result of a barrier check-and-filter.
pub struct EscalationBarrierResult {
    /// Requests that passed the barrier (non-mutation, or barrier inactive).
    pub passed_requests: Vec<(usize, ToolExecutionRequest)>,
    /// Results for blocked mutation tools.
    pub blocked_results: Vec<(usize, ToolExecutionResult)>,
    /// Number of mutations blocked in this batch.
    pub blocked_count: usize,
}

impl EscalationBarrier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check a batch of validated requests. When `armed` is true and the
    /// per-session cap has not been reached, parent-side mutation tools are
    /// moved into `blocked_results`. Non-mutation tools always pass.
    pub fn check_and_filter(
        &mut self,
        validated_requests: Vec<(usize, ToolExecutionRequest)>,
        armed: bool,
    ) -> EscalationBarrierResult {
        if !armed || self.block_count >= MAX_ESCALATION_BARRIER_BLOCKS {
            return EscalationBarrierResult {
                passed_requests: validated_requests,
                blocked_results: Vec::new(),
                blocked_count: 0,
            };
        }

        let has_target = validated_requests
            .iter()
            .any(|(_, r)| BLOCKED_TOOLS.contains(&r.spec.name.as_str()));
        if !has_target {
            return EscalationBarrierResult {
                passed_requests: validated_requests,
                blocked_results: Vec::new(),
                blocked_count: 0,
            };
        }

        self.block_count += 1;
        let mut passed = Vec::new();
        let mut blocked = Vec::new();
        for (idx, request) in validated_requests {
            if BLOCKED_TOOLS.contains(&request.spec.name.as_str()) {
                let result = ToolExecutionResult {
                    tool_call_id: request.tool_call_id.clone(),
                    tool_name: request.spec.name.clone(),
                    status: ToolExecutionStatus::Blocked,
                    summary: format!("[fixslice_escalation_barrier] {ESCALATION_BARRIER_MESSAGE}"),
                    payload: ToolExecutionPayload::None,
                    artifacts: Vec::new(),
                    elapsed_ms: 0,
                    diff_summary: None,
                    edit_detail: None,
                    rolled_back: false,
                    observed_delta: None,
                };
                blocked.push((idx, result));
            } else {
                passed.push((idx, request));
            }
        }

        let blocked_count = blocked.len();
        EscalationBarrierResult {
            passed_requests: passed,
            blocked_results: blocked,
            blocked_count,
        }
    }
}
