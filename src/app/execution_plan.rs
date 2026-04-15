//! Execution plan management for Plan → Execute mode (Issue #249).
//!
//! Provides helper methods on [`App`] for managing the execution plan lifecycle:
//! detecting `ANVIL_PLAN` / `ANVIL_PLAN_UPDATE` blocks, updating plan item
//! status based on tool execution results, and injecting turn guidance.

use crate::agent::{extract_plan_block, extract_plan_update_block, parse_plan_items};
use crate::contracts::{ExecutionPlan, FinalGateDecision, PlanItem};
use crate::session::{MessageRole, SessionMessage};

use super::App;

/// Result of try_register_plan() indicating what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanRegistrationResult {
    /// A new plan was registered (plan was empty).
    Registered,
    /// ANVIL_PLAN detected but plan already active → items extracted for replan.
    Replan,
    /// No ANVIL_PLAN block found in content.
    NoBlock,
}

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
    /// Returns [`PlanRegistrationResult`] indicating what happened:
    /// - `Registered`: a new plan was created (plan was empty).
    /// - `Replan`: follow-up `ANVIL_PLAN` on an active plan was treated as update (Issue #305).
    /// - `NoBlock`: no `ANVIL_PLAN` block found, or replan had no effect.
    ///
    /// Issue #380: `strict_closure_gate` forwards the Issue #327 closure
    /// semantics (reject unchecked expansion) through the replan path.
    pub(crate) fn try_register_plan(
        &mut self,
        content: &str,
        strict_closure_gate: bool,
    ) -> PlanRegistrationResult {
        if let Some(block) = extract_plan_block(content) {
            let items = parse_plan_items(&block);
            if items.is_empty() {
                return PlanRegistrationResult::NoBlock;
            }
            if self.execution_plan.is_empty() {
                // New registration (original behavior)
                tracing::info!(
                    items = items.len(),
                    "ANVIL_PLAN detected; registering execution plan"
                );
                self.execution_plan = ExecutionPlan::new(items);
                // Issue #315: auto-retire no-path / summary-only items
                self.execution_plan.auto_retire_no_path_items();
                // Mark first actionable item as InProgress
                if let Some(first) = self.execution_plan.next_actionable_index() {
                    self.execution_plan.mark_in_progress(first);
                }
                self.agent_telemetry.record_plan_registration();
                self.agent_telemetry.record_anvil_plan_visible();
                return PlanRegistrationResult::Registered;
            }
            // Active plan: follow-up ANVIL_PLAN → replan (Issue #305)
            tracing::info!(
                items = items.len(),
                "follow-up ANVIL_PLAN on active plan; treating as replan (Issue #305)"
            );
            self.agent_telemetry.record_anvil_plan_visible();
            let changed = self.apply_replan_items(&block, items, strict_closure_gate);
            if changed {
                return PlanRegistrationResult::Replan;
            }
            // Replan had no effect (all items deduped/already done) → treat as no-op
            return PlanRegistrationResult::NoBlock;
        }
        PlanRegistrationResult::NoBlock
    }

    /// Shared pipeline for plan update operations (Issue #305 DRY fix).
    ///
    /// Executes: checked-first retire → supersede stale → dedup → append.
    /// Returns `true` if any meaningful change occurred (retire, supersede, or append).
    ///
    /// Issue #380: the strict closure gate (Issue #327 — reject unchecked
    /// expansion, record repair-turn telemetry) is passed in explicitly.
    /// Callers translate the current [`crate::app::AgenticMode`] into this
    /// bool (the old `repair_closure_active` boolean field has been removed).
    fn apply_plan_update_pipeline(
        &mut self,
        block: &str,
        all_items: Vec<PlanItem>,
        strict_closure_gate: bool,
    ) -> bool {
        // --- checked-first retire (Issue #301 + Issue #315) ---
        let checked_indices = detect_checked_lines(block);
        let mut had_retire = false;
        let mut retire_count: u32 = 0;
        if !checked_indices.is_empty() {
            let mut all_retired: Vec<String> = Vec::new();
            let mut no_path_descriptions: Vec<String> = Vec::new();
            for &idx in &checked_indices {
                if let Some(checked_item) = all_items.get(idx) {
                    if !checked_item.target_files.is_empty() {
                        let retired = self.apply_checked_items(&checked_item.target_files);
                        all_retired.extend(retired);
                    } else {
                        // Issue #315: collect no-path checked items for description-based retire
                        no_path_descriptions.push(checked_item.description.clone());
                    }
                }
            }
            // Issue #315: retire existing no-path items by description match
            if !no_path_descriptions.is_empty() {
                let count = self.execution_plan.retire_no_path_items_by_description(
                    &no_path_descriptions,
                    crate::contracts::PlanItemStatus::AlreadySatisfied,
                );
                if count > 0 {
                    had_retire = true;
                    retire_count += count as u32;
                    self.stagnation_state.record_plan_item_completion();
                }
            }
            if !all_retired.is_empty() {
                had_retire = true;
                retire_count += all_retired.len() as u32;
                self.stagnation_state.retire_target_files(&all_retired);
                self.stagnation_state.record_plan_item_completion();
            }
        }
        // Issue #327: record retired item count during repair closure mode.
        if strict_closure_gate && retire_count > 0 {
            self.agent_telemetry
                .record_repair_turn_items_retired(retire_count);
        }

        let mut new_items = filter_unchecked_items(all_items, &checked_indices);
        if new_items.is_empty() {
            if had_retire {
                tracing::info!("plan update pipeline: all items checked → retired");
                self.agent_telemetry.record_plan_update();
            }
            return had_retire;
        }

        // Issue #327: During repair closure mode, reject unchecked items instead
        // of appending them. The repair turn is for closing remaining work, not
        // expanding it.
        if strict_closure_gate {
            let rejected = new_items.len() as u32;
            self.agent_telemetry
                .record_repair_turn_items_rejected(rejected);
            tracing::warn!(
                rejected_items = rejected,
                "plan update pipeline: rejected unchecked items during repair closure mode"
            );
            if had_retire {
                self.agent_telemetry.record_plan_update();
            }
            return had_retire;
        }

        // Issue #336: late-stage closure guard.
        //
        // When `remaining==1 && finished>=1`, the late-stage closure hint is
        // already asking the agent to close or retire the last item.  Before
        // evaluating overlap, reconcile the plan against already-touched
        // files so a last item whose target was already mutated flips to Done
        // naturally — otherwise we'd reject legitimate updates that simply
        // lagged the underlying file state.
        //
        // If closure mode is still active after reconciliation, any incoming
        // unchecked item whose target files overlap the single remaining
        // item's target files is rejected outright.  This stops the
        // supersede→dedup→append churn that kept "refreshing" the last item
        // indefinitely in Phase2 Issue334 follow-up runs.
        let touched_snapshot = self.session.working_memory.touched_files.clone();
        let finished_before_sync = self.execution_plan.finished_count();
        self.execution_plan
            .sync_from_touched_files(&touched_snapshot);
        if self.execution_plan.finished_count() > finished_before_sync {
            self.agent_telemetry.record_sync_from_touched_files();
        }
        if self
            .execution_plan
            .build_late_stage_closure_hint()
            .is_some()
            && let Some(remaining_targets) = self
                .execution_plan
                .next_actionable_index()
                .and_then(|i| self.execution_plan.items.get(i))
                .map(|item| item.target_files.clone())
            && !remaining_targets.is_empty()
        {
            let initial_len = new_items.len();
            new_items.retain(|new_item| {
                !remaining_targets.iter().any(|rt| {
                    new_item
                        .target_files
                        .iter()
                        .any(|nt| crate::contracts::ExecutionPlan::path_matches(rt, nt))
                })
            });
            let rejected = (initial_len - new_items.len()) as u32;
            if rejected > 0 {
                self.agent_telemetry
                    .record_late_stage_closure_items_rejected(rejected);
                tracing::warn!(
                    rejected_items = rejected,
                    "plan update pipeline: rejected unchecked items overlapping single remaining target during late-stage closure mode (Issue #336)"
                );
            }
        }

        if new_items.is_empty() {
            if had_retire {
                self.agent_telemetry.record_plan_update();
            }
            return had_retire;
        }

        // --- supersede → dedup → append ---
        // CB-001 fix: count superseded items before/after to detect new supersedes only
        let superseded_before = self
            .execution_plan
            .items
            .iter()
            .filter(|i| i.status == crate::contracts::PlanItemStatus::Superseded)
            .count();
        self.execution_plan.supersede_stale_items(&new_items);
        let superseded_after = self
            .execution_plan
            .items
            .iter()
            .filter(|i| i.status == crate::contracts::PlanItemStatus::Superseded)
            .count();
        let had_supersede = superseded_after > superseded_before;
        let deduped = self.execution_plan.deduplicate_new_items(new_items);
        if deduped.is_empty() {
            tracing::info!("plan update pipeline: all items deduplicated");
            if had_supersede || had_retire {
                self.agent_telemetry.record_plan_update();
            }
            return had_supersede || had_retire;
        }
        tracing::info!(
            new_items = deduped.len(),
            "plan update pipeline: appending items"
        );
        self.execution_plan.append_items(deduped);
        // Issue #315: auto-retire any newly appended no-path items
        self.execution_plan.auto_retire_no_path_items();
        self.agent_telemetry.record_plan_update();
        true
    }

    /// Apply replan items from a follow-up ANVIL_PLAN on an active plan (Issue #305).
    /// Delegates to shared pipeline. Returns whether any meaningful change occurred.
    ///
    /// Issue #380: forwards the strict closure gate to the pipeline.
    fn apply_replan_items(
        &mut self,
        block: &str,
        all_items: Vec<PlanItem>,
        strict_closure_gate: bool,
    ) -> bool {
        self.apply_plan_update_pipeline(block, all_items, strict_closure_gate)
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
        // Issue #315: auto-retire no-path / summary-only items
        self.execution_plan.auto_retire_no_path_items();
        if let Some(first) = self.execution_plan.next_actionable_index() {
            self.execution_plan.mark_in_progress(first);
        }
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
    /// Issue #305: refactored to use `apply_plan_update_pipeline()` shared helper.
    ///
    /// Returns `true` if the plan was updated.
    ///
    /// Issue #380: `strict_closure_gate` forwards the Issue #327 closure
    /// semantics (reject unchecked expansion) through the plan-update pipeline.
    pub(crate) fn try_update_plan(&mut self, content: &str, strict_closure_gate: bool) -> bool {
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

            return self.apply_plan_update_pipeline(&block, all_items, strict_closure_gate);
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

        // Snapshot finished state before processing
        let was_finished: Vec<bool> = self
            .execution_plan
            .items
            .iter()
            .map(|i| i.is_finished())
            .collect();

        for r in results {
            if r.status != crate::tooling::ToolExecutionStatus::Completed {
                continue;
            }

            // Skip rolled_back mutations
            if r.rolled_back {
                self.agent_telemetry.record_rolled_back_mutation();
                continue;
            }
            let changed_paths = r.observed_changed_paths();
            if changed_paths.is_empty() {
                self.agent_telemetry.record_no_op_mutation();
                continue;
            }

            mutations_count += 1;

            for changed_path in changed_paths {
                // Find matching unfinished items by target_files
                let mut matches: Vec<usize> = Vec::new();
                for (i, item) in self.execution_plan.items.iter().enumerate() {
                    if item.is_finished() || item.target_files.is_empty() {
                        continue;
                    }
                    let file_matches = item
                        .target_files
                        .iter()
                        .any(|tf| ExecutionPlan::path_matches(tf, &changed_path));
                    if file_matches {
                        matches.push(i);
                    }
                }

                if !matches.is_empty() {
                    // Prioritize InProgress items over Pending
                    let inprogress: Vec<usize> = matches
                        .iter()
                        .copied()
                        .filter(|&i| {
                            self.execution_plan.items[i].status == PlanItemStatus::InProgress
                        })
                        .collect();
                    let targets = if inprogress.is_empty() {
                        matches
                    } else {
                        inprogress
                    };
                    for idx in targets {
                        self.execution_plan
                            .record_mutation_success(idx, &changed_path);
                    }
                } else {
                    // Fallback: attribute to current item only (empty target_files or no match)
                    if let Some(idx) = current_idx {
                        self.execution_plan
                            .record_mutation_success(idx, &changed_path);
                    }
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
                r.status == crate::tooling::ToolExecutionStatus::Failed
                    && (!r.summary.contains("(no changes)")
                        || !r.observed_changed_paths().is_empty())
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
            let touched_for_sync =
                self.filter_touched_files_for_plan_sync(&self.session.working_memory.touched_files);
            self.execution_plan
                .sync_from_touched_files(&touched_for_sync);
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
                // Issue #372: (E) plan gate suppression telemetry
                self.agent_telemetry
                    .retry
                    .record_plan_gate_suppression_attempted();
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
                // Issue #372: (E) plan gate suppression telemetry
                self.agent_telemetry
                    .retry
                    .record_plan_gate_suppression_attempted();
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
    /// Issue #309: When exactly 1 item remains, injects a closure-focused hint
    /// that strongly biases toward finishing instead of shell inspection drift.
    ///
    /// Returns the length (in characters) of the injected guidance text, or
    /// `None` if no guidance was injected (e.g. no active plan).  Callers can
    /// forward this value to `AgentTelemetry::record_turn_metrics` so that
    /// `guidance_chars_per_turn` reflects the actual guidance size.
    pub(crate) fn inject_plan_turn_guidance(&mut self) -> Option<usize> {
        // Issue #309: late-stage closure mode takes precedence when remaining==1.
        if let Some(closure_hint) = self.execution_plan.build_late_stage_closure_hint() {
            let len = closure_hint.len();
            tracing::info!("late-stage closure mode: remaining=1, injecting closure hint");
            let msg = SessionMessage::new(MessageRole::Tool, "system", closure_hint)
                .with_id(self.next_message_id("tool"));
            self.session.push_message(msg);
            return Some(len);
        }

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

    // --- Issue #305: PlanRegistrationResult / replan tests ---

    /// Build a minimal App for testing execution_plan methods.
    fn build_test_app() -> App {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let tmp = std::env::temp_dir().join(format!(
            "anvil_test_305_{:?}_{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            std::thread::current().id()
        ));
        let mut config =
            crate::config::EffectiveConfig::default_for_test().expect("config should load");
        config.paths.cwd = tmp.clone();
        config.paths.workspace_dir = tmp.join("workspace");
        config.paths.config_file = tmp.join(".anvil").join("config");
        config.paths.state_dir = tmp.join(".anvil").join("state");
        config.paths.session_dir = tmp.join(".anvil").join("sessions");
        config.paths.session_file = config.paths.session_dir.join("default.json");
        config.paths.logs_dir = tmp.join(".anvil").join("logs");
        config.paths.mcp_config_file = tmp.join(".anvil").join("mcp.json");
        config.paths.hooks_config_file = tmp.join(".anvil").join("hooks.json");
        let provider = crate::provider::ProviderRuntimeContext::bootstrap(&config)
            .expect("provider should bootstrap");
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        App::new(config, provider, shutdown_flag).expect("app should initialize")
    }

    #[test]
    fn replan_basic_followup_anvil_plan_on_active_plan() {
        let mut app = build_test_app();
        // Initial registration
        let content1 =
            "```ANVIL_PLAN\n- [ ] src/a.rs: implement feature\n- [ ] src/b.rs: add tests\n```";
        let result1 = app.try_register_plan(content1, false);
        assert_eq!(result1, PlanRegistrationResult::Registered);
        assert_eq!(app.execution_plan.items.len(), 2);

        // Follow-up ANVIL_PLAN on active plan → should be treated as replan
        let content2 = "```ANVIL_PLAN\n- [ ] src/c.rs: new task\n```";
        let result2 = app.try_register_plan(content2, false);
        assert_eq!(result2, PlanRegistrationResult::Replan);
        // New item should be appended
        assert_eq!(app.execution_plan.items.len(), 3);
    }

    #[test]
    fn replan_telemetry_update_count_increases() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        app.try_register_plan(content1, false);
        assert_eq!(app.agent_telemetry.plan_registration_count, 1);
        assert_eq!(app.agent_telemetry.plan_update_count, 0);
        assert_eq!(app.agent_telemetry.anvil_plan_visible_count, 1);

        // Replan
        let content2 = "```ANVIL_PLAN\n- [ ] src/b.rs: new task\n```";
        app.try_register_plan(content2, false);
        // registration_count should NOT increase
        assert_eq!(app.agent_telemetry.plan_registration_count, 1);
        // update_count SHOULD increase
        assert_eq!(app.agent_telemetry.plan_update_count, 1);
        // anvil_plan_visible_count SHOULD increase
        assert_eq!(app.agent_telemetry.anvil_plan_visible_count, 2);
    }

    #[test]
    fn replan_supersede_stale_items() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: old task\n```";
        app.try_register_plan(content1, false);

        // Replan with same file but different description → supersede
        let content2 = "```ANVIL_PLAN\n- [ ] src/a.rs: new approach\n```";
        let result = app.try_register_plan(content2, false);
        assert_eq!(result, PlanRegistrationResult::Replan);
        // Old item should be superseded, new one appended
        assert!(
            app.execution_plan
                .items
                .iter()
                .any(|i| i.status == crate::contracts::PlanItemStatus::Superseded)
        );
    }

    #[test]
    fn replan_dedup_existing_items() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        app.try_register_plan(content1, false);
        // Simulate a mutation so the item won't be superseded
        app.execution_plan.items[0]
            .mutated_files
            .push("src/a.rs".into());

        // Replan with exact same item → should be deduped (target match)
        let content2 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        let result = app.try_register_plan(content2, false);
        // All items deduped, no supersede → NoBlock
        assert_eq!(result, PlanRegistrationResult::NoBlock);
        // Item count unchanged
        assert_eq!(app.execution_plan.items.len(), 1);
    }

    #[test]
    fn replan_done_items_not_appended() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n- [ ] src/b.rs: task2\n```";
        app.try_register_plan(content1, false);
        // Mark first item as done
        app.execution_plan.mark_done(0);

        // Replan includes only a new item + the done item description
        let content2 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n- [ ] src/c.rs: new task\n```";
        let result = app.try_register_plan(content2, false);
        assert_eq!(result, PlanRegistrationResult::Replan);
        // src/c.rs should be added, src/a.rs should be deduped (already done)
        let new_items: Vec<_> = app
            .execution_plan
            .items
            .iter()
            .filter(|i| i.target_files.iter().any(|f| f == "src/c.rs"))
            .collect();
        assert_eq!(new_items.len(), 1);
    }

    #[test]
    fn replan_checked_first_retire() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n- [ ] src/b.rs: task2\n```";
        app.try_register_plan(content1, false);

        // Replan with [x] checked items → should retire
        let content2 = "```ANVIL_PLAN\n- [x] src/a.rs: task\n- [ ] src/c.rs: new task\n```";
        let result = app.try_register_plan(content2, false);
        assert_eq!(result, PlanRegistrationResult::Replan);
        // src/a.rs item should be AlreadySatisfied
        let a_item = app
            .execution_plan
            .items
            .iter()
            .find(|i| i.target_files.iter().any(|f| f == "src/a.rs"))
            .unwrap();
        assert_eq!(
            a_item.status,
            crate::contracts::PlanItemStatus::AlreadySatisfied
        );
    }

    #[test]
    fn replan_subset_items() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n- [ ] src/b.rs: task2\n- [ ] src/c.rs: task3\n```";
        app.try_register_plan(content1, false);
        assert_eq!(app.execution_plan.items.len(), 3);

        // Replan with only a subset of new items
        let content2 = "```ANVIL_PLAN\n- [ ] src/d.rs: new task\n```";
        let result = app.try_register_plan(content2, false);
        assert_eq!(result, PlanRegistrationResult::Replan);
        assert_eq!(app.execution_plan.items.len(), 4);
    }

    #[test]
    fn replan_no_effect_returns_noblock() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        app.try_register_plan(content1, false);
        // Simulate a mutation so the item won't be superseded by replan
        app.execution_plan.items[0]
            .mutated_files
            .push("src/a.rs".into());

        // Replan with same item (deduped, not superseded) → no effect
        let content2 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        let result = app.try_register_plan(content2, false);
        assert_eq!(result, PlanRegistrationResult::NoBlock);
    }

    #[test]
    fn first_registration_unchanged() {
        let mut app = build_test_app();
        let content = "```ANVIL_PLAN\n- [ ] src/a.rs: implement feature\n```";
        let result = app.try_register_plan(content, false);
        assert_eq!(result, PlanRegistrationResult::Registered);
        assert_eq!(app.execution_plan.items.len(), 1);
        assert_eq!(app.agent_telemetry.plan_registration_count, 1);
        assert_eq!(app.agent_telemetry.anvil_plan_visible_count, 1);
    }

    #[test]
    fn plan_update_unchanged() {
        let mut app = build_test_app();
        // Register initial plan
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: task\n```";
        app.try_register_plan(content1, false);

        // Update via ANVIL_PLAN_UPDATE (not ANVIL_PLAN)
        let content2 = "```ANVIL_PLAN_UPDATE\n- [ ] src/b.rs: new task\n```";
        let updated = app.try_update_plan(content2, false);
        assert!(updated);
        assert_eq!(app.execution_plan.items.len(), 2);
        assert_eq!(app.agent_telemetry.plan_update_count, 1);
    }

    // --- Issue #336: late-stage closure guard tests ---

    /// Regression: when remaining==1 with finished>=1, a follow-up
    /// ANVIL_PLAN_UPDATE that only restates the single remaining target must
    /// be rejected rather than creating an endless supersede→append churn.
    #[test]
    fn late_stage_closure_rejects_same_target_replan() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: item A\n- [ ] src/b.rs: item B\n```";
        app.try_register_plan(content1, false);
        // Simulate item A completed via mutation.
        app.execution_plan.mark_done(0);
        assert_eq!(app.execution_plan.finished_count(), 1);

        // Follow-up restating item B's file with a "refined" description.
        let update = "```ANVIL_PLAN_UPDATE\n- [ ] src/b.rs: refined item B\n```";
        let updated = app.try_update_plan(update, false);

        // No new item appended, existing item B stays actionable.
        assert_eq!(
            app.execution_plan.items.len(),
            2,
            "closure guard must not append a refreshed last item"
        );
        let b_item = &app.execution_plan.items[1];
        assert!(
            !b_item.is_finished(),
            "existing remaining item must not be superseded by closure guard"
        );
        // Pipeline reports no meaningful change (nothing retired, nothing appended).
        assert!(!updated);
        assert_eq!(
            app.agent_telemetry.late_stage_closure_items_rejected, 1,
            "telemetry must count the rejected closure-mode item"
        );
        assert_eq!(app.agent_telemetry.plan_update_count, 0);
    }

    /// Closure guard must allow follow-up items targeting genuinely different
    /// files, so the agent can still pivot to new work when remaining==1.
    #[test]
    fn late_stage_closure_allows_different_target_replan() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: A\n- [ ] src/b.rs: B\n```";
        app.try_register_plan(content1, false);
        app.execution_plan.mark_done(0);

        let update = "```ANVIL_PLAN_UPDATE\n- [ ] src/c.rs: genuinely new task\n```";
        let updated = app.try_update_plan(update, false);

        assert!(updated);
        assert_eq!(app.execution_plan.items.len(), 3);
        assert_eq!(app.agent_telemetry.late_stage_closure_items_rejected, 0);
    }

    /// Closure guard must not fire when remaining > 1: normal replan
    /// supersede/dedup semantics must keep working for mid-plan churn.
    #[test]
    fn late_stage_closure_inactive_when_multiple_remaining() {
        let mut app = build_test_app();
        let content1 =
            "```ANVIL_PLAN\n- [ ] src/a.rs: A\n- [ ] src/b.rs: B\n- [ ] src/c.rs: C\n```";
        app.try_register_plan(content1, false);
        app.execution_plan.mark_done(0);
        // remaining == 2 → closure guard inactive.

        let update = "```ANVIL_PLAN_UPDATE\n- [ ] src/b.rs: refined B\n```";
        let updated = app.try_update_plan(update, false);

        assert!(updated);
        assert_eq!(app.agent_telemetry.late_stage_closure_items_rejected, 0);
    }

    /// The closure path must reconcile the plan against already-touched files
    /// before rejecting: if the remaining target was already mutated, sync
    /// should close the plan instead of indefinitely gating on an update.
    #[test]
    fn late_stage_closure_reconciles_touched_files_first() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: A\n- [ ] src/b.rs: B\n```";
        app.try_register_plan(content1, false);
        app.execution_plan.mark_done(0);
        // External evidence: src/b.rs was already touched.
        app.session
            .working_memory
            .touched_files
            .push("src/b.rs".into());

        let update = "```ANVIL_PLAN_UPDATE\n- [ ] src/b.rs: refined B\n```";
        let _ = app.try_update_plan(update, false);

        assert!(
            app.execution_plan.all_finished(),
            "sync_from_touched_files should have closed the last item before guard"
        );
        // No rejection recorded — the guard condition no longer holds post-sync.
        assert_eq!(app.agent_telemetry.late_stage_closure_items_rejected, 0);
    }

    /// Partial-overlap update: the portion targeting the single remaining item
    /// is rejected, while unrelated new items are still appended.
    #[test]
    fn late_stage_closure_partial_overlap_rejects_only_overlapping() {
        let mut app = build_test_app();
        let content1 = "```ANVIL_PLAN\n- [ ] src/a.rs: A\n- [ ] src/b.rs: B\n```";
        app.try_register_plan(content1, false);
        app.execution_plan.mark_done(0);

        let update =
            "```ANVIL_PLAN_UPDATE\n- [ ] src/b.rs: refined B\n- [ ] src/d.rs: new work\n```";
        let updated = app.try_update_plan(update, false);

        assert!(updated);
        // Original B stays, d.rs appended, refined B rejected.
        assert_eq!(app.execution_plan.items.len(), 3);
        assert_eq!(app.agent_telemetry.late_stage_closure_items_rejected, 1);
        let has_d = app
            .execution_plan
            .items
            .iter()
            .any(|i| i.target_files.iter().any(|f| f == "src/d.rs"));
        assert!(has_d, "non-overlapping item must still be appended");
        let b_item = app
            .execution_plan
            .items
            .iter()
            .find(|i| i.target_files.iter().any(|f| f == "src/b.rs"))
            .unwrap();
        assert!(!b_item.is_finished());
    }
}
