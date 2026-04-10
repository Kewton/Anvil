//! Shared contract types used across modules.
//!
//! These types form the schema for snapshots, console rendering, and
//! persistent session state.  They are intentionally plain data with
//! `Serialize`/`Deserialize` support.

pub mod tokens;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Sub-agent payload types (Issue #129)
// ---------------------------------------------------------------------------

/// Sub-agent の実行終了理由。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// ANVIL_FINAL未発火時のフォールバック完了検出 (Issue #159).
    FallbackCompleted,
    Timeout,
    MaxIterations,
    LoopDetected,
    /// Tool call count limit reached (Issue #172).
    MaxToolCalls,
    /// Normal completion. Also serves as fallback for unknown variants via `#[serde(other)]`.
    #[default]
    #[serde(other)]
    Completed,
}

impl std::fmt::Display for TerminationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Timeout => write!(f, "timeout"),
            Self::MaxIterations => write!(f, "max_iterations"),
            Self::LoopDetected => write!(f, "loop_detected"),
            Self::MaxToolCalls => write!(f, "max_tool_calls"),
            Self::FallbackCompleted => write!(f, "fallback_completed"),
        }
    }
}

/// Sub-agent が探索で発見した個別の知見。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// 発見事項の短いタイトル
    pub title: String,
    /// 根拠を含む詳細説明
    pub detail: String,
    /// 関連するファイルパス、シンボル名、行参照など
    pub related_code: Vec<String>,
}

/// Sub-agent の構造化返却 payload。
/// 成功時もエラー時も同一構造で返す。
/// termination_reason / error はシステムが設定するフィールドであり、LLM出力からは設定されない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPayload {
    /// 探索で発見した関連ファイルパス
    pub found_files: Vec<String>,
    /// 主要な発見事項
    pub key_findings: Vec<Finding>,
    /// LLMが生成したフリーテキストのサマリー
    pub raw_summary: String,
    /// 結果の信頼度（0.0-1.0、clamp適用）
    /// 現在は情報提供目的のみ。将来的に親エージェントが低信頼度時の再探索判断に使用可能。
    #[serde(default)]
    pub confidence: Option<f32>,
    /// 実行終了理由（システムが設定、LLM出力には含まない）
    #[serde(default)]
    pub termination_reason: TerminationReason,
    /// エラー時のメッセージ（成功時はNone）
    #[serde(default)]
    pub error: Option<String>,
}

impl SubAgentPayload {
    /// フォールバック用コンストラクタ（JSON パース失敗時や部分結果構築時に使用）
    pub fn fallback(raw_summary: String, reason: TerminationReason) -> Self {
        Self {
            found_files: vec![],
            key_findings: vec![],
            raw_summary,
            confidence: None,
            termination_reason: reason,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// FixSlice proposal (Issue #291)
// ---------------------------------------------------------------------------

/// A structured proposal returned by the FixSlice sub-agent.
///
/// The sub-agent produces this as its ANVIL_FINAL output; the parent validates
/// and applies it via the normal `file.rewrite` execution path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixSliceProposal {
    /// Target file path (relative, must match the parent-specified path).
    pub target_path: String,
    /// First line of the replacement range (1-based, inclusive).
    pub start_line: u32,
    /// Last line of the replacement range (1-based, inclusive).
    pub end_line: u32,
    /// The replacement content for the specified line range.
    pub replacement_content: String,
    /// Human-readable rationale for the change.
    #[serde(default)]
    pub rationale: String,
}

// ---------------------------------------------------------------------------
// Completion taxonomy (Issue #255: AgentPhase integration)
// ---------------------------------------------------------------------------

/// How the agentic session ended, classified by plan state and verification.
///
/// Priority order (highest first):
/// 1. Blocked (has_blocked items -> always Blocked)
/// 2. CompleteVerified
/// 3. CompleteUnverified
/// 4. Exhausted
/// 5. Partial
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// All actionable items completed + verify succeeded.
    CompleteVerified,
    /// All actionable items completed + verify unavailable/denied.
    CompleteUnverified,
    /// Some changes made but items remain unfinished.
    #[default]
    Partial,
    /// Item(s) blocked due to repeated failures.
    Blocked,
    /// Budget / turn limit exhausted with unfinished items.
    Exhausted,
}

impl std::fmt::Display for CompletionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompleteVerified => write!(f, "complete_verified"),
            Self::CompleteUnverified => write!(f, "complete_unverified"),
            Self::Partial => write!(f, "partial"),
            Self::Blocked => write!(f, "blocked"),
            Self::Exhausted => write!(f, "exhausted"),
        }
    }
}

impl CompletionKind {
    /// Classify termination based on plan state, verify outcome, and budget.
    ///
    /// - `verify_pass`: `Some(true)` = verified, `Some(false)` = verify failed,
    ///   `None` = unavailable/denied.
    /// - `budget_exhausted`: whether budget/turn limit was reached.
    pub fn classify(
        plan: &ExecutionPlan,
        verify_pass: Option<bool>,
        budget_exhausted: bool,
    ) -> Self {
        let all_finished = plan.all_finished() && !plan.is_empty();

        // Priority 1: blocked items exist → always Blocked
        if plan.has_blocked_items() {
            return CompletionKind::Blocked;
        }

        // Priority 2-3: all items finished → complete_*
        if all_finished {
            return match verify_pass {
                Some(true) => CompletionKind::CompleteVerified,
                _ => CompletionKind::CompleteUnverified,
            };
        }

        // Priority 4: budget exhausted
        if budget_exhausted {
            return CompletionKind::Exhausted;
        }

        // Priority 5: fallback
        CompletionKind::Partial
    }
}

// ---------------------------------------------------------------------------
// Agent telemetry (Issue #255: Stage 0 observability)
// ---------------------------------------------------------------------------

/// Telemetry counters for the agentic session.
///
/// Tracks key metrics for evaluating agent loop quality:
/// - Premature ANVIL_FINAL requests (PFRR)
/// - Plan registration / update counts
/// - `sync_from_touched_files` rescue invocations
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentTelemetry {
    /// Number of ANVIL_FINAL requests that were suppressed (plan incomplete).
    pub premature_final_count: u32,
    /// Total ANVIL_FINAL requests observed (both accepted and suppressed).
    pub total_final_requests: u32,
    /// Number of times a plan was registered via ANVIL_PLAN.
    pub plan_registration_count: u32,
    /// Number of times the plan was updated.
    ///
    /// Includes both explicit `ANVIL_PLAN_UPDATE` and follow-up `ANVIL_PLAN`
    /// on an active plan (replan, Issue #305).
    pub plan_update_count: u32,
    /// Number of times sync_from_touched_files actually advanced items.
    pub sync_from_touched_files_count: u32,
    /// Final completion classification.
    pub completion_kind: Option<CompletionKind>,

    /// Number of times initial ANVIL_PLAN detection missed (raw_content fallback used).
    #[serde(default)]
    pub initial_plan_miss_count: u32,
    /// Number of no-op mutations filtered (summary contains "(no changes)").
    #[serde(default)]
    pub no_op_mutation_count: u32,
    /// Number of rolled_back mutations filtered.
    #[serde(default)]
    pub rolled_back_mutation_count: u32,
    /// Mutations per turn (Phase 0: actual values).
    #[serde(default)]
    pub mutations_per_turn: Vec<u32>,
    /// Items advanced per turn (Phase 0: actual values).
    #[serde(default)]
    pub items_advanced_per_turn: Vec<u32>,
    /// Guidance characters per turn (Phase 0: 0, Phase 1: actual values).
    #[serde(default)]
    pub guidance_chars_per_turn: Vec<u32>,
    /// Workset size per turn (Phase 0: always 1, Phase 1: actual values).
    #[serde(default)]
    pub workset_size_per_turn: Vec<u32>,
    /// Number of forced workset transitions triggered by stagnation control (Issue #263).
    #[serde(default)]
    pub forced_workset_transition_count: u32,
    /// Number of ANVIL_PLAN_UPDATE requests triggered by stagnation control (Issue #263).
    #[serde(default)]
    pub plan_repair_request_count: u32,

    /// Number of times the LLM emitted a visible ANVIL_PLAN block.
    ///
    /// Incremented for both initial registration and follow-up replan (Issue #305).
    /// Note: this count may exceed plan_registration_count when replan occurs.
    #[serde(default)]
    pub anvil_plan_visible_count: u32,

    /// Last turn on which a mutation (file change) occurred.
    #[serde(default)]
    pub last_mutation_turn: u32,

    /// ANVIL_FINAL suppressed with remaining core targets > 0 (count).
    #[serde(default)]
    pub final_suppressed_with_remaining_targets_count: u32,

    /// Number of times pre-mutation barrier blocked mutation tools (Issue #303).
    #[serde(default)]
    pub mutation_barrier_block_count: u32,

    /// First mutation event turn (Issue #273 Phase 1.5).
    /// None if no mutation occurred during the session.
    #[serde(default)]
    pub first_mutation_event_turn: Option<u32>,

    /// Elapsed seconds from session start to first mutation event (Issue #273 Phase 1.5).
    /// None if no mutation occurred during the session.
    #[serde(default)]
    pub first_mutation_event_elapsed_s: Option<f64>,

    /// Tool name that triggered the first mutation event (Issue #273 Phase 1.5).
    /// None if no mutation occurred during the session.
    #[serde(default)]
    pub first_mutation_event_tool: Option<String>,

    /// Number of times fix_slice escalation was triggered (Issue #321).
    #[serde(default)]
    pub fixslice_escalation_count: u32,

    /// Number of pre-exit repair turns injected (Issue #325).
    #[serde(default)]
    pub pre_exit_repair_injected_count: u32,

    /// Number of pre-exit repair turns actually consumed by the LLM (Issue #325).
    #[serde(default)]
    pub pre_exit_repair_consumed_count: u32,
}

