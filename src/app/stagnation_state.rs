//! Per-turn stagnation telemetry and policy functions (Issue #263).
//!
//! `StagnationState` is an observation-only struct held by `App`.
//! Policy decisions (scoring, plan repair, workset steering) are
//! implemented as module-level pure functions to maintain SRP.

use std::collections::VecDeque;

use crate::contracts::{ExecutionPlan, PlanItem};

/// Maximum number of recent read-only turn flags to keep.
const RECENT_TURNS_CAP: usize = 5;

/// Maximum workset size returned by `compute_next_workset()`.
pub(crate) const MAX_WORKSET_SIZE: usize = 5;

/// Maximum number of starved file paths to display in forced messages.
const MAX_DISPLAY_PATHS: usize = 5;

/// Per-turn stagnation telemetry (observation only).
///
/// `App` holds this struct and updates it from the agentic loop each turn.
/// Follows the existing tracker pattern (ReadRepeatTracker, etc.).
pub struct StagnationState {
    /// Turns since last mutation (file.edit/file.write success).
    pub turns_since_last_mutation: usize,
    /// Turns since mutation on a new (previously untouched) target file.
    pub turns_since_new_target_file: usize,
    /// Consecutive turns with the same workset.
    pub same_workset_turns: usize,
    /// Turns since a plan item was completed.
    pub turns_since_plan_item_completion: usize,
    /// Ring buffer of read-only flags for the most recent turns.
    pub recent_read_only_turns: VecDeque<bool>,
    /// Target files that have not yet been mutated.
    pub starved_target_files: Vec<String>,
    /// Previous workset indices (for same-workset detection).
    previous_workset: Vec<usize>,
    /// Flag: whether end_turn was called after the last begin_turn.
    turn_ended: bool,
    /// Flag: whether record_mutation hit a new target this turn.
    had_new_target_this_turn: bool,
    /// Flag: whether record_plan_item_completion was called this turn.
    had_plan_item_completion_this_turn: bool,
}

impl Default for StagnationState {
    fn default() -> Self {
        Self::new()
    }
}

impl StagnationState {
    pub fn new() -> Self {
        Self {
            turns_since_last_mutation: 0,
            turns_since_new_target_file: 0,
            same_workset_turns: 0,
            turns_since_plan_item_completion: 0,
            recent_read_only_turns: VecDeque::with_capacity(RECENT_TURNS_CAP),
            starved_target_files: Vec::new(),
            previous_workset: Vec::new(),
            turn_ended: true,
            had_new_target_this_turn: false,
            had_plan_item_completion_this_turn: false,
        }
    }

    /// Initialize from a plan's target files.
    pub fn init_from_plan(target_files: &[String]) -> Self {
        let mut state = Self::new();
        state.starved_target_files = target_files.to_vec();
        state
    }

    /// Called at the start of each turn. Updates workset staleness.
    ///
    /// If the previous turn's `end_turn()` was not called (crash recovery),
    /// we treat it as a read-only turn with no mutation (DR1-011).
    pub fn begin_turn(&mut self, current_workset: &[usize]) {
        // Fallback: if end_turn wasn't called, treat previous turn as no-mutation
        if !self.turn_ended {
            self.end_turn(false);
        }
        self.turn_ended = false;
        self.had_new_target_this_turn = false;
        self.had_plan_item_completion_this_turn = false;

        // Workset staleness check
        if current_workset == self.previous_workset.as_slice() {
            self.same_workset_turns += 1;
        } else {
            self.same_workset_turns = 1;
            self.previous_workset = current_workset.to_vec();
        }
    }

    /// Record a successful mutation on a file path.
    pub fn record_mutation(&mut self, file_path: &str) {
        // Check if this is a previously-starved target file
        let was_starved = self
            .starved_target_files
            .iter()
            .any(|f| ExecutionPlan::path_matches(f, file_path));
        if was_starved {
            self.starved_target_files
                .retain(|f| !ExecutionPlan::path_matches(f, file_path));
            self.had_new_target_this_turn = true;
        }
    }

    /// Record a plan item completion.
    pub fn record_plan_item_completion(&mut self) {
        self.had_plan_item_completion_this_turn = true;
    }

