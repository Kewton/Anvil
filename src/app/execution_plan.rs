//! Execution plan management for Plan → Execute mode (Issue #249).
//!
//! Provides helper methods on [`App`] for managing the execution plan lifecycle:
//! detecting `ANVIL_PLAN` / `ANVIL_PLAN_UPDATE` blocks, updating plan item
//! status based on tool execution results, and injecting turn guidance.

use crate::agent::{extract_plan_block, extract_plan_update_block, parse_plan_items};
use crate::contracts::{ExecutionPlan, FinalGateDecision, PlanItem};
use crate::session::{MessageRole, SessionMessage};

use super::App;

// ---------------------------------------------------------------------------
// Checked-line detection helpers (Issue #301)
// ---------------------------------------------------------------------------

/// Detect `[x]`/`[X]` lines in a plan block, returning their 0-based indices.
///
/// The index corresponds to the position among all parseable plan item lines
/// (i.e. lines starting with `- [`, `- ` checkbox format).
pub(crate) fn detect_checked_lines(block: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut item_idx = 0usize;
    for line in block.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ") {
            indices.push(item_idx);
            item_idx += 1;
        } else if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- ") && trimmed.len() > 2 {
            item_idx += 1;
        }
        // Non-item lines are ignored
    }
    indices
}

/// Filter out checked items from a `Vec<PlanItem>`, returning only unchecked ones.
///
/// `checked_indices` are 0-based indices into `items`.
pub(crate) fn filter_unchecked_items(
    items: Vec<PlanItem>,
    checked_indices: &[usize],
) -> Vec<PlanItem> {
    items
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !checked_indices.contains(i))
        .map(|(_, item)| item)
        .collect()
}

/// Message injected when ANVIL_FINAL is suppressed because no plan exists yet.
const PLAN_REQUIRED_MESSAGE: &str = "[System] まず変更計画を作成してください。```ANVIL_PLAN ブロックで変更対象ファイルと作業内容を出力してください。\n\
     例:\n\
     ```ANVIL_PLAN\n\
     - [ ] src/foo.rs: 変更内容の説明\n\
     - [ ] src/bar.rs: 変更内容の説明\n\
     ```";