impl AgentTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_premature_final(&mut self) {
        self.premature_final_count += 1;
    }

    pub fn record_final_request(&mut self) {
        self.total_final_requests += 1;
    }

    pub fn record_plan_registration(&mut self) {
        self.plan_registration_count += 1;
    }

    pub fn record_plan_update(&mut self) {
        self.plan_update_count += 1;
    }

    pub fn record_sync_from_touched_files(&mut self) {
        self.sync_from_touched_files_count += 1;
    }

    /// Record an initial plan miss (raw_content fallback used).
    pub fn record_initial_plan_miss(&mut self) {
        self.initial_plan_miss_count += 1;
    }

    /// Record a no-op mutation (filtered out).
    pub fn record_no_op_mutation(&mut self) {
        self.no_op_mutation_count += 1;
    }

    /// Record a rolled_back mutation (filtered out).
    pub fn record_rolled_back_mutation(&mut self) {
        self.rolled_back_mutation_count += 1;
    }

    /// Record per-turn metrics for batch experiment telemetry.
    pub fn record_turn_metrics(
        &mut self,
        mutations: u32,
        items_advanced: u32,
        guidance_chars: u32,
        workset_size: u32,
    ) {
        self.mutations_per_turn.push(mutations);
        self.items_advanced_per_turn.push(items_advanced);
        self.guidance_chars_per_turn.push(guidance_chars);
        self.workset_size_per_turn.push(workset_size);
    }

    /// Record a forced workset transition (Issue #263).
    pub fn record_forced_workset_transition(&mut self) {
        self.forced_workset_transition_count += 1;
    }

    /// Record a plan repair request (Issue #263).
    pub fn record_plan_repair_request(&mut self) {
        self.plan_repair_request_count += 1;
    }

    /// Record an ANVIL_PLAN block observed (visible to external telemetry).
    pub fn record_anvil_plan_visible(&mut self) {
        self.anvil_plan_visible_count += 1;
    }

    /// Record a pre-exit repair turn injection (Issue #325).
    pub fn record_pre_exit_repair_injected(&mut self) {
        self.pre_exit_repair_injected_count += 1;
    }

    /// Record a pre-exit repair turn consumed by LLM (Issue #325).
    pub fn record_pre_exit_repair_consumed(&mut self) {
        self.pre_exit_repair_consumed_count += 1;
    }

    /// Record a pre-mutation barrier block (Issue #303).
    pub fn record_mutation_barrier_block(&mut self) {
        self.mutation_barrier_block_count += 1;
    }

    /// Record a mutation turn: updates `last_mutation_turn` and, on the first call only,
    /// populates `first_mutation_event_*`.
    ///
    /// Callers must ensure `tool_name` is an allowlisted mutation tool
    /// (i.e. from `crate::app::MUTATION_TOOLS`) to avoid storing arbitrary strings.
    pub fn record_mutation_turn(&mut self, turn: u32, elapsed_s: Option<f64>, tool_name: &str) {
        self.last_mutation_turn = turn;
        if self.first_mutation_event_turn.is_none() {
            self.first_mutation_event_turn = Some(turn);
            self.first_mutation_event_elapsed_s = elapsed_s;
            self.first_mutation_event_tool = Some(tool_name.to_string());
        }
    }

    /// Record an ANVIL_FINAL suppression with remaining core targets.
    pub fn record_final_suppressed_with_remaining_targets(&mut self) {
        self.final_suppressed_with_remaining_targets_count += 1;
    }

    /// Record a fix_slice escalation event (Issue #321).
    pub fn record_fixslice_escalation(&mut self) {
        self.fixslice_escalation_count += 1;
    }

    /// Threshold: mutations after this turn are considered "late".
    pub const LATE_MUTATION_THRESHOLD: u32 = 20;

    /// Late mutation flag: last_mutation_turn exceeds the threshold.
    pub fn is_late_mutation(&self) -> bool {
        self.last_mutation_turn > Self::LATE_MUTATION_THRESHOLD
    }

    /// Accepted ANVIL_FINAL count: total minus premature (saturating).
    pub fn accepted_final_count(&self) -> u32 {
        self.total_final_requests
            .saturating_sub(self.premature_final_count)
    }

    /// Write telemetry artifact to `$ANVIL_TELEMETRY_DIR/{session_id}_telemetry.json`.
    ///
    /// Returns `Ok(())` when `ANVIL_TELEMETRY_DIR` is unset (no-op) or when the
    /// file was successfully written.  Returns `Err` on validation or I/O failure.
    pub fn write_artifact(&self, session_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let dir_raw = match std::env::var("ANVIL_TELEMETRY_DIR") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(()), // no-op when unset
        };

        self.write_artifact_to_dir(&dir_raw, session_id)
    }

    /// Write telemetry artifact to the given directory path.
    ///
    /// Validates that the path is absolute, not a symlink, and is an existing
    /// directory.  Creates `{session_id}_telemetry.json` using `create_new`
    /// (refuses to overwrite).
    pub fn write_artifact_to_dir(
        &self,
        dir_raw: &str,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir_path = std::path::Path::new(dir_raw);

        // Reject relative paths.
        if !dir_path.is_absolute() {
            return Err("telemetry dir must be an absolute path".into());
        }

        // Reject symlinks on the directory itself.
        let meta = std::fs::symlink_metadata(dir_path)
            .map_err(|_| "telemetry dir does not exist or is inaccessible")?;
        if meta.file_type().is_symlink() {
            return Err("telemetry dir must not be a symlink".into());
        }

        // Canonicalize and verify it is a directory.
        let canonical =
            std::fs::canonicalize(dir_path).map_err(|_| "telemetry dir cannot be canonicalized")?;
        if !canonical.is_dir() {
            return Err("telemetry dir is not a directory".into());
        }

        // Sanitize session_id: reject any path separators or leading dots to prevent
        // directory traversal (e.g. "../evil" or "/abs/path").
        if session_id.contains('/') || session_id.contains('\\') || session_id.starts_with('.') {
            return Err("session_id contains invalid characters for use in a filename".into());
        }

        let file_path = canonical.join(format!("{session_id}_telemetry.json"));

        // Verify the resulting path is still inside the canonical directory.
        if !file_path.starts_with(&canonical) {
            return Err("telemetry file path escapes the target directory".into());
        }

        // Build the artifact payload with derived values.
        let completion = self
            .completion_kind
            .map(|k| k.to_string())
            .unwrap_or_else(|| "none".to_string());

        let payload = serde_json::json!({
            "schema_version": "2",
            "session_id": session_id,
            "completion_kind": completion,
            "premature_final_count": self.premature_final_count,
            "total_final_requests": self.total_final_requests,
            "accepted_final_count": self.accepted_final_count(),
            "plan_registration_count": self.plan_registration_count,
            "plan_update_count": self.plan_update_count,
            "anvil_plan_visible_count": self.anvil_plan_visible_count,
            "last_mutation_turn": self.last_mutation_turn,
            "late_mutation_flag": self.is_late_mutation(),
            "final_suppressed_with_remaining_targets_count":
                self.final_suppressed_with_remaining_targets_count,
            "sync_from_touched_files_count": self.sync_from_touched_files_count,
            "forced_workset_transition_count": self.forced_workset_transition_count,
            "initial_plan_miss_count": self.initial_plan_miss_count,
            "no_op_mutation_count": self.no_op_mutation_count,
            "rolled_back_mutation_count": self.rolled_back_mutation_count,
            "plan_repair_request_count": self.plan_repair_request_count,
            "mutations_per_turn": self.mutations_per_turn,
            "items_advanced_per_turn": self.items_advanced_per_turn,
            "guidance_chars_per_turn": self.guidance_chars_per_turn,
            "workset_size_per_turn": self.workset_size_per_turn,
            "first_mutation_event_turn": self.first_mutation_event_turn,
            "first_mutation_event_elapsed_s": self.first_mutation_event_elapsed_s,
            "first_mutation_event_tool": self.first_mutation_event_tool,
            "first_mutation_event_semantic_basis": "runtime_lower_bound",
        });

        let json_bytes = serde_json::to_vec_pretty(&payload)?;

        // create_new: refuse to overwrite existing files.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file_path)
            .map_err(|e| format!("failed to create telemetry file: {e}"))?;

        use std::io::Write;
        file.write_all(&json_bytes)?;

        Ok(())
    }

    /// Premature Final Request Rate: ratio of suppressed finals to total finals.
    pub fn premature_final_request_rate(&self) -> f64 {
        if self.total_final_requests == 0 {
            return 0.0;
        }
        self.premature_final_count as f64 / self.total_final_requests as f64
    }
}

// ---------------------------------------------------------------------------
// Execution Plan types (Issue #249: Plan → Execute mode)
// ---------------------------------------------------------------------------

/// Status of an individual plan item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    /// Not yet started.
    Pending,
    /// Currently being executed.
    InProgress,
    /// Successfully completed.
    Done,
    /// Blocked due to repeated failures.
    Blocked,
    /// Superseded by a corrected ANVIL_PLAN_UPDATE item (Issue #289).
    Superseded,
    /// Retired via `[x]` marker in ANVIL_PLAN_UPDATE without mutations (Issue #301).
    AlreadySatisfied,
}

impl std::fmt::Display for PlanItemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Done => write!(f, "done"),
            Self::Blocked => write!(f, "blocked"),
            Self::Superseded => write!(f, "superseded"),
            Self::AlreadySatisfied => write!(f, "already_satisfied"),
        }
    }
}

/// A single item in the execution plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    /// Human-readable description of the work item.
    pub description: String,
    /// Target file paths (extracted from the description, if any).
    pub target_files: Vec<String>,
    /// Current status.
    pub status: PlanItemStatus,
    /// Number of consecutive execution failures for this item.
    #[serde(default)]
    pub retry_count: u8,
    /// Files that have been successfully mutated for this item (Issue #255).
    /// Used to track progress toward completing all `target_files`.
    #[serde(default)]
    pub mutated_files: Vec<String>,
}

impl PlanItem {
    /// Maximum consecutive failures before marking as Blocked.
    pub const MAX_RETRIES: u8 = 3;

    pub fn new(description: String, target_files: Vec<String>) -> Self {
        Self {
            description,
            target_files,
            status: PlanItemStatus::Pending,
            retry_count: 0,
            mutated_files: Vec::new(),
        }
    }