    /// Called at the end of each turn.
    pub fn end_turn(&mut self, had_mutation: bool) {
        self.turn_ended = true;

        // turns_since_last_mutation
        if had_mutation {
            self.turns_since_last_mutation = 0;
        } else {
            self.turns_since_last_mutation += 1;
        }

        // turns_since_new_target_file
        if self.had_new_target_this_turn {
            self.turns_since_new_target_file = 0;
        } else {
            self.turns_since_new_target_file += 1;
        }

        // turns_since_plan_item_completion
        if self.had_plan_item_completion_this_turn {
            self.turns_since_plan_item_completion = 0;
        } else {
            self.turns_since_plan_item_completion += 1;
        }

        // Track read-only turn in ring buffer
        if self.recent_read_only_turns.len() >= RECENT_TURNS_CAP {
            self.recent_read_only_turns.pop_front();
        }
        self.recent_read_only_turns.push_back(!had_mutation);
    }
}

// ---------------------------------------------------------------------------
// Policy pure functions
// ---------------------------------------------------------------------------

/// Compute stagnation score (0-4).
///
/// Scoring rules:
///   +1: `turns_since_last_mutation >= 5` (mutation drought)
///   +1: `same_workset_turns >= 3` (workset staleness)
///   +1: `turns_since_plan_item_completion >= 8` (no plan progress)
///   +1: 4+ of last 5 turns were read-only (read domination)
pub fn compute_stagnation_score(state: &StagnationState) -> usize {
    let mut score = 0;
    if state.turns_since_last_mutation >= 5 {
        score += 1;
    }
    if state.same_workset_turns >= 3 {
        score += 1;
    }
    if state.turns_since_plan_item_completion >= 8 {
        score += 1;
    }
    // Read domination: 4+ of last 5 turns read-only
    let read_only_count = state.recent_read_only_turns.iter().filter(|&&v| v).count();
    if state.recent_read_only_turns.len() >= 5 && read_only_count >= 4 {
        score += 1;
    }
    score
}

/// ANVIL_PLAN_UPDATE request conditions.
///
/// Returns `true` when all four conditions are met:
/// - stagnation score >= 2
/// - starved target files >= 2
/// - plan_repair_request_count < 2 (limit)
/// - remaining_turns >= 5
pub fn should_request_plan_repair(
    state: &StagnationState,
    plan_repair_request_count: usize,
    remaining_turns: usize,
) -> bool {
    compute_stagnation_score(state) >= 2
        && state.starved_target_files.len() >= 2
        && plan_repair_request_count < 2
        && remaining_turns >= 5
}

// ---------------------------------------------------------------------------
// Workset steering (Phase 1)
// ---------------------------------------------------------------------------

/// Score-based workset selection.
///
/// Assigns scores to actionable plan items and returns the top
/// `MAX_WORKSET_SIZE` indices sorted by score (descending).
///
/// Scoring:
///   - untouched target file in item: +100
///   - stagnant item (same workset >= 2): +50
///   - high stagnation score: +30 for mutation-likely items
pub fn compute_next_workset(
    plan: &ExecutionPlan,
    stagnation: &StagnationState,
    stagnation_score: usize,
) -> Vec<usize> {
    if plan.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, i64)> = Vec::new();

    for (idx, item) in plan.items.iter().enumerate() {
        if item.is_finished() {
            continue;
        }

        let mut item_score: i64 = 0;

        // +100 for untouched target files
        let has_untouched = item.target_files.iter().any(|tf| {
            !item
                .mutated_files
                .iter()
                .any(|mf| ExecutionPlan::path_matches(mf, tf))
        });
        if has_untouched {
            item_score += 100;
        }

        // +50 for items with starved target files (specific to this item)
        let has_starved = item.target_files.iter().any(|tf| {
            stagnation
                .starved_target_files
                .iter()
                .any(|sf| ExecutionPlan::path_matches(sf, tf))
        });
        if has_starved {
            item_score += 50;
        }

        // -40 for items in the previous workset (encourage rotation)
        if stagnation.same_workset_turns >= 2 && stagnation.previous_workset.contains(&idx) {
            item_score -= 40;
        }

        // +30 for mutation-likely items when stagnation score is high
        if stagnation_score >= 2 && !item.target_files.is_empty() {
            item_score += 30;
        }

        scored.push((idx, item_score));
    }

    // Sort by score descending, then by index ascending (stable)
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    scored
        .into_iter()
        .take(MAX_WORKSET_SIZE)
        .map(|(idx, _)| idx)
        .collect()
}

// ---------------------------------------------------------------------------
// Sanitization (Phase 2)
// ---------------------------------------------------------------------------

