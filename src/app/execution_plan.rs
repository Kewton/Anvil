//! Execution plan management for Plan → Execute mode (Issue #249).
//!
//! Provides helper methods on [`App`] for managing the execution plan lifecycle:
//! detecting `ANVIL_PLAN` / `ANVIL_PLAN_UPDATE` blocks, updating plan item
//! status based on tool execution results, and injecting turn guidance.

use crate::agent::{extract_plan_block, extract_plan_update_block, parse_plan_items};
use crate::contracts::{ExecutionPlan, FinalGateDecision};
use crate::session::{MessageRole, SessionMessage};

use super::App;

/// Message injected when ANVIL_FINAL is suppressed because no plan exists yet.
const PLAN_REQUIRED_MESSAGE: &str = "[System] まず変更計画を作成してください。```ANVIL_PLAN ブロックで変更対象ファイルと作業内容を出力してください。\n\
     例:\n\
     ```ANVIL_PLAN\n\
     - [ ] src/foo.rs: 変更内容の説明\n\
     - [ ] src/bar.rs: 変更内容の説明\n\
     ```";

/// Message injected when ANVIL_FINAL is suppressed because items remain.
fn incomplete_plan_message(next_desc: &str, remaining: usize, total: usize) -> String {
    format!(
        "[System] まだ {remaining}/{total} 項目が未完了です。次の項目を実行してください:\n  {next_desc}\n\
         全項目完了後に ANVIL_FINAL を出力してください。"
    )
}

impl App {
    /// Try to detect and register an `ANVIL_PLAN` block from the LLM response.
    ///
    /// Returns `true` if a new plan was registered.
    pub(crate) fn try_register_plan(&mut self, content: &str) -> bool {
        // Guard: ignore re-registration when a plan is already active.
        if !self.execution_plan.is_empty() {
            return false;
        }
        if let Some(block) = extract_plan_block(content) {
            let items = parse_plan_items(&block);
            if !items.is_empty() {
                tracing::info!(
                    items = items.len(),
                    "ANVIL_PLAN detected; registering execution plan"
                );
                self.execution_plan = ExecutionPlan::new(items);
                // Mark first item as InProgress
                self.execution_plan.mark_in_progress(0);
                self.agent_telemetry.record_plan_registration();
                return true;
            }
        }
        false
    }

    /// Try to detect and apply an `ANVIL_PLAN_UPDATE` block.
    ///
    /// Returns `true` if the plan was updated.
    pub(crate) fn try_update_plan(&mut self, content: &str) -> bool {
        if let Some(block) = extract_plan_update_block(content) {
            let new_items = parse_plan_items(&block);
            if !new_items.is_empty() {
                tracing::info!(
                    new_items = new_items.len(),
                    "ANVIL_PLAN_UPDATE detected; appending items"
                );
                self.execution_plan.append_items(new_items);
                self.agent_telemetry.record_plan_update();
                return true;
            }
        }
        false
    }

    /// Update plan item status based on tool execution results.
    ///
    /// Issue #255: Uses `record_mutation_success` to require ALL target_files
    /// to be mutated before marking an item as Done. Each successful mutation
    /// is tracked individually per file path.
    pub(crate) fn update_plan_from_results(
        &mut self,
        results: &[crate::tooling::ToolExecutionResult],
    ) {
        if self.execution_plan.is_empty() {
            return;
        }

        let idx = match self.execution_plan.next_actionable_index() {
            Some(i) => i,
            None => return,
        };

        let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];

        let was_done_before = self.execution_plan.items[idx].is_finished();

        // Record each successful mutation individually (Issue #255)
        for r in results {
            if mutation_tools.contains(&r.tool_name.as_str())
                && r.status == crate::tooling::ToolExecutionStatus::Completed
            {
                // summary contains the file path for mutation tools
                if !r.summary.is_empty() {
                    self.execution_plan.record_mutation_success(idx, &r.summary);
                }
            }
        }

        // Check if item just transitioned to Done
        if !was_done_before && self.execution_plan.items[idx].is_finished() {
            tracing::info!(
                item = idx + 1,
                description = %self.execution_plan.items[idx].description,
                "plan item completed (all target_files mutated)"
            );
            // Auto-advance next item to InProgress
            if let Some(next) = self.execution_plan.next_actionable_index() {
                self.execution_plan.mark_in_progress(next);
            }
        }

