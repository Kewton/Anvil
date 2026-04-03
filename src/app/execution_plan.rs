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
/// Backward-compatible wrapper for sequential mode.
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
    /// Filters out rolled-back and no-op mutations. Each valid mutation is
    /// matched against all unfinished items' target_files (multi-item attribution).
    ///
    /// Returns `(mutations, items_advanced)` for per-turn telemetry.
    pub(crate) fn update_plan_from_results(
        &mut self,
        results: &[crate::tooling::ToolExecutionResult],
    ) -> (u32, u32) {
        use crate::contracts::PlanItemStatus;

        if self.execution_plan.is_empty() {
            return (0, 0);
        }

        let mut mutations_count: u32 = 0;
        let current_idx = self.execution_plan.next_actionable_index();

        let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];

        // Snapshot finished state before processing
        let was_finished: Vec<bool> = self
            .execution_plan
            .items
            .iter()
            .map(|i| i.is_finished())
            .collect();

        for r in results {
            if !mutation_tools.contains(&r.tool_name.as_str())
                || r.status != crate::tooling::ToolExecutionStatus::Completed
            {
                continue;
            }

            // Skip rolled_back mutations
            if r.rolled_back {
                self.agent_telemetry.record_rolled_back_mutation();
                continue;
            }
            // Skip no-op mutations
            if r.summary.contains("(no changes)") {
                self.agent_telemetry.record_no_op_mutation();
                continue;
            }

            if r.summary.is_empty() {
                continue;
            }

            mutations_count += 1;

            // Find matching unfinished items by target_files
            let mut matches: Vec<usize> = Vec::new();
            for (i, item) in self.execution_plan.items.iter().enumerate() {
                if item.is_finished() || item.target_files.is_empty() {
                    continue;
                }
                let file_matches = item
                    .target_files
                    .iter()
                    .any(|tf| r.summary.ends_with(tf) || tf.ends_with(&r.summary));
                if file_matches {
                    matches.push(i);
                }
            }

            if !matches.is_empty() {
                // Prioritize InProgress items over Pending
                let inprogress: Vec<usize> = matches
                    .iter()
                    .copied()
                    .filter(|&i| self.execution_plan.items[i].status == PlanItemStatus::InProgress)
                    .collect();
                let targets = if inprogress.is_empty() {
                    matches
                } else {
                    inprogress
                };
                for idx in targets {
                    self.execution_plan.record_mutation_success(idx, &r.summary);
                }
            } else {
                // Fallback: attribute to current item only (empty target_files or no match)
                if let Some(idx) = current_idx {
                    self.execution_plan.record_mutation_success(idx, &r.summary);
                }
            }
        }

        // Check which items just transitioned to Done and log; count advances
        let mut items_advanced: u32 = 0;
        for (i, &was) in was_finished.iter().enumerate() {
            if !was && self.execution_plan.items[i].is_finished() {
                items_advanced += 1;
                tracing::info!(
                    item = i + 1,
                    description = %self.execution_plan.items[i].description,
                    "plan item completed (all target_files mutated)"
                );
            }
        }

        // Auto-advance next pending item to InProgress
        if let Some(next) = self.execution_plan.next_actionable_index()
            && self.execution_plan.items[next].status == PlanItemStatus::Pending
        {
            self.execution_plan.mark_in_progress(next);
        }

        // Record failures (attributed to current item)
        if let Some(idx) = current_idx {
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

        (mutations_count, items_advanced)
    }

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
                let mode = self.config.runtime.guidance_mode;
                let guidance_text = match mode {
                    crate::config::GuidanceMode::Batch => self
                        .execution_plan
                        .build_incomplete_plan_message_with_mode(mode),
                    _ => incomplete_plan_message(&next_description, remaining, total),
                };
                let msg = SessionMessage::new(MessageRole::Tool, "system", guidance_text)
                    .with_id(self.next_message_id("tool"));
                self.session.push_message(msg);
                true
            }
        }
    }

    /// Inject turn guidance for the current plan item.
    ///
    /// Called at the beginning of each follow-up turn to guide the LLM.
    /// Uses the configured `guidance_mode` from runtime config.
    pub(crate) fn inject_plan_turn_guidance(&mut self) {
        let mode = self.config.runtime.guidance_mode;
        if let Some(guidance) = self.execution_plan.build_turn_guidance_with_mode(mode) {
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