// incomplete_plan_message() removed in Issue #269 Phase 1.
// All modes now use ExecutionPlan::build_incomplete_plan_message_with_mode()
// to avoid the DRY violation of maintaining two parallel message builders.

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
                self.agent_telemetry.record_anvil_plan_visible();
                return true;
            }
        }
        false
    }

    /// Internal helper: register a plan from pre-parsed items (Issue #303).
    ///
    /// Used by `try_update_plan()` when an `ANVIL_PLAN_UPDATE` arrives but no
    /// plan has been registered yet.  Shares the same registration logic as
    /// `try_register_plan()` but skips block extraction and parsing.
    fn register_plan_from_items(&mut self, items: Vec<PlanItem>) {
        tracing::info!(
            items = items.len(),
            "ANVIL_PLAN_UPDATE on empty plan; registering as new plan (Issue #303)"
        );
        self.execution_plan = ExecutionPlan::new(items);
        self.execution_plan.mark_in_progress(0);
        self.agent_telemetry.record_plan_registration();
        self.agent_telemetry.record_anvil_plan_visible();
    }

    /// Try to detect and apply an `ANVIL_PLAN_UPDATE` block.
    ///
    /// Issue #301: checked-first processing. Before the standard flow:
    ///   1. Detect `[x]` lines in the block.
    ///   2. Mark matching existing plan items as `AlreadySatisfied`.
    ///   3. Retire target files from stagnation state.
    ///   4. Proceed with standard flow for unchecked items only.
    ///
    /// Returns `true` if the plan was updated.
    pub(crate) fn try_update_plan(&mut self, content: &str) -> bool {
        if let Some(block) = extract_plan_update_block(content) {
            let all_items = parse_plan_items(&block);
            if all_items.is_empty() {
                return false;
            }

            // Issue #303: if plan is empty, treat ANVIL_PLAN_UPDATE as new plan registration.
            if self.execution_plan.is_empty() {
                self.register_plan_from_items(all_items);
                return true;
            }

            // --- Issue #301: checked-first retire ---
            // CB-001 fix: process each checked item individually to prevent
            // cross-item target combination from causing false retires.
            let checked_indices = detect_checked_lines(&block);
            let mut had_retire = false;
            if !checked_indices.is_empty() {
                let mut all_retired: Vec<String> = Vec::new();
                for &idx in &checked_indices {
                    if let Some(checked_item) = all_items.get(idx) {
                        if checked_item.target_files.is_empty() {
                            continue;
                        }
                        let retired = self.apply_checked_items(&checked_item.target_files);
                        all_retired.extend(retired);
                    }
                }
                if !all_retired.is_empty() {
                    had_retire = true;
                    // Sync stagnation: retire target files and record completion
                    self.stagnation_state.retire_target_files(&all_retired);
                    self.stagnation_state.record_plan_item_completion();
                }
            }

            // If all items were checked, we're done (no unchecked items to process)
            let new_items = filter_unchecked_items(all_items, &checked_indices);
            if new_items.is_empty() {
                if had_retire {
                    tracing::info!("ANVIL_PLAN_UPDATE detected; all items checked → retired");
                    self.agent_telemetry.record_plan_update();
                }
                return had_retire;
            }

            // --- Standard flow for unchecked items ---
            // Issue #289: supersede stale items BEFORE dedup.
            self.execution_plan.supersede_stale_items(&new_items);
            // Issue #287: deduplicate against existing items before appending
            let deduped = self.execution_plan.deduplicate_new_items(new_items);
            if deduped.is_empty() {
                tracing::info!("ANVIL_PLAN_UPDATE detected; all items deduplicated");
                let had_supersede = self
                    .execution_plan
                    .items
                    .iter()
                    .any(|i| i.status == crate::contracts::PlanItemStatus::Superseded);
                if had_supersede || had_retire {
                    self.agent_telemetry.record_plan_update();
                }
                return had_supersede || had_retire;
            }
            tracing::info!(
                new_items = deduped.len(),
                "ANVIL_PLAN_UPDATE detected; appending items"
            );
            self.execution_plan.append_items(deduped);
            self.agent_telemetry.record_plan_update();
            return true;
        }
        false
    }

    /// Mark existing plan items as `AlreadySatisfied` based on checked target files (Issue #301).
    ///
    /// Returns the list of target file paths that were actually retired.
    fn apply_checked_items(&mut self, checked_targets: &[String]) -> Vec<String> {
        self.execution_plan.mark_unfinished_items_by_target(
            checked_targets,
            crate::contracts::PlanItemStatus::AlreadySatisfied,
        )
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

        let mutation_tools = super::MUTATION_TOOLS;

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
                    .any(|tf| ExecutionPlan::path_matches(tf, &r.summary));
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
    #[allow(dead_code)]
    pub(crate) fn check_plan_final_gate(&mut self) -> bool {
        self.check_plan_final_gate_inner(false)
    }

    /// Like [`check_plan_final_gate`] but also suppresses ANVIL_FINAL when
    /// no plan has been registered yet (Issue #253: Done path guard).
    pub(crate) fn check_plan_final_gate_require_plan(&mut self) -> bool {
        self.check_plan_final_gate_inner(true)
    }

    /// require_plan を外部から指定できる汎用バリアント (Issue #285 A2 fix).
    /// is_mutation_task=true の場合は NoPlan も suppress する。
    pub(crate) fn check_plan_final_gate_with_require(&mut self, require_plan: bool) -> bool {
        self.check_plan_final_gate_inner(require_plan)
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
                // Issue #271: Track ANVIL_FINAL suppression with remaining targets.
                self.agent_telemetry
                    .record_final_suppressed_with_remaining_targets();
                tracing::info!(
                    remaining,
                    total,
                    next = %next_description,
                    pfrr = %self.agent_telemetry.premature_final_request_rate(),
                    "plan-aware final gate: suppressing ANVIL_FINAL (premature)"
                );
                // Issue #269 Phase 1: use build_incomplete_plan_message_with_mode for all
                // modes (replaces the removed incomplete_plan_message() function).
                let mode = self.config.runtime.guidance_mode;
                let guidance_text = self
                    .execution_plan
                    .build_incomplete_plan_message_with_mode(mode);
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
    ///
    /// Returns the length (in characters) of the injected guidance text, or
    /// `None` if no guidance was injected (e.g. no active plan).  Callers can
    /// forward this value to `AgentTelemetry::record_turn_metrics` so that
    /// `guidance_chars_per_turn` reflects the actual guidance size.
    pub(crate) fn inject_plan_turn_guidance(&mut self) -> Option<usize> {
        let mode = self.config.runtime.guidance_mode;
        // Issue #269 Phase 3: use workset-aware guidance when stagnation is detected.
        let workset = self.execution_plan.current_workset();
        let forced = self.forced_mode_active;
        if let Some(guidance) = self
            .execution_plan
            .build_turn_guidance_with_workset(mode, &workset, forced)
        {
            let len = guidance.len();
            let msg = SessionMessage::new(MessageRole::Tool, "system", guidance)
                .with_id(self.next_message_id("tool"));
            self.session.push_message(msg);
            Some(len)
        } else {
            None
        }
    }

    /// Reset the execution plan (e.g. at the start of a new user turn).
    pub(crate) fn reset_execution_plan(&mut self) {
        self.execution_plan = ExecutionPlan::default();
    }
}

// ---------------------------------------------------------------------------
// Tests (Issue #301)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_checked_lines tests ---

    #[test]
    fn detect_checked_lines_mixed() {
        let block = "- [x] src/done.rs: already done\n- [ ] src/todo.rs: still todo\n- [X] src/also_done.rs: also done";
        let indices = detect_checked_lines(block);
        assert_eq!(indices, vec![0, 2]);
    }

    #[test]
    fn detect_checked_lines_none() {
        let block = "- [ ] src/a.rs: task\n- [ ] src/b.rs: task";
        let indices = detect_checked_lines(block);
        assert!(indices.is_empty());
    }

    #[test]
    fn detect_checked_lines_all_checked() {
        let block = "- [x] src/a.rs: done\n- [x] src/b.rs: done";
        let indices = detect_checked_lines(block);
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn detect_checked_lines_ignores_non_item_lines() {
        let block = "Plan update:\n\n- [x] src/a.rs: done\nSome text\n- [ ] src/b.rs: todo";
        let indices = detect_checked_lines(block);
        assert_eq!(indices, vec![0]);
    }

    // --- filter_unchecked_items tests ---

    #[test]
    fn filter_unchecked_items_removes_checked() {
        let items = vec![
            PlanItem::new("done".into(), vec!["src/a.rs".into()]),
            PlanItem::new("todo".into(), vec!["src/b.rs".into()]),
            PlanItem::new("also done".into(), vec!["src/c.rs".into()]),
        ];
        let result = filter_unchecked_items(items, &[0, 2]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].description, "todo");
    }

    #[test]
    fn filter_unchecked_items_empty_checked() {
        let items = vec![
            PlanItem::new("a".into(), vec![]),
            PlanItem::new("b".into(), vec![]),
        ];
        let result = filter_unchecked_items(items, &[]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_unchecked_items_all_checked() {
        let items = vec![
            PlanItem::new("a".into(), vec![]),
            PlanItem::new("b".into(), vec![]),
        ];
        let result = filter_unchecked_items(items, &[0, 1]);
        assert!(result.is_empty());
    }
}