    /// Whether this item is considered finished (Done, Blocked, Superseded, or AlreadySatisfied).
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            PlanItemStatus::Done
                | PlanItemStatus::Blocked
                | PlanItemStatus::Superseded
                | PlanItemStatus::AlreadySatisfied
        )
    }
}

/// The execution plan maintained by Anvil (Issue #249).
///
/// Parsed from `ANVIL_PLAN` / `ANVIL_PLAN_UPDATE` blocks emitted by the LLM.
/// Controls ANVIL_FINAL acceptance: the loop cannot terminate until all items
/// are finished (Done or Blocked).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub items: Vec<PlanItem>,
}

/// Result of checking whether ANVIL_FINAL should be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalGateDecision {
    /// All items finished → allow ANVIL_FINAL.
    Allow,
    /// Plan not yet created → suppress and request plan creation.
    NoPlan,
    /// Unfinished items remain → suppress and guide to next item.
    Incomplete {
        next_description: String,
        remaining: usize,
        total: usize,
    },
}

/// Known file.edit result suffixes stripped before path comparison.
/// Mirrors `EditFallbackStage` variant suffixes without depending on the
/// `tooling` crate (avoids circular dependency).
const EDIT_FALLBACK_SUFFIXES: &[&str] = &[" (trailing-ws fallback)", " (anchor fallback)"];

/// Strip a known edit-fallback suffix from the end of a string.
fn strip_edit_fallback_suffix(s: &str) -> &str {
    for suffix in EDIT_FALLBACK_SUFFIXES {
        if let Some(stripped) = s.strip_suffix(suffix) {
            return stripped;
        }
    }
    s
}

/// Strip a parenthesized annotation suffix (e.g., `" (bar.ts)"`) from a path.
///
/// Only strips when there is a space before the opening paren (`" ("`) and the
/// string ends with `")"`.  This preserves legitimate paths like `foo(1).ts`.
/// Issue #289: prevents poisoned identity from bracket-annotated plan items.
fn strip_parenthesized_suffix(s: &str) -> &str {
    if let Some(pos) = s.find(" (")
        && s.ends_with(')')
    {
        return s[..pos].trim_end();
    }
    s
}

impl ExecutionPlan {
    pub fn new(items: Vec<PlanItem>) -> Self {
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of finished (Done or Blocked) items.
    pub fn finished_count(&self) -> usize {
        self.items.iter().filter(|i| i.is_finished()).count()
    }

    /// Whether all items are finished.
    pub fn all_finished(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(PlanItem::is_finished)
    }

    /// Get the index of the next pending or in-progress item.
    pub fn next_actionable_index(&self) -> Option<usize> {
        self.items.iter().position(|i| {
            matches!(
                i.status,
                PlanItemStatus::Pending | PlanItemStatus::InProgress
            )
        })
    }

    /// Mark an item as Done by index.
    pub fn mark_done(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            item.status = PlanItemStatus::Done;
        }
    }