        // Record failures
        let has_failed_mutation = results.iter().any(|r| {
            mutation_tools.contains(&r.tool_name.as_str())
                && r.status == crate::tooling::ToolExecutionStatus::Failed
        });
        if has_failed_mutation && !self.execution_plan.items[idx].is_finished() {
            self.execution_plan.record_failure(idx);
            tracing::warn!(
                item = idx + 1,
                retry_count = self.execution_plan.items[idx].retry_count,
                "plan item mutation failed"
            );
        }
    }

    /// Check the plan-aware ANVIL_FINAL gate.
    ///
    /// Returns `true` if ANVIL_FINAL should be suppressed (plan incomplete).
    /// When suppressed, injects a guidance message into the session.
    /// Check the plan-aware ANVIL_FINAL gate.
    ///
    /// Returns `true` if ANVIL_FINAL should be suppressed (plan incomplete).
    /// When suppressed, injects a guidance message into the session.
    ///
    /// When `require_plan` is true, the NoPlan branch also suppresses
    /// ANVIL_FINAL and requests plan creation (Issue #253).
    pub(crate) fn check_plan_final_gate(&mut self) -> bool {
        self.check_plan_final_gate_inner(false)
    }

    /// Like [`check_plan_final_gate`] but also suppresses ANVIL_FINAL when
    /// no plan has been registered yet (Issue #253: Done path guard).
    pub(crate) fn check_plan_final_gate_require_plan(&mut self) -> bool {
        self.check_plan_final_gate_inner(true)
    }

    fn check_plan_final_gate_inner(&mut self, require_plan: bool) -> bool {
        // Issue #255: Track every ANVIL_FINAL request.
        self.agent_telemetry.record_final_request();

        // Issue #251: Sync plan completion from touched_files before gate check.
        if !self.execution_plan.is_empty() {
            let before = self.execution_plan.finished_count();
            self.execution_plan
                .sync_from_touched_files(&self.session.working_memory.touched_files);
            let after = self.execution_plan.finished_count();
            if after > before {
                self.agent_telemetry.record_sync_from_touched_files();
            }
        }

        match self.execution_plan.check_final_gate() {
            FinalGateDecision::Allow => {
                tracing::info!("plan-aware final gate: all items finished, allowing ANVIL_FINAL");
                false
            }
            FinalGateDecision::NoPlan => {
                if !require_plan {
                    return false; // No plan → fall through to existing guard
                }
                // Issue #255: NoPlan suppression counts as premature
                self.agent_telemetry.record_premature_final();
                tracing::info!("plan-aware final gate: no plan, requesting plan creation");
                let msg = SessionMessage::new(
                    MessageRole::Tool,
                    "system",
                    PLAN_REQUIRED_MESSAGE.to_string(),
                )
                .with_id(self.next_message_id("tool"));
                self.session.push_message(msg);
                true
            }
            FinalGateDecision::Incomplete {
                next_description,
                remaining,
                total,
            } => {
                // Issue #255: Track premature final request (PFRR).
                self.agent_telemetry.record_premature_final();
                tracing::info!(
                    remaining,
                    total,
                    next = %next_description,
                    pfrr = %self.agent_telemetry.premature_final_request_rate(),
                    "plan-aware final gate: suppressing ANVIL_FINAL (premature)"
                );
                let msg = SessionMessage::new(
                    MessageRole::Tool,
                    "system",
                    incomplete_plan_message(&next_description, remaining, total),
                )
                .with_id(self.next_message_id("tool"));
                self.session.push_message(msg);
                true
            }
        }
    }

    /// Inject turn guidance for the current plan item.
    ///
    /// Called at the beginning of each follow-up turn to guide the LLM.
    pub(crate) fn inject_plan_turn_guidance(&mut self) {
        if let Some(guidance) = self.execution_plan.build_turn_guidance() {
            let msg = SessionMessage::new(MessageRole::Tool, "system", guidance)
                .with_id(self.next_message_id("tool"));
            self.session.push_message(msg);
        }
    }

    /// Reset the execution plan (e.g. at the start of a new user turn).
    pub(crate) fn reset_execution_plan(&mut self) {
        self.execution_plan = ExecutionPlan::default();
    }
}