/// Sanitize a string for safe injection into a prompt.
///
/// Removes control characters, ANVIL_* markers, and triple backticks.
pub fn sanitize_for_prompt_entry(input: &str) -> String {
    let mut result = input.to_string();
    // Remove all control characters including newline and tab
    result.retain(|c| !c.is_control());
    // Remove ANVIL_* markers
    for marker in &[
        "ANVIL_FINAL",
        "ANVIL_PLAN_UPDATE",
        "ANVIL_PLAN",
        "ANVIL_DONE",
    ] {
        result = result.replace(marker, "");
    }
    // Remove triple backticks
    result = result.replace("```", "");
    result.trim().to_string()
}

/// Build a forced-mode stagnation message.
pub fn build_forced_message(
    score: usize,
    starved_target_files: &[String],
    remaining_turns: usize,
) -> String {
    let display_files: Vec<String> = starved_target_files
        .iter()
        .take(MAX_DISPLAY_PATHS)
        .map(|f| sanitize_for_prompt_entry(f))
        .collect();
    let files_str = if starved_target_files.len() > MAX_DISPLAY_PATHS {
        format!(
            "{} ...and {} more",
            display_files.join(", "),
            starved_target_files.len() - MAX_DISPLAY_PATHS
        )
    } else {
        display_files.join(", ")
    };

    format!(
        "⚠ STAGNATION DETECTED (score={score}/4) — Forced mode active.\n\n\
         You MUST change your approach immediately:\n\
         - UNTOUCHED target files requiring mutation: {files_str}\n\
         - Prioritize file.edit / file.write over search / read / confirm\n\
         - Move to a different target file NOW\n\
         - Remaining turns: {remaining_turns}"
    )
}

// ---------------------------------------------------------------------------
// Deduplication (Phase 3)
// ---------------------------------------------------------------------------

/// Deduplicate new plan items against existing items.
///
/// Excludes new items whose normalized `target_files` exactly match:
/// 1. An existing unfinished item's `target_files`, OR
/// 2. A `Done` item's `target_files`
pub fn deduplicate_plan_items(
    existing_items: &[PlanItem],
    new_items: Vec<PlanItem>,
) -> Vec<PlanItem> {
    new_items
        .into_iter()
        .filter(|new_item| {
            let mut new_targets = new_item.target_files.clone();
            new_targets.sort();

            !existing_items.iter().any(|existing| {
                let mut existing_targets = existing.target_files.clone();
                existing_targets.sort();
                new_targets == existing_targets
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Budget-aware thresholds (Phase 4)
// ---------------------------------------------------------------------------

/// Effective thresholds derived from config baselines and runtime state.
#[derive(Debug, Clone, Copy)]
pub struct EffectiveThresholds {
    pub phase_force_transition: usize,
    pub read_transition: usize,
}

/// Compute effective thresholds based on budget pressure.
///
/// `budget_factor = min(1.0, remaining_turns / (untouched_targets * 5 + 5))`
///
/// Factor is clamped to [0.3, 1.0]. Effective threshold = max(3, round(baseline * factor)).
pub fn compute_effective_thresholds(
    baseline_phase_force: usize,
    baseline_read_transition: usize,
    remaining_turns: usize,
    untouched_target_count: usize,
    _recent_mutation_rate: f64,
) -> EffectiveThresholds {
    let denominator = (untouched_target_count * 5 + 5) as f64;
    let raw_factor = remaining_turns as f64 / denominator;
    let factor = raw_factor.clamp(0.3, 1.0);

    let phase = ((baseline_phase_force as f64) * factor).round() as usize;
    let read = ((baseline_read_transition as f64) * factor).round() as usize;

    EffectiveThresholds {
        phase_force_transition: phase.max(3),
        read_transition: read.max(3),
    }
}

/// Build a plan repair request message.
pub fn build_plan_repair_message(starved_target_files: &[String]) -> String {
    let display_files: Vec<String> = starved_target_files
        .iter()
        .take(MAX_DISPLAY_PATHS)
        .map(|f| sanitize_for_prompt_entry(f))
        .collect();
    let files_str = display_files.join(", ");

    format!(
        "Your current plan has stalled. Please issue an ANVIL_PLAN_UPDATE to reorganize \
         remaining work around these untouched target files: {files_str}\n\n\
         Rules:\n\
         - Focus on untouched/incomplete target files only\n\
         - Do NOT add \"confirm existence\" or \"verify\" items\n\
         - Do NOT re-add completed items\n\
         - Add only concrete mutation actions (file.edit / file.write)"
    )
}