    /// Mark an item as InProgress by index.
    pub fn mark_in_progress(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            item.status = PlanItemStatus::InProgress;
        }
    }

    /// Record a failure for the current in-progress item.
    /// Automatically transitions to Blocked after MAX_RETRIES.
    pub fn record_failure(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            item.retry_count += 1;
            if item.retry_count >= PlanItem::MAX_RETRIES {
                item.status = PlanItemStatus::Blocked;
            }
        }
    }

    /// Whether any item has Blocked status.
    pub fn has_blocked_items(&self) -> bool {
        self.items
            .iter()
            .any(|i| i.status == PlanItemStatus::Blocked)
    }

    /// Whether the plan completed successfully: all items finished, none Blocked,
    /// and at least one item is Done.
    /// Returns false for empty plans (via `all_finished()` internal guard).
    /// Superseded-only plans return false because no actual work was completed.
    /// AlreadySatisfied counts as successful completion alongside Done (Issue #301).
    pub fn is_successfully_completed(&self) -> bool {
        self.all_finished()
            && !self.has_blocked_items()
            && self.items.iter().any(|i| {
                i.status == PlanItemStatus::Done || i.status == PlanItemStatus::AlreadySatisfied
            })
    }

    /// Whether the plan reached a clean terminal state: all items finished with
    /// no blocked items (Issue #311).
    ///
    /// Unlike `is_successfully_completed()` which requires positive evidence
    /// (Done/AlreadySatisfied), this method also accepts superseded-only plans.
    /// Used by exit/recovery logic to ensure consistency with
    /// `check_final_gate` / `CompletionKind::classify`.
    pub fn is_cleanly_finished(&self) -> bool {
        self.all_finished() && !self.has_blocked_items()
    }

    /// Decide whether ANVIL_FINAL should be accepted.
    pub fn check_final_gate(&self) -> FinalGateDecision {
        if self.items.is_empty() {
            return FinalGateDecision::NoPlan;
        }
        if self.all_finished() {
            return FinalGateDecision::Allow;
        }
        let remaining = self.items.iter().filter(|i| !i.is_finished()).count();
        let next_desc = self
            .next_actionable_index()
            .and_then(|i| self.items.get(i))
            .map(|i| i.description.clone())
            .unwrap_or_default();
        FinalGateDecision::Incomplete {
            next_description: next_desc,
            remaining,
            total: self.items.len(),
        }
    }

    /// Fuzzy path match: true when either path is a suffix of the other.
    ///
    /// Strips known `EditFallbackStage` suffixes before comparison so that
    /// results like `"src/foo.rs (trailing-ws fallback)"` match `"src/foo.rs"`.
    ///
    /// Used consistently across `record_mutation_success`, `sync_from_touched_files`,
    /// and `update_plan_from_results` to avoid divergent matching behaviour.
    pub fn path_matches(a: &str, b: &str) -> bool {
        let a = strip_edit_fallback_suffix(a);
        let b = strip_edit_fallback_suffix(b);
        let a = strip_parenthesized_suffix(a);
        let b = strip_parenthesized_suffix(b);
        if a.is_empty() || b.is_empty() {
            return false;
        }
        a.ends_with(b) || b.ends_with(a)
    }

    /// Record a successful mutation for a plan item (Issue #255).
    ///
    /// Tracks which files have been mutated. When all `target_files` are
    /// covered (or item has no target_files), marks the item as Done.
    pub fn record_mutation_success(&mut self, index: usize, file_path: &str) {
        let item = match self.items.get_mut(index) {
            Some(i) => i,
            None => return,
        };
        if item.is_finished() {
            return;
        }

        // Add to mutated_files if not already present (Issue #287: use path_matches for dedup)
        if !item
            .mutated_files
            .iter()
            .any(|f| Self::path_matches(f, file_path))
        {
            item.mutated_files.push(file_path.to_string());
        }

        // Check completion: all target_files must be covered
        if item.target_files.is_empty() {
            // No explicit targets → any mutation completes
            item.status = PlanItemStatus::Done;
        } else {
            let all_covered = item.target_files.iter().all(|tf| {
                item.mutated_files
                    .iter()
                    .any(|mf| Self::path_matches(mf, tf))
            });
            if all_covered {
                item.status = PlanItemStatus::Done;
            }
        }
    }

    /// Return indices of the current workset: up to 5 actionable (Pending or InProgress) items.
    ///
    /// Used by batch guidance mode to identify items that can be executed together in a single turn.
    /// Returns empty Vec when all items are finished.
    pub fn current_workset(&self) -> Vec<usize> {
        const MAX_WORKSET_SIZE: usize = 5;
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches!(
                    item.status,
                    PlanItemStatus::Pending | PlanItemStatus::InProgress
                )
            })
            .take(MAX_WORKSET_SIZE)
            .map(|(i, _)| i)
            .collect()
    }

    /// Return the number of items in the current workset.
    ///
    /// Equivalent to `current_workset().len()` but avoids allocating the
    /// index vector, and is convenient for passing to `record_turn_metrics`.
    pub fn current_workset_size(&self) -> usize {
        const MAX_WORKSET_SIZE: usize = 5;
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    PlanItemStatus::Pending | PlanItemStatus::InProgress
                )
            })
            .take(MAX_WORKSET_SIZE)
            .count()
    }

    /// Append new items (used by ANVIL_PLAN_UPDATE).
    pub fn append_items(&mut self, new_items: Vec<PlanItem>) {
        self.items.extend(new_items);
    }

    /// Deduplicate new plan items against existing items (Issue #287).
    ///
    /// Excludes new items whose `target_files` match an existing item
    /// (using `path_matches` for fuzzy comparison).
    ///
    /// Issue #311: `Superseded` items are excluded from dedup matching so that
    /// corrected replacements survive and remain actionable in the plan.
    /// Without this, a supersede→dedup pipeline can produce a superseded-only
    /// terminal plan that diverges `check_final_gate` / `CompletionKind` from
    /// `is_successfully_completed` / exit semantics.
    pub fn deduplicate_new_items(&self, new_items: Vec<PlanItem>) -> Vec<PlanItem> {
        new_items
            .into_iter()
            .filter(|new_item| {
                let mut new_targets = new_item.target_files.clone();
                new_targets.sort();

                !self.items.iter().any(|existing| {
                    // Issue #311: Skip Superseded items — corrected replacements
                    // must not be deduped against the items they replaced.
                    if existing.status == PlanItemStatus::Superseded {
                        return false;
                    }
                    let mut existing_targets = existing.target_files.clone();
                    existing_targets.sort();
                    if new_targets.len() != existing_targets.len() {
                        return false;
                    }
                    new_targets
                        .iter()
                        .zip(existing_targets.iter())
                        .all(|(a, b)| Self::path_matches(a, b))
                })
            })
            .collect()
    }

    /// Supersede stale plan items that overlap with corrected new items (Issue #289).
    ///
    /// A stale item is one that is not yet finished, has no recorded mutations,
    /// and has at least one `target_file` that matches (via `path_matches`) a
    /// target in one of the `new_items`.  Such items are marked `Superseded` so
    /// they no longer block plan completion.
    ///
    /// To avoid false positives (CB-002), each new item can supersede at most one
    /// existing item, and each existing item can be superseded by at most one new
    /// item (1:1 matching).  New items are consumed greedily in order.
    pub fn supersede_stale_items(&mut self, new_items: &[PlanItem]) {
        // Track which new items have already been consumed.
        let mut new_consumed = vec![false; new_items.len()];
        for existing in &mut self.items {
            // Only supersede unfinished items with no mutations recorded yet.
            if existing.is_finished() || !existing.mutated_files.is_empty() {
                continue;
            }
            // Find the first unconsumed new item whose targets intersect.
            let matched = new_items.iter().enumerate().find(|(j, new_item)| {
                !new_consumed[*j]
                    && existing.target_files.iter().any(|et| {
                        new_item
                            .target_files
                            .iter()
                            .any(|nt| Self::path_matches(et, nt))
                    })
            });
            if let Some((j, _)) = matched {
                new_consumed[j] = true;
                tracing::info!(
                    description = %existing.description,
                    "plan item superseded by corrected ANVIL_PLAN_UPDATE"
                );
                existing.status = PlanItemStatus::Superseded;
            }
        }
    }

    /// Mark unfinished items whose `target_files` match the given list as `new_status` (Issue #301).
    ///
    /// Safety guard (DR4-001): only marks an item when:
    ///   1. The item is not already finished.
    ///   2. The item has non-empty `target_files`.
    ///   3. The number of `target_files` in the item matches the number of matched
    ///      entries in `target_files_to_retire` (1:1 target count match).
    ///   4. Every target file in the item has a 1:1 match in `target_files_to_retire`.
    ///
    /// Returns the list of target file paths that were actually retired.
    pub fn mark_unfinished_items_by_target(
        &mut self,
        target_files_to_retire: &[String],
        new_status: PlanItemStatus,
    ) -> Vec<String> {
        let mut retired: Vec<String> = Vec::new();
        for item in &mut self.items {
            if item.is_finished() || item.target_files.is_empty() {
                continue;
            }
            // Check 1:1 target count match: every item target must match exactly one
            // retire target, and the counts must be equal.
            let all_matched = item.target_files.iter().all(|tf| {
                target_files_to_retire
                    .iter()
                    .any(|rt| Self::path_matches(tf, rt))
            });
            // Count how many retire targets match this item's targets
            let matched_retire_count = target_files_to_retire
                .iter()
                .filter(|rt| {
                    item.target_files
                        .iter()
                        .any(|tf| Self::path_matches(tf, rt))
                })
                .count();
            if all_matched && matched_retire_count == item.target_files.len() {
                tracing::info!(
                    description = %item.description,
                    status = %new_status,
                    "plan item retired via checked marker (Issue #301)"
                );
                item.status = new_status;
                retired.extend(item.target_files.clone());
            }
        }
        retired
    }

    /// Auto-retire plan items that have no `target_files` (Issue #315).
    ///
    /// No-path / summary-only items (e.g. `- [ ] 残っている gap があれば修正`)
    /// violate the `<relative-path>: <description>` prompt contract and cannot
    /// be retired by any existing file-based mechanism.  Marking them as
    /// `AlreadySatisfied` on registration prevents them from blocking the
    /// final gate indefinitely.
    ///
    /// Returns the number of items auto-retired.
    pub fn auto_retire_no_path_items(&mut self) -> usize {
        let mut count = 0;
        for item in &mut self.items {
            if !item.is_finished() && item.target_files.is_empty() {
                tracing::info!(
                    description = %item.description,
                    "auto-retiring no-path plan item (Issue #315)"
                );
                item.status = PlanItemStatus::AlreadySatisfied;
                count += 1;
            }
        }
        count
    }

    /// Retire unfinished no-path plan items whose description matches one of
    /// the given descriptions (Issue #315).
    ///
    /// Used when a checked `[x]` item in `ANVIL_PLAN_UPDATE` has no
    /// `target_files`.  Because file-based `mark_unfinished_items_by_target`
    /// skips no-path items, this method provides a description-based fallback
    /// to retire the corresponding existing no-path item.
    ///
    /// Matching is case-insensitive and whitespace-normalized.
    ///
    /// Returns the number of items retired.
    pub fn retire_no_path_items_by_description(
        &mut self,
        descriptions: &[String],
        new_status: PlanItemStatus,
    ) -> usize {
        let normalized_targets: Vec<String> = descriptions
            .iter()
            .map(|d| Self::normalize_description(d))
            .collect();
        let mut count = 0;
        for item in &mut self.items {
            if item.is_finished() || !item.target_files.is_empty() {
                continue;
            }
            let normalized = Self::normalize_description(&item.description);
            if normalized_targets.contains(&normalized) {
                tracing::info!(
                    description = %item.description,
                    status = %new_status,
                    "no-path plan item retired via description match (Issue #315)"
                );
                item.status = new_status;
                count += 1;
            }
        }
        count
    }

    /// Normalize a description for fuzzy matching: trim, collapse whitespace,
    /// lowercase.
    fn normalize_description(desc: &str) -> String {
        desc.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// Sync plan item completion from the set of files actually modified.
    ///
    /// When a file.write/file.edit succeeds but the result is not passed to
    /// `update_plan_from_results` (e.g. because the tool call appeared after
    /// `ANVIL_FINAL` in the LLM response — Issue #251), the plan item stays
    /// Pending/InProgress even though the work is done.  This method fixes
    /// that by matching `touched_files` against each item's `target_files`.
    ///
    /// Issue #255: Changed from ANY to ALL target_files matching, consistent
    /// with the strengthened item completion condition.
    pub fn sync_from_touched_files(&mut self, touched_files: &[String]) {
        if self.items.is_empty() || touched_files.is_empty() {
            return;
        }
        let mut advanced = false;
        for item in &mut self.items {
            if item.is_finished() {
                continue;
            }
            if item.target_files.is_empty() {
                continue;
            }
            // Issue #255: Mark done only if ALL target files have been touched.
            let all_matched = item.target_files.iter().all(|tf| {
                touched_files
                    .iter()
                    .any(|touched| ExecutionPlan::path_matches(touched, tf))
            });
            if all_matched {
                tracing::info!(
                    description = %item.description,
                    "plan item completed (synced from touched_files)"
                );
                item.status = PlanItemStatus::Done;
                advanced = true;
            }
        }
        // Auto-advance next pending item to InProgress
        if advanced && let Some(next) = self.next_actionable_index() {
            self.items[next].status = PlanItemStatus::InProgress;
        }
    }

    /// Format the plan as a checklist string for display / system prompt injection.
    pub fn format_checklist(&self) -> String {
        let mut lines = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let marker = match item.status {
                PlanItemStatus::Done => "[x]",
                PlanItemStatus::Blocked => "[!]",
                PlanItemStatus::Superseded => "[~]",
                PlanItemStatus::AlreadySatisfied => "[=]",
                PlanItemStatus::InProgress => "[>]",
                PlanItemStatus::Pending => "[ ]",
            };
            lines.push(format!("  {}. {} {}", i + 1, marker, item.description));
        }
        lines.join("\n")
    }

    /// Build the system message to inject at the start of each execution turn.
    ///
    /// Uses Sequential mode (backward compatible). For mode-aware guidance,
    /// use [`build_turn_guidance_with_mode`].
    pub fn build_turn_guidance(&self) -> Option<String> {
        self.build_turn_guidance_with_mode(crate::config::GuidanceMode::Sequential)
    }

    /// Build the system message with explicit guidance mode.
    ///
    /// - `Sequential`: guides LLM to execute one item at a time (current behavior).
    /// - `Batch`: guides LLM to execute the current workset (up to 5 items) at once.
    pub fn build_turn_guidance_with_mode(
        &self,
        mode: crate::config::GuidanceMode,
    ) -> Option<String> {
        let idx = self.next_actionable_index()?;
        let finished = self.finished_count();
        let total = self.items.len();

        match mode {
            crate::config::GuidanceMode::Sequential => {
                let item = &self.items[idx];
                let safe_desc =
                    crate::app::stagnation_state::sanitize_for_prompt_entry(&item.description);
                // Issue #269 Phase 2: show untouched target files instead of full checklist.
                let untouched: Vec<String> = item
                    .target_files
                    .iter()
                    .filter(|tf| {
                        !item
                            .mutated_files
                            .iter()
                            .any(|mf| Self::path_matches(mf, tf))
                    })
                    .map(|tf| {
                        format!(
                            "    - {}",
                            crate::app::stagnation_state::sanitize_for_prompt_entry(tf)
                        )
                    })
                    .collect();
                let target_hint = if untouched.is_empty() {
                    String::new()
                } else {
                    format!("\n  未修正ファイル:\n{}", untouched.join("\n"))
                };
                Some(format!(
                    "[System] 計画の次の項目を実行してください:\n  {}. {}{}\n完了: {}/{} 項目\n\n1項目ずつ実行し、全項目完了時のみ ANVIL_FINAL を出力してください。",
                    idx + 1,
                    safe_desc,
                    target_hint,
                    finished,
                    total,
                ))
            }
            crate::config::GuidanceMode::Batch => {
                let workset = self.current_workset();
                let remaining = total - finished;
                let mut workset_lines = Vec::new();
                for &wi in &workset {
                    workset_lines.push(format!("  {}. {}", wi + 1, self.items[wi].description));
                }
                // Issue #269 Phase 2: removed full checklist re-display.
                Some(format!(
                    "[System] 以下の項目をまとめて実行してください:\n{}\n完了: {}/{} 項目 (残り {})\n\nこれらの項目をまとめて進めてください。全項目完了時のみ ANVIL_FINAL を出力してください。",
                    workset_lines.join("\n"),
                    finished,
                    total,
                    remaining,
                ))
            }
            crate::config::GuidanceMode::Minimal => {
                // Issue #269 Phase 1: minimal guidance for baseline comparison arm.
                // Suppresses verbose checklists; only shows a terse reminder.
                Some(format!(
                    "[System] Proceed with next plan item. ({}/{} done)",
                    finished, total
                ))
            }
        }
    }

    /// Build the incomplete plan message for ANVIL_FINAL suppression.
    ///
    /// - `Sequential`: simple "next item" message (backward compatible).
    /// - `Batch`: includes completed items, pending items (workset), and batch instruction.
    pub fn build_incomplete_plan_message_with_mode(
        &self,
        mode: crate::config::GuidanceMode,
    ) -> String {
        let finished = self.finished_count();
        let total = self.items.len();
        let remaining = total - finished;

        match mode {
            crate::config::GuidanceMode::Sequential => {
                // Issue #269 Phase 2: show untouched target files of next item.
                let next_item = self.next_actionable_index().and_then(|i| self.items.get(i));
                let next_desc = next_item
                    .map(|i| {
                        crate::app::stagnation_state::sanitize_for_prompt_entry(&i.description)
                    })
                    .unwrap_or_default();
                let target_hint = next_item
                    .map(|item| {
                        let untouched: Vec<String> = item
                            .target_files
                            .iter()
                            .filter(|tf| {
                                !item
                                    .mutated_files
                                    .iter()
                                    .any(|mf| Self::path_matches(mf, tf))
                            })
                            .map(|tf| {
                                format!(
                                    "    - {}",
                                    crate::app::stagnation_state::sanitize_for_prompt_entry(tf)
                                )
                            })
                            .collect();
                        if untouched.is_empty() {
                            String::new()
                        } else {
                            format!("\n  未修正ファイル:\n{}", untouched.join("\n"))
                        }
                    })
                    .unwrap_or_default();
                format!(
                    "[System] まだ {remaining}/{total} 項目が未完了です。次の項目を実行してください:\n  {next_desc}{target_hint}\n\
                     全項目完了後に ANVIL_FINAL を出力してください。"
                )
            }
            crate::config::GuidanceMode::Batch => {
                // Completed items (Done or AlreadySatisfied — Issue #301)
                let completed_lines: Vec<String> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        item.status == PlanItemStatus::Done
                            || item.status == PlanItemStatus::AlreadySatisfied
                    })
                    .map(|(i, item)| {
                        let safe = crate::app::stagnation_state::sanitize_for_prompt_entry(
                            &item.description,
                        );
                        format!("  {}. {}", i + 1, safe)
                    })
                    .collect();

                // Workset (pending/in-progress items)
                let workset = self.current_workset();
                let workset_lines: Vec<String> = workset
                    .iter()
                    .map(|&i| format!("  {}. {}", i + 1, self.items[i].description))
                    .collect();

                let mut msg = format!("[System] まだ {remaining}/{total} 項目が未完了です。\n\n");

                if !completed_lines.is_empty() {
                    msg.push_str(&format!(
                        "完了済み ({}/{total}):\n{}\n\n",
                        completed_lines.len(),
                        completed_lines.join("\n")
                    ));
                }

                if !workset_lines.is_empty() {
                    msg.push_str(&format!(
                        "未完了 (次のworkset):\n{}\n\n",
                        workset_lines.join("\n")
                    ));
                }

                msg.push_str(
                    "これらの項目をまとめて実行してください。全項目完了後に ANVIL_FINAL を出力してください。",
                );
                msg
            }
            crate::config::GuidanceMode::Minimal => {
                // Issue #269 Phase 1: minimal incomplete message for baseline comparison arm.
                format!("Plan incomplete: {remaining} of {total} items remain.")
            }
        }
    }

    /// Build turn guidance with a precomputed workset and forced mode flag (Issue #263).
    ///
    /// When `forced_mode` is true, includes a stagnation warning in the guidance.
    pub fn build_turn_guidance_with_workset(
        &self,
        mode: crate::config::GuidanceMode,
        workset: &[usize],
        forced_mode: bool,
    ) -> Option<String> {
        if workset.is_empty() {
            return self.build_turn_guidance_with_mode(mode);
        }

        let finished = self.finished_count();
        let total = self.items.len();
        let remaining = total - finished;

        let mut workset_lines = Vec::new();
        for &wi in workset {
            if let Some(item) = self.items.get(wi) {
                let safe_desc =
                    crate::app::stagnation_state::sanitize_for_prompt_entry(&item.description);
                workset_lines.push(format!("  {}. {}", wi + 1, safe_desc));
            }
        }

        let forced_prefix = if forced_mode {
            "⚠ STAGNATION DETECTED — You MUST change your approach.\n\n"
        } else {
            ""
        };

        // Issue #269 Phase 2: removed full checklist re-display.
        Some(format!(
            "{forced_prefix}[System] 以下の項目をまとめて実行してください:\n{}\n完了: {}/{} 項目 (残り {})\n\nこれらの項目をまとめて進めてください。全項目完了時のみ ANVIL_FINAL を出力してください。",
            workset_lines.join("\n"),
            finished,
            total,
            remaining,
        ))
    }

    /// Build incomplete plan message with precomputed workset (Issue #263).
    pub fn build_incomplete_plan_message_with_workset(
        &self,
        mode: crate::config::GuidanceMode,
        workset: &[usize],
        forced_mode: bool,
    ) -> String {
        if workset.is_empty() {
            return self.build_incomplete_plan_message_with_mode(mode);
        }

        let finished = self.finished_count();
        let total = self.items.len();
        let remaining = total - finished;

        let forced_prefix = if forced_mode {
            "⚠ STAGNATION DETECTED — Forced workset transition active.\n\n"
        } else {
            ""
        };

        let workset_lines: Vec<String> = workset
            .iter()
            .filter_map(|&i| {
                self.items.get(i).map(|item| {
                    let safe_desc =
                        crate::app::stagnation_state::sanitize_for_prompt_entry(&item.description);
                    format!("  {}. {}", i + 1, safe_desc)
                })
            })
            .collect();

        format!(
            "{forced_prefix}[System] まだ {remaining}/{total} 項目が未完了です。\n\n\
             未完了 (次のworkset):\n{}\n\n\
             これらの項目をまとめて実行してください。全項目完了後に ANVIL_FINAL を出力してください。",
            workset_lines.join("\n")
        )
    }
    /// Build a late-stage closure hint when only 1 actionable item remains (Issue #309).
    ///
    /// Returns `Some(message)` when exactly 1 unfinished item remains and
    /// the plan has made substantial progress (>= 50% done). The message
    /// strongly guides the agent to finish or structurally retire the last item
    /// instead of drifting into shell-based inspection.
    pub fn build_late_stage_closure_hint(&self) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        let finished = self.finished_count();
        let total = self.items.len();
        let remaining = total - finished;

        if remaining != 1 || finished == 0 {
            return None;
        }

        // Find the remaining item
        let next_item = self.next_actionable_index().and_then(|i| self.items.get(i));
        let next_item = next_item?;
        let safe_desc =
            crate::app::stagnation_state::sanitize_for_prompt_entry(&next_item.description);
        let target_files: Vec<String> = next_item
            .target_files
            .iter()
            .map(|tf| crate::app::stagnation_state::sanitize_for_prompt_entry(tf))
            .collect();
        let files_hint = if target_files.is_empty() {
            String::new()
        } else {
            format!("\n  対象ファイル: {}", target_files.join(", "))
        };

        Some(format!(
            "[System] ⚠ CLOSURE MODE: 残り1項目です ({finished}/{total} 完了)。\n\
             最後の項目: {safe_desc}{files_hint}\n\n\
             この項目を完了させてください:\n\
             - file.edit / file.write で対象ファイルを変更する\n\
             - または変更不要なら ANVIL_PLAN_UPDATE で [x] マークして退役させる\n\
             - shell.exec での追加調査は不要です。すぐに実装に進んでください。"
        ))
    }
}

// ---------------------------------------------------------------------------
// Application lifecycle types
// ---------------------------------------------------------------------------

/// The runtime lifecycle states of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeState {
    Ready,
    Thinking,
    Working,
    AwaitingApproval,
    Interrupted,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppEvent {
    ConfigLoaded,
    ProviderBootstrapped,
    StartupCompleted,
    StateChanged,
    PlanItemAdded,
    PlanFocusChanged,
    PlanCleared,
    PlanCheckpointSaved,
    SessionCompacted,
    SessionLoaded,
    SessionSaved,
    SessionNormalizedAfterInterrupt,
    UndoExecuted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub items: Vec<String>,
    pub active_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    pub tool_name: String,
    pub summary: String,
    pub risk: String,
    pub tool_call_id: String,
    #[serde(skip)]
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptView {
    pub interrupted_what: String,
    pub saved_status: String,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLogView {
    pub tool_name: String,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
}

/// Context usage warning level based on threshold evaluation.
///
/// Used as `Option<ContextWarningLevel>` where `None` means no warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextWarningLevel {
    /// Usage >= 80%: warning
    Warning,
    /// Usage >= 90%: critical
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageView {
    pub estimated_tokens: usize,
    pub max_tokens: u32,
}

impl ContextUsageView {
    /// Context usage ratio clamped to 0.0..=1.0. Returns 0.0 if max_tokens is 0.
    pub fn usage_ratio(&self) -> f64 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        let ratio = self.estimated_tokens as f64 / self.max_tokens as f64;
        ratio.clamp(0.0, 1.0)
    }

    /// Warning level based on usage thresholds (>=0.9 Critical, >=0.8 Warning, else None).
    pub fn warning_level(&self) -> Option<ContextWarningLevel> {
        let ratio = self.usage_ratio();
        if ratio >= 0.9 {
            Some(ContextWarningLevel::Critical)
        } else if ratio >= 0.8 {
            Some(ContextWarningLevel::Warning)
        } else {
            None
        }
    }

    /// Usage percentage for display (0..=100).
    pub fn usage_percent(&self) -> u32 {
        (self.usage_ratio() * 100.0).round() as u32
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferencePerformanceView {
    /// tokens/sec * 10 (integer). e.g. 32.5 tok/s -> 325
    pub tokens_per_sec_tenths: Option<u64>,
    /// Generated token count (for session persistence / debug)
    pub eval_tokens: Option<u64>,
    /// Evaluation time in milliseconds (for session persistence / debug)
    pub eval_duration_ms: Option<u64>,
    /// Actual prompt token count from provider response.
    /// Ollama: `prompt_eval_count`, OpenAI: `prompt_tokens`.
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
}

impl InferencePerformanceView {
    /// Return a formatted string for TUI display.
    pub fn formatted_tokens_per_sec(&self) -> Option<String> {
        self.tokens_per_sec_tenths
            .map(|tenths| format!("{}.{}tok/s", tenths / 10, tenths % 10))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleMessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleMessageView {
    pub role: ConsoleMessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRenderContext {
    pub snapshot: AppStateSnapshot,
    pub model_name: String,
    pub messages: Vec<ConsoleMessageView>,
    pub history_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStateSnapshot {
    /// Current lifecycle state. Set in every state.
    pub state: RuntimeState,
    /// Most recent event that caused this snapshot. Set in every state.
    #[serde(default)]
    pub last_event: Option<AppEvent>,
    /// Human-readable status line. Set in every state.
    #[serde(default)]
    pub status: StatusView,
    /// Active plan items. Used in: Thinking, Working, Done.
    #[serde(default)]
    pub plan: Option<PlanView>,
    /// LLM reasoning steps. Used in: Thinking.
    #[serde(default)]
    pub reasoning_summary: Vec<String>,
    /// Pending approval details. Used in: AwaitingApproval.
    #[serde(default)]
    pub approval: Option<ApprovalView>,
    /// Interrupt details. Used in: Interrupted.
    #[serde(default)]
    pub interrupt: Option<InterruptView>,
    /// Tool execution log entries. Used in: Working, Done.
    #[serde(default)]
    pub tool_logs: Vec<ToolLogView>,
    /// Wall-clock milliseconds for the current turn. Used in: Thinking, Working, Done, Interrupted, Error.
    #[serde(default)]
    pub elapsed_ms: Option<u128>,
    /// Token budget usage. Set in every state.
    #[serde(default)]
    pub context_usage: Option<ContextUsageView>,
    /// Summary of what was accomplished. Used in: Done.
    #[serde(default)]
    pub completion_summary: Option<String>,
    /// Session persistence status. Used in: Done, Interrupted.
    #[serde(default)]
    pub saved_status: Option<String>,
    /// Error description. Used in: Error.
    #[serde(default)]
    pub error_summary: Option<String>,
    /// Suggested recovery actions. Used in: Error, Interrupted.
    #[serde(default)]
    pub recommended_actions: Vec<String>,
    /// Context overflow warning level. Used in: Done.
    #[serde(default)]
    pub context_warning: Option<ContextWarningLevel>,
    /// Inference performance metrics. Used in: Done.
    #[serde(default)]
    pub inference_performance: Option<InferencePerformanceView>,
}

impl AppStateSnapshot {
    pub fn new(state: RuntimeState) -> Self {
        Self {
            state,
            last_event: None,
            status: StatusView {
                line: String::new(),
            },
            plan: None,
            reasoning_summary: Vec::new(),
            approval: None,
            interrupt: None,
            tool_logs: Vec::new(),
            elapsed_ms: None,
            context_usage: None,
            completion_summary: None,
            saved_status: None,
            error_summary: None,
            recommended_actions: Vec::new(),
            context_warning: None,
            inference_performance: None,
        }
    }

    pub fn with_event(mut self, event: AppEvent) -> Self {
        self.last_event = Some(event);
        self
    }

    pub fn with_status(mut self, status: String) -> Self {
        self.status = StatusView { line: status };
        self
    }

    pub fn with_plan(mut self, items: Vec<String>, active_index: Option<usize>) -> Self {
        self.plan = Some(PlanView {
            items,
            active_index,
        });
        self
    }

    pub fn with_reasoning_summary(mut self, reasoning_summary: Vec<String>) -> Self {
        self.reasoning_summary = reasoning_summary;
        self
    }

    pub fn with_approval(
        mut self,
        tool_name: String,
        summary: String,
        risk: String,
        tool_call_id: String,
    ) -> Self {
        self.approval = Some(ApprovalView {
            tool_name,
            summary,
            risk,
            tool_call_id,
            diff_preview: None,
        });
        self
    }

    pub fn with_diff_preview(mut self, preview: Option<String>) -> Self {
        if let Some(ref mut approval) = self.approval {
            approval.diff_preview = preview;
        }
        self
    }

    pub fn with_interrupt(
        mut self,
        interrupted_what: String,
        saved_status: String,
        next_actions: Vec<String>,
    ) -> Self {
        self.interrupt = Some(InterruptView {
            interrupted_what,
            saved_status,
            next_actions,
        });
        self
    }

    pub fn with_tool_logs(mut self, tool_logs: Vec<ToolLogView>) -> Self {
        self.tool_logs = tool_logs;
        self
    }

    pub fn with_elapsed_ms(mut self, elapsed_ms: u128) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }

    pub fn with_context_usage(mut self, estimated_tokens: usize, max_tokens: u32) -> Self {
        self.context_usage = Some(ContextUsageView {
            estimated_tokens,
            max_tokens,
        });
        self
    }

    pub fn with_completion_summary(
        mut self,
        completion_summary: impl Into<String>,
        saved_status: impl Into<String>,
    ) -> Self {
        self.completion_summary = Some(completion_summary.into());
        self.saved_status = Some(saved_status.into());
        self
    }

    pub fn with_error_summary(
        mut self,
        error_summary: impl Into<String>,
        recommended_actions: Vec<String>,
    ) -> Self {
        self.error_summary = Some(error_summary.into());
        self.recommended_actions = recommended_actions;
        self
    }

    pub fn with_context_warning(mut self, level: ContextWarningLevel) -> Self {
        self.context_warning = Some(level);
        self
    }

    pub fn with_inference_performance(mut self, perf: InferencePerformanceView) -> Self {
        self.inference_performance = Some(perf);
        self
    }
}

#[cfg(test)]
impl AppStateSnapshot {
    /// Assert that the snapshot has the expected fields populated for its state.
    pub fn assert_valid_for_state(&self) {
        match self.state {
            RuntimeState::AwaitingApproval => {
                assert!(
                    self.approval.is_some(),
                    "AwaitingApproval must have approval"
                );
            }
            RuntimeState::Done => {
                assert!(
                    self.completion_summary.is_some(),
                    "Done must have completion_summary"
                );
            }
            RuntimeState::Error => {
                assert!(
                    self.error_summary.is_some(),
                    "Error must have error_summary"
                );
            }
            RuntimeState::Interrupted => {
                assert!(self.interrupt.is_some(), "Interrupted must have interrupt");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ratio_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 5000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_zero_max_tokens() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert!((usage.usage_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_clamped_above_one() {
        let usage = ContextUsageView {
            estimated_tokens: 15000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_boundary_values() {
        let usage_80 = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert!((usage_80.usage_ratio() - 0.8).abs() < f64::EPSILON);

        let usage_90 = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert!((usage_90.usage_ratio() - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn warning_level_below_80_is_none() {
        let usage = ContextUsageView {
            estimated_tokens: 7999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), None);
    }

    #[test]
    fn warning_level_at_80_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_89_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_90_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn warning_level_at_100_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 10000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn usage_percent_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 2200,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 22);
    }

    #[test]
    fn usage_percent_rounding() {
        let usage = ContextUsageView {
            estimated_tokens: 3333,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 33);
    }

    #[test]
    fn usage_percent_zero_max() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert_eq!(usage.usage_percent(), 0);
    }

    #[test]
    fn inference_performance_default_is_all_none() {
        let perf = InferencePerformanceView::default();
        assert_eq!(perf.tokens_per_sec_tenths, None);
        assert_eq!(perf.eval_tokens, None);
        assert_eq!(perf.eval_duration_ms, None);
        assert_eq!(perf.formatted_tokens_per_sec(), None);
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            ..Default::default()
        };
        assert_eq!(
            perf.formatted_tokens_per_sec(),
            Some("32.5tok/s".to_string())
        );
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec_zero_fraction() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(100),
            ..Default::default()
        };
        assert_eq!(
            perf.formatted_tokens_per_sec(),
            Some("10.0tok/s".to_string())
        );
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec_none_when_no_tenths() {
        let perf = InferencePerformanceView {
            eval_tokens: Some(50),
            eval_duration_ms: Some(1000),
            ..Default::default()
        };
        assert_eq!(perf.formatted_tokens_per_sec(), None);
    }

    #[test]
    fn inference_performance_serialize_deserialize() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            ..Default::default()
        };
        let json = serde_json::to_string(&perf).expect("serialize");
        let back: InferencePerformanceView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(perf, back);
    }

    #[test]
    fn app_state_snapshot_backward_compat_without_inference_performance() {
        // Old JSON without inference_performance field should still deserialize
        let json = r#"{
            "state": "Done",
            "status": {"line": "Done."},
            "reasoning_summary": [],
            "tool_logs": [],
            "recommended_actions": []
        }"#;
        let snapshot: AppStateSnapshot = serde_json::from_str(json).expect("deserialize");
        assert!(snapshot.inference_performance.is_none());
    }

    #[test]
    fn tool_log_view_elapsed_ms_default_none() {
        let json = r#"{"tool_name":"Read","action":"open","target":"src/main.rs"}"#;
        let view: ToolLogView = serde_json::from_str(json).expect("deserialize");
        assert_eq!(view.elapsed_ms, None);
    }

    #[test]
    fn tool_log_view_elapsed_ms_round_trip() {
        let view = ToolLogView {
            tool_name: "Read".to_string(),
            action: "open".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: Some(1234),
        };
        let json = serde_json::to_string(&view).expect("serialize");
        let back: ToolLogView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.elapsed_ms, Some(1234));
    }

    #[test]
    fn app_state_snapshot_backward_compat_without_elapsed_ms() {
        // Old JSON with tool_logs that lack elapsed_ms should still deserialize
        let json = r#"{
            "state": "Working",
            "status": {"line": "Working..."},
            "reasoning_summary": [],
            "tool_logs": [{"tool_name":"Read","action":"open","target":"src/main.rs"}],
            "recommended_actions": []
        }"#;
        let snapshot: AppStateSnapshot = serde_json::from_str(json).expect("deserialize");
        assert_eq!(snapshot.tool_logs.len(), 1);
        assert_eq!(snapshot.tool_logs[0].elapsed_ms, None);
    }

    #[test]
    fn inference_perf_without_prompt_tokens() {
        // Old JSON without prompt_tokens field should still deserialize
        let json = r#"{"tokens_per_sec_tenths":325,"eval_tokens":100,"eval_duration_ms":3077}"#;
        let perf: InferencePerformanceView = serde_json::from_str(json).expect("deserialize");
        assert_eq!(perf.tokens_per_sec_tenths, Some(325));
        assert_eq!(perf.prompt_tokens, None);
    }

    #[test]
    fn inference_perf_with_prompt_tokens() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            prompt_tokens: Some(500),
        };
        let json = serde_json::to_string(&perf).expect("serialize");
        let back: InferencePerformanceView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(perf, back);
        assert_eq!(back.prompt_tokens, Some(500));
    }

    #[test]
    fn app_state_snapshot_with_inference_performance_builder() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(200),
            eval_tokens: Some(80),
            eval_duration_ms: Some(4000),
            ..Default::default()
        };
        let snapshot =
            AppStateSnapshot::new(RuntimeState::Done).with_inference_performance(perf.clone());
        assert_eq!(snapshot.inference_performance, Some(perf));
    }

    // ============================================================
    // TerminationReason tests (Issue #129, Task 1.1)
    // ============================================================

    #[test]
    fn termination_reason_default_is_completed() {
        assert_eq!(TerminationReason::default(), TerminationReason::Completed);
    }

    #[test]
    fn termination_reason_display() {
        assert_eq!(TerminationReason::Completed.to_string(), "completed");
        assert_eq!(TerminationReason::Timeout.to_string(), "timeout");
        assert_eq!(
            TerminationReason::MaxIterations.to_string(),
            "max_iterations"
        );
        assert_eq!(TerminationReason::LoopDetected.to_string(), "loop_detected");
        assert_eq!(
            TerminationReason::MaxToolCalls.to_string(),
            "max_tool_calls"
        );
    }

    #[test]
    fn termination_reason_serde_roundtrip() {
        for reason in [
            TerminationReason::Completed,
            TerminationReason::Timeout,
            TerminationReason::MaxIterations,
            TerminationReason::LoopDetected,
            TerminationReason::MaxToolCalls,
        ] {
            let json = serde_json::to_string(&reason).expect("serialize");
            let back: TerminationReason = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(reason, back);
        }
    }

    #[test]
    fn termination_reason_serde_snake_case() {
        let json = serde_json::to_string(&TerminationReason::MaxIterations).expect("serialize");
        assert_eq!(json, r#""max_iterations""#);
        let back: TerminationReason =
            serde_json::from_str(r#""max_iterations""#).expect("deserialize");
        assert_eq!(back, TerminationReason::MaxIterations);
    }

    // ============================================================
    // Finding tests (Issue #129, Task 1.2)
    // ============================================================

    #[test]
    fn finding_serde_roundtrip() {
        let finding = Finding {
            title: "Found pattern".to_string(),
            detail: "The module uses X pattern".to_string(),
            related_code: vec!["src/main.rs:10".to_string(), "src/lib.rs:20".to_string()],
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finding, back);
    }

    #[test]
    fn finding_empty_related_code() {
        let finding = Finding {
            title: "Simple finding".to_string(),
            detail: "No code refs".to_string(),
            related_code: vec![],
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finding, back);
    }

    // ============================================================
    // SubAgentPayload tests (Issue #129, Task 1.3)
    // ============================================================

    #[test]
    fn subagent_payload_full_roundtrip() {
        let payload = SubAgentPayload {
            found_files: vec!["src/main.rs".to_string()],
            key_findings: vec![Finding {
                title: "Entry point".to_string(),
                detail: "Main function found".to_string(),
                related_code: vec!["src/main.rs:1".to_string()],
            }],
            raw_summary: "Found the entry point".to_string(),
            confidence: Some(0.9),
            termination_reason: TerminationReason::Completed,
            error: None,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: SubAgentPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.found_files, payload.found_files);
        assert_eq!(back.key_findings, payload.key_findings);
        assert_eq!(back.raw_summary, payload.raw_summary);
        assert_eq!(back.confidence, payload.confidence);
        assert_eq!(back.termination_reason, payload.termination_reason);
        assert_eq!(back.error, payload.error);
    }

    #[test]
    fn subagent_payload_defaults_for_optional_fields() {
        // JSON without optional fields should deserialize with defaults
        let json = r#"{"found_files":[],"key_findings":[],"raw_summary":"hello"}"#;
        let payload: SubAgentPayload = serde_json::from_str(json).expect("deserialize");
        assert_eq!(payload.confidence, None);
        assert_eq!(payload.termination_reason, TerminationReason::Completed);
        assert_eq!(payload.error, None);
    }

    #[test]
    fn subagent_payload_fallback_constructor() {
        let payload =
            SubAgentPayload::fallback("partial result".to_string(), TerminationReason::Timeout);
        assert!(payload.found_files.is_empty());
        assert!(payload.key_findings.is_empty());
        assert_eq!(payload.raw_summary, "partial result");
        assert_eq!(payload.confidence, None);
        assert_eq!(payload.termination_reason, TerminationReason::Timeout);
        assert_eq!(payload.error, None);
    }

    #[test]
    fn subagent_payload_with_error() {
        let payload = SubAgentPayload {
            found_files: vec![],
            key_findings: vec![],
            raw_summary: String::new(),
            confidence: None,
            termination_reason: TerminationReason::Timeout,
            error: Some("timed out during exploration".to_string()),
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: SubAgentPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.error, Some("timed out during exploration".to_string()));
        assert_eq!(back.termination_reason, TerminationReason::Timeout);
    }

    // ============================================================
    // ExecutionPlan tests (Issue #249)
    // ============================================================

    #[test]
    fn plan_item_new_defaults_to_pending() {
        let item = PlanItem::new("do stuff".into(), vec!["src/main.rs".into()]);
        assert_eq!(item.status, PlanItemStatus::Pending);
        assert_eq!(item.retry_count, 0);
        assert!(!item.is_finished());
    }

    #[test]
    fn plan_item_is_finished() {
        let mut item = PlanItem::new("x".into(), vec![]);
        assert!(!item.is_finished());
        item.status = PlanItemStatus::Done;
        assert!(item.is_finished());
        item.status = PlanItemStatus::Blocked;
        assert!(item.is_finished());
        item.status = PlanItemStatus::InProgress;
        assert!(!item.is_finished());
    }

    #[test]
    fn execution_plan_empty_default() {
        let plan = ExecutionPlan::default();
        assert!(plan.is_empty());
        assert!(!plan.all_finished());
        assert_eq!(plan.finished_count(), 0);
        assert_eq!(plan.next_actionable_index(), None);
    }

    #[test]
    fn execution_plan_mark_done_advances() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("a".into(), vec![]),
            PlanItem::new("b".into(), vec![]),
            PlanItem::new("c".into(), vec![]),
        ]);
        plan.mark_in_progress(0);
        assert_eq!(plan.next_actionable_index(), Some(0));

        plan.mark_done(0);
        assert_eq!(plan.finished_count(), 1);
        assert!(!plan.all_finished());
        assert_eq!(plan.next_actionable_index(), Some(1));

        plan.mark_done(1);
        plan.mark_done(2);
        assert!(plan.all_finished());
        assert_eq!(plan.next_actionable_index(), None);
    }

    #[test]
    fn execution_plan_record_failure_blocks_after_max() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("x".into(), vec![])]);
        plan.mark_in_progress(0);

        for _ in 0..PlanItem::MAX_RETRIES - 1 {
            plan.record_failure(0);
            assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
        }
        plan.record_failure(0);
        assert_eq!(plan.items[0].status, PlanItemStatus::Blocked);
        assert!(plan.all_finished());
    }

    #[test]
    fn execution_plan_check_final_gate_no_plan() {
        let plan = ExecutionPlan::default();
        assert_eq!(plan.check_final_gate(), FinalGateDecision::NoPlan);
    }

    #[test]
    fn execution_plan_check_final_gate_incomplete() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("first".into(), vec![]),
            PlanItem::new("second".into(), vec![]),
        ]);
        plan.mark_in_progress(0);
        match plan.check_final_gate() {
            FinalGateDecision::Incomplete {
                remaining, total, ..
            } => {
                assert_eq!(remaining, 2);
                assert_eq!(total, 2);
            }
            other => panic!("expected Incomplete, got {other:?}"),
        }
    }

    #[test]
    fn execution_plan_check_final_gate_allow() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("only".into(), vec![])]);
        plan.mark_done(0);
        assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    }

    #[test]
    fn execution_plan_append_items() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("a".into(), vec![])]);
        plan.append_items(vec![PlanItem::new("b".into(), vec![])]);
        assert_eq!(plan.items.len(), 2);
    }

    #[test]
    fn execution_plan_format_checklist() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("done item".into(), vec![]),
            PlanItem::new("in progress".into(), vec![]),
            PlanItem::new("pending item".into(), vec![]),
        ]);
        plan.mark_done(0);
        plan.mark_in_progress(1);
        let checklist = plan.format_checklist();
        assert!(checklist.contains("[x]"));
        assert!(checklist.contains("[>]"));
        assert!(checklist.contains("[ ]"));
    }

    #[test]
    fn execution_plan_build_turn_guidance() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("first task".into(), vec![]),
            PlanItem::new("second task".into(), vec![]),
        ]);
        plan.mark_done(0);
        plan.mark_in_progress(1);
        let guidance = plan.build_turn_guidance().expect("should have guidance");
        assert!(guidance.contains("second task"));
        assert!(guidance.contains("1/2"));
    }

    #[test]
    fn execution_plan_build_turn_guidance_none_when_all_done() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("x".into(), vec![])]);
        plan.mark_done(0);
        assert!(plan.build_turn_guidance().is_none());
    }

    #[test]
    fn plan_item_status_display() {
        assert_eq!(PlanItemStatus::Pending.to_string(), "pending");
        assert_eq!(PlanItemStatus::InProgress.to_string(), "in_progress");
        assert_eq!(PlanItemStatus::Done.to_string(), "done");
        assert_eq!(PlanItemStatus::Blocked.to_string(), "blocked");
        assert_eq!(PlanItemStatus::Superseded.to_string(), "superseded");
    }

    #[test]
    fn plan_item_serde_roundtrip() {
        let item = PlanItem::new("desc".into(), vec!["src/lib.rs".into()]);
        let json = serde_json::to_string(&item).expect("serialize");
        let back: PlanItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item, back);
    }

    #[test]
    fn execution_plan_serde_roundtrip() {
        let plan = ExecutionPlan::new(vec![
            PlanItem::new("a".into(), vec!["f1.rs".into()]),
            PlanItem::new("b".into(), vec![]),
        ]);
        let json = serde_json::to_string(&plan).expect("serialize");
        let back: ExecutionPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plan, back);
    }

    // Issue #251: sync_from_touched_files tests

    #[test]
    fn sync_from_touched_files_marks_matching_items_done() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("src/lib.rs: add docs".into(), vec!["src/lib.rs".into()]),
            PlanItem::new("src/main.rs: add docs".into(), vec!["src/main.rs".into()]),
        ]);
        plan.mark_in_progress(0);

        let touched = vec!["src/lib.rs".to_string()];
        plan.sync_from_touched_files(&touched);

        assert_eq!(plan.items[0].status, PlanItemStatus::Done);
        assert_eq!(plan.items[1].status, PlanItemStatus::InProgress); // auto-advanced
    }

    #[test]
    fn sync_from_touched_files_all_done_allows_final_gate() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("src/lib.rs: add docs".into(), vec!["src/lib.rs".into()]),
            PlanItem::new("src/main.rs: add docs".into(), vec!["src/main.rs".into()]),
        ]);
        plan.mark_in_progress(0);

        let touched = vec!["src/lib.rs".to_string(), "src/main.rs".to_string()];
        plan.sync_from_touched_files(&touched);

        assert_eq!(plan.items[0].status, PlanItemStatus::Done);
        assert_eq!(plan.items[1].status, PlanItemStatus::Done);
        assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    }

    #[test]
    fn sync_from_touched_files_no_match_leaves_incomplete() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "src/lib.rs: add docs".into(),
            vec!["src/lib.rs".into()],
        )]);
        plan.mark_in_progress(0);

        let touched = vec!["src/other.rs".to_string()];
        plan.sync_from_touched_files(&touched);

        assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
    }

    #[test]
    fn sync_from_touched_files_empty_touched_is_noop() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "src/lib.rs: add docs".into(),
            vec!["src/lib.rs".into()],
        )]);
        plan.mark_in_progress(0);

        plan.sync_from_touched_files(&[]);
        assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
    }

    #[test]
    fn sync_from_touched_files_skips_already_done_items() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("src/lib.rs: add docs".into(), vec!["src/lib.rs".into()]),
            PlanItem::new("src/main.rs: add docs".into(), vec!["src/main.rs".into()]),
        ]);
        plan.mark_done(0);
        plan.mark_in_progress(1);

        let touched = vec!["src/lib.rs".to_string(), "src/main.rs".to_string()];
        plan.sync_from_touched_files(&touched);

        assert_eq!(plan.items[0].status, PlanItemStatus::Done);
        assert_eq!(plan.items[1].status, PlanItemStatus::Done);
    }

    #[test]
    fn sync_from_touched_files_suffix_matching() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "/tmp/test1.js: add comments".into(),
            vec!["/tmp/test1.js".into()],
        )]);
        plan.mark_in_progress(0);

        // touched_files uses relative paths; target_files may use absolute
        let touched = vec!["test1.js".to_string()];
        plan.sync_from_touched_files(&touched);

        assert_eq!(plan.items[0].status, PlanItemStatus::Done);
    }

    #[test]
    fn sync_from_touched_files_skips_items_without_target_files() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("run cargo test".into(), vec![]),
            PlanItem::new("src/lib.rs: add docs".into(), vec!["src/lib.rs".into()]),
        ]);
        plan.mark_in_progress(0);

        let touched = vec!["src/lib.rs".to_string()];
        plan.sync_from_touched_files(&touched);

        // Item 0 has no target_files → stays InProgress
        assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
        // Item 1 matched → Done
        assert_eq!(plan.items[1].status, PlanItemStatus::Done);
    }

    // --- Issue #296: ExecutionPlan helper method tests ---

    #[test]
    fn execution_plan_has_blocked_items_true() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::Blocked;
        assert!(plan.has_blocked_items());
    }

    #[test]
    fn execution_plan_has_blocked_items_false() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::Done;
        assert!(!plan.has_blocked_items());
    }

    #[test]
    fn execution_plan_is_successfully_completed_all_done() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::Done;
        assert!(plan.is_successfully_completed());
    }

    #[test]
    fn execution_plan_is_successfully_completed_with_blocked() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::Blocked;
        assert!(!plan.is_successfully_completed());
    }

    #[test]
    fn execution_plan_is_successfully_completed_empty() {
        let plan = ExecutionPlan::default();
        assert!(!plan.is_successfully_completed());
    }

    #[test]
    fn execution_plan_is_successfully_completed_with_superseded() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
            PlanItem::new("task3".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::Superseded;
        plan.items[2].status = PlanItemStatus::Done;
        assert!(plan.is_successfully_completed());
    }

    #[test]
    fn execution_plan_is_successfully_completed_superseded_only() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Superseded;
        plan.items[1].status = PlanItemStatus::Superseded;
        assert!(!plan.is_successfully_completed());
    }

    // ============================================================
    // Issue #301: AlreadySatisfied status tests
    // ============================================================

    #[test]
    fn already_satisfied_display() {
        assert_eq!(
            PlanItemStatus::AlreadySatisfied.to_string(),
            "already_satisfied"
        );
    }

    #[test]
    fn already_satisfied_is_finished() {
        let mut item = PlanItem::new("x".into(), vec![]);
        item.status = PlanItemStatus::AlreadySatisfied;
        assert!(item.is_finished());
    }

    #[test]
    fn already_satisfied_format_checklist_marker() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("retired".into(), vec![])]);
        plan.items[0].status = PlanItemStatus::AlreadySatisfied;
        let checklist = plan.format_checklist();
        assert!(checklist.contains("[=]"));
    }

    #[test]
    fn already_satisfied_is_successfully_completed() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec![]),
            PlanItem::new("task2".into(), vec![]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::AlreadySatisfied;
        assert!(plan.is_successfully_completed());
    }

    #[test]
    fn already_satisfied_only_is_successfully_completed() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec![])]);
        plan.items[0].status = PlanItemStatus::AlreadySatisfied;
        assert!(plan.is_successfully_completed());
    }

    #[test]
    fn already_satisfied_allows_final_gate() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
            PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        ]);
        plan.items[0].status = PlanItemStatus::Done;
        plan.items[1].status = PlanItemStatus::AlreadySatisfied;
        assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    }

    #[test]
    fn already_satisfied_serde_roundtrip() {
        let mut item = PlanItem::new("desc".into(), vec!["src/lib.rs".into()]);
        item.status = PlanItemStatus::AlreadySatisfied;
        let json = serde_json::to_string(&item).expect("serialize");
        let back: PlanItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, PlanItemStatus::AlreadySatisfied);
    }

    // ============================================================
    // Issue #301: mark_unfinished_items_by_target tests
    // ============================================================

    #[test]
    fn mark_unfinished_items_by_target_retires_matching() {
        let mut plan = ExecutionPlan::new(vec![
            PlanItem::new("src/a.rs: update".into(), vec!["src/a.rs".into()]),
            PlanItem::new("src/b.rs: update".into(), vec!["src/b.rs".into()]),
        ]);
        plan.mark_in_progress(0);
        let retired = plan.mark_unfinished_items_by_target(
            &["src/a.rs".to_string()],
            PlanItemStatus::AlreadySatisfied,
        );
        assert_eq!(retired, vec!["src/a.rs".to_string()]);
        assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
        assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
    }

    #[test]
    fn mark_unfinished_items_by_target_skips_finished() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "src/a.rs: update".into(),
            vec!["src/a.rs".into()],
        )]);
        plan.mark_done(0);
        let retired = plan.mark_unfinished_items_by_target(
            &["src/a.rs".to_string()],
            PlanItemStatus::AlreadySatisfied,
        );
        assert!(retired.is_empty());
        assert_eq!(plan.items[0].status, PlanItemStatus::Done);
    }

    #[test]
    fn mark_unfinished_items_by_target_skips_no_target_files() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new("run tests".into(), vec![])]);
        let retired = plan.mark_unfinished_items_by_target(
            &["src/a.rs".to_string()],
            PlanItemStatus::AlreadySatisfied,
        );
        assert!(retired.is_empty());
    }

    #[test]
    fn mark_unfinished_items_by_target_multi_target_match() {
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "src/a.rs, src/b.rs: update".into(),
            vec!["src/a.rs".into(), "src/b.rs".into()],
        )]);
        let retired = plan.mark_unfinished_items_by_target(
            &["src/a.rs".to_string(), "src/b.rs".to_string()],
            PlanItemStatus::AlreadySatisfied,
        );
        assert_eq!(retired.len(), 2);
        assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
    }

    #[test]
    fn mark_unfinished_items_by_target_partial_match_rejected() {
        // DR4-001: partial match should NOT retire (target count mismatch)
        let mut plan = ExecutionPlan::new(vec![PlanItem::new(
            "src/a.rs, src/b.rs: update".into(),
            vec!["src/a.rs".into(), "src/b.rs".into()],
        )]);
        // Only one of two targets provided
        let retired = plan.mark_unfinished_items_by_target(
            &["src/a.rs".to_string()],
            PlanItemStatus::AlreadySatisfied,
        );
        assert!(retired.is_empty());
        assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
    }
}
