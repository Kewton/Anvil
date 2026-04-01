//! Session persistence and message history.
//!
//! [`SessionRecord`] captures the full conversation state and is serialised
//! to disk via [`SessionStore`] so that sessions survive process restarts.

use crate::agent::PendingTurnState;
use crate::config::EffectiveConfig;
use crate::contracts::tokens::{
    ContentKind, IMAGE_TOKENS, estimate_tokens as contracts_estimate_tokens,
};
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleMessageRole, ConsoleMessageView, ConsoleRenderContext,
};
use crate::provider::ProviderErrorRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Working Memory ────────────────────────────────────────────────────

/// Structured working memory that persists across compaction.
///
/// Captures the agent's current operational context so that it survives
/// history compaction and session resume.  Injected into the system prompt
/// on every turn via `build_dynamic_system_prompt()`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkingMemory {
    /// Current high-level task description (e.g., "Implement issue #130").
    #[serde(default)]
    pub active_task: Option<String>,

    /// Active constraints or invariants the agent should respect.
    #[serde(default)]
    pub constraints: Vec<String>,

    /// Recently touched file paths (relative to cwd). Capped at 20 entries.
    #[serde(default)]
    pub touched_files: Vec<String>,

    /// Unresolved errors or issues. Capped at 10 entries.
    #[serde(default)]
    pub unresolved_errors: Vec<String>,

    /// Recent diff summary. Capped at ~500 tokens.
    #[serde(default)]
    pub recent_diffs: Option<String>,

    /// Context pruning notice. Set when messages are pruned from the
    /// turn request. Cleared each turn or after compaction.
    #[serde(default)]
    pub context_notice: Option<String>,
}

impl WorkingMemory {
    /// Maximum number of tracked files.
    const MAX_TOUCHED_FILES: usize = 20;
    /// Maximum number of tracked errors.
    const MAX_UNRESOLVED_ERRORS: usize = 10;
    /// Approximate token limit for recent_diffs.
    const MAX_DIFF_TOKENS: usize = 500;

    /// Add a file path to touched_files (dedup, FIFO eviction).
    pub fn update_touched_files(&mut self, path: &str) {
        // Remove existing entry to move it to the end (most recent)
        self.touched_files.retain(|p| p != path);
        self.touched_files.push(path.to_string());
        // Evict oldest if over capacity (only one add, so at most one eviction)
        if self.touched_files.len() > Self::MAX_TOUCHED_FILES {
            self.touched_files.remove(0);
        }
    }

    /// Remove a file path from touched_files (for rollback).
    pub fn remove_touched_file(&mut self, path: &str) {
        self.touched_files.retain(|p| p != path);
    }

    /// Add an unresolved error (FIFO eviction at capacity).
    pub fn add_error(&mut self, error: impl Into<String>) {
        self.unresolved_errors.push(error.into());
        // Only one add, so at most one eviction needed
        if self.unresolved_errors.len() > Self::MAX_UNRESOLVED_ERRORS {
            self.unresolved_errors.remove(0);
        }
    }

    /// Clear a specific error by content match.
    pub fn clear_error(&mut self, error: &str) {
        self.unresolved_errors.retain(|e| e != error);
    }

    /// Clear all unresolved errors.
    pub fn clear_all_errors(&mut self) {
        self.unresolved_errors.clear();
    }

    /// Set or clear the active task.
    pub fn set_active_task(&mut self, task: Option<String>) {
        self.active_task = task;
    }

    /// Update recent_diffs, truncating to approximate token limit.
    pub fn set_recent_diffs(&mut self, diffs: Option<String>) {
        self.recent_diffs = diffs.map(|d| truncate_to_token_limit(&d, Self::MAX_DIFF_TOKENS));
    }

    /// Set or clear the context pruning notice.
    pub fn set_context_notice(&mut self, notice: Option<String>) {
        self.context_notice = notice;
    }

    /// Add a constraint string.
    pub fn add_constraint(&mut self, constraint: impl Into<String>) {
        self.constraints.push(constraint.into());
    }

    /// Returns true if all fields are empty/None.
    pub fn is_empty(&self) -> bool {
        self.active_task.is_none()
            && self.constraints.is_empty()
            && self.touched_files.is_empty()
            && self.unresolved_errors.is_empty()
            && self.recent_diffs.is_none()
            && self.context_notice.is_none()
    }

    /// Serialize working memory into a human-readable format for system prompt injection.
    /// Returns None if the working memory is empty.
    pub fn format_for_prompt(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut sections = Vec::new();
        sections.push("## Working Memory (auto-maintained)".to_string());

        if let Some(ref task) = self.active_task {
            sections.push(format!(
                "**Active task:** {}",
                sanitize_for_prompt_entry(task)
            ));
        }

        // Helper: append a labelled bullet list if non-empty
        let mut push_list = |label: &str, items: &[String]| {
            if !items.is_empty() {
                sections.push(format!("**{label}:**"));
                for item in items {
                    sections.push(format!("- {}", sanitize_for_prompt_entry(item)));
                }
            }
        };
        push_list("Constraints", &self.constraints);
        push_list("Touched files", &self.touched_files);
        push_list("Unresolved errors", &self.unresolved_errors);

        if let Some(ref diffs) = self.recent_diffs {
            sections.push(format!(
                "**Recent diffs:**\n```\n{}\n```",
                sanitize_for_prompt_entry(diffs)
            ));
        }

        if let Some(ref notice) = self.context_notice {
            sections.push(format!(
                "**Context notice:** {}",
                sanitize_for_prompt_entry(notice)
            ));
        }

        Some(sections.join("\n"))
    }
}

/// Sanitize a string before embedding it in the system prompt.
///
/// Removes protocol markers (`ANVIL_TOOL`, `ANVIL_FINAL`), triple backticks,
/// all control characters (including newline and tab to prevent prompt structure
/// injection), and truncates extremely long strings using char count for UTF-8
/// safety.
pub(crate) fn sanitize_for_prompt_entry(input: &str) -> String {
    const REMOVALS: &[&str] = &[
        "ANVIL_TOOL",
        "ANVIL_FINAL",
        "ANVIL_PLAN_UPDATE",
        "ANVIL_PLAN",
        "```",
    ];
    let mut s = input.to_string();
    for pattern in REMOVALS {
        s = s.replace(pattern, "");
    }
    // Remove control characters including newline and tab to prevent prompt structure injection
    s = s.chars().filter(|&c| !c.is_control()).collect();
    // Truncate extremely long strings (> 500 chars) — use char count for UTF-8 safety
    let char_count = s.chars().count();
    if char_count > 500 {
        s = format!("{}...[truncated]", s.chars().take(497).collect::<String>());
    }
    s
}

/// Truncate a string to approximately `max_tokens` tokens.
///
/// Uses `contracts::tokens::estimate_tokens` with `ContentKind::Text` to
/// estimate the token count and performs a binary-search-like trim by
/// character count.
fn truncate_to_token_limit(input: &str, max_tokens: usize) -> String {
    let tokens = contracts_estimate_tokens(input, ContentKind::Text);
    if tokens <= max_tokens {
        return input.to_string();
    }
    // Keep the *latest* content (tail) since new diffs are appended at the end (CB-003).
    let chars: Vec<char> = input.chars().collect();
    let ratio = max_tokens as f64 / tokens as f64;
    let target_chars = (chars.len() as f64 * ratio).floor() as usize;
    let skip = chars.len().saturating_sub(target_chars);
    let truncated: String = chars[skip..].iter().collect();
    format!("[truncated]...{truncated}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStatus {
    Committed,
    InProgress,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: String,
    pub role: MessageRole,
    pub author: String,
    pub content: String,
    pub status: MessageStatus,
    pub tool_call_id: Option<String>,
    /// Whether this tool result represents an error.
    /// Added for non-interactive mode exit code determination.
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub image_paths: Option<Vec<String>>,
    /// Expanded content after @file reference resolution.
    /// Not serialized — only exists in memory during the live session.
    #[serde(skip)]
    pub expanded_content: Option<String>,
}

impl SessionMessage {
    pub fn new(role: MessageRole, author: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            role,
            author: author.into(),
            content: content.into(),
            status: MessageStatus::Committed,
            tool_call_id: None,
            is_error: false,
            image_paths: None,
            expanded_content: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_status(mut self, status: MessageStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    pub fn with_image_paths(mut self, paths: Vec<String>) -> Self {
        self.image_paths = Some(paths);
        self
    }

    /// Return expanded_content if set, otherwise fall back to content.
    /// Used for token estimation and LLM message construction (DRY).
    pub fn effective_content(&self) -> &str {
        self.expanded_content.as_deref().unwrap_or(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub cwd: PathBuf,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub metadata: SessionMetadata,
    #[serde(default)]
    pub messages: Vec<SessionMessage>,
    #[serde(default)]
    pub last_snapshot: Option<AppStateSnapshot>,
    #[serde(default)]
    pub session_event: Option<AppEvent>,
    #[serde(default)]
    pub event_log: Vec<AppEvent>,
    #[serde(default)]
    pub pending_turn: Option<PendingTurnState>,
    #[serde(default)]
    pub provider_errors: Vec<ProviderErrorRecord>,
    /// Set of tool names that have been used in this session.
    /// Used for dynamic system prompt generation (Issue #73).
    #[serde(default)]
    pub used_tools: HashSet<String>,
    /// Structured working memory preserved across compaction.
    /// Added in Issue #130.
    #[serde(default)]
    pub working_memory: WorkingMemory,
    /// Tracks whether in-memory state has diverged from disk.
    /// Not serialized — always starts as `false` after deserialization.
    #[serde(skip)]
    pub dirty: bool,
    /// Cached estimated token count, updated incrementally on message changes.
    /// Not serialized — rebuilt lazily after deserialization.
    /// Uses `Cell` for interior mutability so callers can use `&self`.
    #[serde(skip)]
    cached_token_count: std::cell::Cell<Option<usize>>,
    /// Auto-compaction threshold (configurable, default 64).
    #[serde(skip)]
    pub auto_compact_threshold: usize,
    /// Smart compact threshold ratio (configurable, default 0.75).
    /// When estimated tokens exceed context_window * ratio, token-based compaction triggers.
    #[serde(skip)]
    pub smart_compact_threshold_ratio: f64,
}

impl SessionRecord {
    /// Create a new session with a hash-based ID derived from the working directory.
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_id(session_id_for_cwd(&cwd), cwd)
    }

    /// Create a new session with an explicit name after validation.
    pub fn new_named(name: &str, cwd: PathBuf) -> Result<Self, SessionError> {
        validate_session_name(name)?;
        Ok(Self::with_id(name.to_string(), cwd))
    }

    /// Internal constructor shared by `new` and `new_named`.
    fn with_id(session_id: String, cwd: PathBuf) -> Self {
        let now = now_ms();
        Self {
            metadata: SessionMetadata {
                session_id,
                cwd,
                created_at_ms: now,
                updated_at_ms: now,
            },
            messages: Vec::new(),
            last_snapshot: None,
            session_event: None,
            event_log: Vec::new(),
            pending_turn: None,
            provider_errors: Vec::new(),
            used_tools: HashSet::new(),
            working_memory: WorkingMemory::default(),
            dirty: false,
            cached_token_count: std::cell::Cell::new(None),
            auto_compact_threshold: 64,
            smart_compact_threshold_ratio: 0.75,
        }
    }

    pub fn push_message(&mut self, message: SessionMessage) {
        // Update cached token count incrementally
        let kind = ContentKind::from_message_role(message.role);
        let mut msg_tokens = contracts_estimate_tokens(message.effective_content(), kind);
        // Add fixed 300 tokens per image
        if let Some(ref paths) = message.image_paths {
            msg_tokens += IMAGE_TOKENS * paths.len();
        }
        if let Some(cached) = self.cached_token_count.get() {
            self.cached_token_count.set(Some(cached + msg_tokens));
        }
        self.messages.push(message);
        self.touch();
    }

    /// Check whether auto-compaction should run (DR3-001).
    /// Zero-guard: returns false if auto_compact_threshold == 0.
    pub fn should_compact(&self) -> bool {
        self.auto_compact_threshold > 0 && self.messages.len() > self.auto_compact_threshold
    }

    /// Token-based compaction check.
    /// Returns true when estimated tokens exceed effective_limit * ratio.
    /// When `context_budget` is set, uses `min(context_window, context_budget)`
    /// as the effective limit so that compact triggers within the budget.
    pub fn should_smart_compact(&self, context_window: u32, context_budget: Option<u32>) -> bool {
        let effective_limit = match context_budget {
            Some(budget) => context_window.min(budget),
            None => context_window,
        };
        self.smart_compact_threshold_ratio > 0.0
            && self.estimated_token_count()
                > (effective_limit as f64 * self.smart_compact_threshold_ratio) as usize
    }

    /// Run auto-compaction if message count exceeds the threshold.
    /// Called at turn boundaries (before flush) to avoid per-message overhead.
    #[deprecated(note = "Use App::compact_with_hooks() instead")]
    pub fn compact_if_needed(&mut self) -> bool {
        if self.auto_compact_threshold > 0 && self.messages.len() > self.auto_compact_threshold {
            self.compact_history(self.auto_compact_threshold / 2)
        } else {
            false
        }
    }

    pub fn set_last_snapshot(&mut self, snapshot: AppStateSnapshot) {
        self.last_snapshot = Some(snapshot);
        self.touch();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn recent_message_views(
        &self,
        limit: usize,
        exclude_messages: bool,
    ) -> Vec<ConsoleMessageView> {
        // When exclude_messages is true, skip ALL messages because they
        // were already shown during the live turn (streaming to stderr,
        // tool execution output, etc.).  Rendering them again in the
        // console frame would cause duplicate output (Issue #1).
        if exclude_messages {
            return Vec::new();
        }

        let len = self.messages.len();
        let start = len.saturating_sub(limit);

        self.messages[start..]
            .iter()
            .map(|message| ConsoleMessageView {
                role: match message.role {
                    MessageRole::User => ConsoleMessageRole::User,
                    MessageRole::Assistant => ConsoleMessageRole::Assistant,
                    MessageRole::Tool => ConsoleMessageRole::Tool,
                    MessageRole::System => ConsoleMessageRole::System,
                },
                content: message.content.clone(),
            })
            .collect()
    }

    pub fn recent_history_summary(&self, visible_count: usize) -> Option<String> {
        (self.messages.len() > visible_count)
            .then(|| format!("history: recent {visible_count} messages"))
    }

    pub fn render_timeline(&self, limit: usize) -> String {
        let mut lines = vec!["[A] anvil > timeline".to_string()];

        if let Some(snapshot) = &self.last_snapshot
            && let Some(plan) = &snapshot.plan
        {
            lines.push("  plan :".to_string());
            for (index, item) in plan.items.iter().enumerate() {
                let marker = if plan.active_index == Some(index) {
                    "*"
                } else {
                    "-"
                };
                lines.push(format!("    {marker} {}. {}", index + 1, item));
            }
        }

        let event_start = self.event_log.len().saturating_sub(limit);
        for event in &self.event_log[event_start..] {
            lines.push(format!("  event: {:?}", event));
        }

        let message_start = self.messages.len().saturating_sub(limit);
        for message in &self.messages[message_start..] {
            let role = match message.role {
                MessageRole::System => "system",
                MessageRole::User => "you",
                MessageRole::Assistant => "anvil",
                MessageRole::Tool => "tool",
            };
            let preview = compact_preview(&message.content, 72);
            lines.push(format!("  msg  : {role} > {preview}"));
        }

        lines.join("\n")
    }

    pub fn console_render_context(
        &self,
        snapshot: &AppStateSnapshot,
        model_name: &str,
        visible_message_limit: usize,
        exclude_messages: bool,
    ) -> ConsoleRenderContext {
        let messages = self.recent_message_views(visible_message_limit, exclude_messages);
        let history_summary = if exclude_messages {
            None
        } else {
            self.recent_history_summary(messages.len())
        };

        ConsoleRenderContext {
            snapshot: snapshot.clone(),
            model_name: model_name.to_string(),
            messages,
            history_summary,
        }
    }

    pub fn estimated_token_count(&self) -> usize {
        if let Some(cached) = self.cached_token_count.get() {
            return cached;
        }
        let count: usize = self
            .messages
            .iter()
            .map(|message| {
                let kind = ContentKind::from_message_role(message.role);
                let mut tokens = contracts_estimate_tokens(message.effective_content(), kind);
                if let Some(ref paths) = message.image_paths {
                    tokens += IMAGE_TOKENS * paths.len();
                }
                tokens
            })
            .sum();
        self.cached_token_count.set(Some(count));
        count
    }

    pub fn compact_history(&mut self, keep_recent: usize) -> bool {
        self.compact_history_impl(keep_recent, None)
    }

    /// Compact history with an optional LLM-generated summary.
    ///
    /// When `llm_summary` is `Some`, the LLM text is used as the summary body
    /// combined with extracted file targets. When `None`, falls back to the
    /// existing rule-based summarization.
    pub fn compact_history_with_llm_summary(
        &mut self,
        keep_recent: usize,
        llm_summary: Option<String>,
    ) -> bool {
        self.compact_history_impl(keep_recent, llm_summary)
    }

    /// Generate summary text for the given messages using conversation context.
    ///
    /// Delegates to [`build_conversation_text_for_summary()`].
    pub fn conversation_text_for_summary(
        &self,
        max_messages: usize,
        max_chars_per_msg: usize,
        max_total_chars: usize,
    ) -> String {
        build_conversation_text_for_summary(
            &self.messages,
            max_messages,
            max_chars_per_msg,
            max_total_chars,
        )
    }

    fn compact_history_impl(&mut self, keep_recent: usize, llm_summary: Option<String>) -> bool {
        if self.messages.len() <= keep_recent {
            return false;
        }

        let before_messages = self.messages.len();
        let split_at = before_messages - keep_recent;
        tracing::debug!(
            compacted = split_at,
            kept = keep_recent,
            before_messages = before_messages,
            after_messages = keep_recent + 1, // +1 for the summary message
            "compacting session history"
        );

        // Extract file targets from compacted messages (used for both paths)
        let file_targets = extract_file_targets(&self.messages[..split_at]);

        /// Maximum number of file references to include in LLM summary.
        const MAX_FILE_REFS_IN_SUMMARY: usize = 5;

        /// Maximum character length for LLM-generated summary text (CB-003).
        const MAX_LLM_SUMMARY_CHARS: usize = 2000;

        let summary = if let Some(llm_text) = llm_summary {
            // LLM summary path: combine LLM text with file references
            // Truncate to MAX_LLM_SUMMARY_CHARS to bound memory usage (CB-003).
            let truncated = if llm_text.len() > MAX_LLM_SUMMARY_CHARS {
                let mut end = MAX_LLM_SUMMARY_CHARS;
                // Avoid splitting a multi-byte character
                while !llm_text.is_char_boundary(end) && end > 0 {
                    end -= 1;
                }
                format!("{}...(truncated)", &llm_text[..end])
            } else {
                llm_text
            };
            let mut lines = vec!["[compacted session summary]".to_string()];
            lines.push(truncated);
            if !file_targets.is_empty() {
                lines.push("- refs:".to_string());
                for reference in file_targets.iter().take(MAX_FILE_REFS_IN_SUMMARY) {
                    lines.push(format!("  - {reference}"));
                }
            }
            lines.join("\n")
        } else {
            // Rule-based path: existing logic
            // Step 1: Replace large tool results with summaries
            replace_tool_results_with_summaries(&mut self.messages[..split_at]);

            // Step 2: Compute importance scores
            let scores = compute_importance_scores(&self.messages, split_at);

            // Step 3: Generate summary using scores
            generate_compact_summary(&self.messages[..split_at], &scores)
        };

        // Step 4: Drain old messages and insert summary
        self.messages.drain(..split_at);
        self.cached_token_count.set(None); // Invalidate cache after drain
        self.messages.insert(
            0,
            SessionMessage::new(MessageRole::System, "anvil", summary)
                .with_id(format!("compact_{}", now_ms()))
                .with_status(MessageStatus::Committed),
        );
        // Clear context_notice after compaction (Issue #157).
        // Messages have been restructured, so old pruning info is stale.
        self.working_memory.set_context_notice(None);

        self.record_event(AppEvent::SessionCompacted);
        self.touch();
        true
    }

    pub fn normalize_interrupted_turn(&mut self, interrupted_what: &str) {
        let mut normalized_count = 0usize;

        for message in self.messages.iter_mut().rev() {
            if message.status != MessageStatus::InProgress {
                break;
            }

            if message.content.trim().is_empty() {
                message.content = format!("[interrupted: {interrupted_what}]");
            }
            message.status = MessageStatus::Interrupted;
            normalized_count += 1;
        }

        if normalized_count == 0 {
            self.messages.push(
                SessionMessage::new(
                    MessageRole::Assistant,
                    "anvil",
                    format!("[interrupted: {interrupted_what}]"),
                )
                .with_id(format!("interrupt_{}", now_ms()))
                .with_status(MessageStatus::Interrupted),
            );
        }

        self.record_event(AppEvent::SessionNormalizedAfterInterrupt);

        self.touch();
    }

    pub fn record_event(&mut self, event: AppEvent) {
        self.session_event = Some(event);
        self.event_log.push(event);
        self.dirty = true;
    }

    pub fn set_pending_turn(&mut self, pending_turn: PendingTurnState) {
        self.pending_turn = Some(pending_turn);
        self.touch();
    }

    pub fn clear_pending_turn(&mut self) {
        self.pending_turn = None;
        self.touch();
    }

    pub fn has_pending_turn(&self) -> bool {
        self.pending_turn.is_some()
    }

    pub fn push_provider_error(&mut self, provider_error: ProviderErrorRecord) {
        self.provider_errors.push(provider_error);
        self.touch();
    }

    fn touch(&mut self) {
        self.metadata.updated_at_ms = now_ms();
        self.dirty = true;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Return the content of the last assistant message, if any.
    pub fn last_assistant_message(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
            .map(|m| m.content.as_str())
    }

    /// Return tool result messages from the last turn (after the last user message).
    pub fn last_turn_tool_results(&self) -> impl Iterator<Item = &SessionMessage> {
        let last_user_idx = self
            .messages
            .iter()
            .rposition(|m| m.role == MessageRole::User)
            .unwrap_or(0);

        self.messages[last_user_idx..]
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
    }
}

/// Information about a session for listing purposes.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub updated_at_ms: u128,
    pub message_count: usize,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    file_path: PathBuf,
    session_dir: PathBuf,
}

impl SessionStore {
    pub fn new(file_path: PathBuf, session_dir: PathBuf) -> Self {
        Self {
            file_path,
            session_dir,
        }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self::new(
            config.paths.session_file.clone(),
            config.paths.session_dir.clone(),
        )
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Extract the session name from the file path stem (e.g. "default" from "default.json").
    fn session_name(&self) -> &str {
        self.file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("default")
    }

    pub fn load_or_create(&self, cwd: &Path) -> Result<SessionRecord, SessionError> {
        tracing::debug!(path = %self.file_path.display(), "loading session");
        if self.file_path.exists() {
            match self.load() {
                Ok(mut record) => {
                    record.record_event(AppEvent::SessionLoaded);
                    return Ok(record);
                }
                Err(SessionError::SessionDeserializeFailed(_)) => {
                    tracing::debug!("creating new session");
                    let mut record = SessionRecord::new(cwd.to_path_buf());
                    record.record_event(AppEvent::SessionLoaded);
                    self.save(&record)?;
                    return Ok(record);
                }
                Err(err) => return Err(err),
            }
        }

        // Migration: look for old hash-based session file
        let old_key = session_id_for_cwd(cwd);
        let old_path = self.session_dir.join(format!("{old_key}.json"));
        let session_name = self.session_name();
        if old_path.exists() {
            tracing::debug!(old = %old_path.display(), new = %self.file_path.display(), "migrating old session file");
            std::fs::rename(&old_path, &self.file_path)
                .map_err(SessionError::SessionWriteFailed)?;
            let mut session = self.load()?;
            session.metadata.session_id = session_name.to_string();
            session.record_event(AppEvent::SessionLoaded);
            self.save(&session)?;
            return Ok(session);
        }

        // New session creation using named constructor
        tracing::debug!("creating new session");
        let mut record = if validate_session_name(session_name).is_ok() {
            SessionRecord::new_named(session_name, cwd.to_path_buf())?
        } else {
            // Fallback for non-conforming file names (e.g. hash-based)
            SessionRecord::new(cwd.to_path_buf())
        };
        record.record_event(AppEvent::SessionLoaded);
        self.save(&record)?;
        Ok(record)
    }

    /// List all sessions in the session directory.
    /// Skips symlinks and non-JSON files.
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, SessionError> {
        let entries = match std::fs::read_dir(&self.session_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(SessionError::SessionReadFailed(err)),
        };

        let mut sessions = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Skip symlinks
            if path.symlink_metadata().map_or(true, |m| m.is_symlink()) {
                continue;
            }

            // Only process .json files (skip .json.tmp and others)
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Try to read and deserialize the session
            let contents = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let record: SessionRecord = match serde_json::from_str(&contents) {
                Ok(r) => r,
                Err(_) => continue,
            };

            sessions.push(SessionInfo {
                name,
                updated_at_ms: record.metadata.updated_at_ms,
                message_count: record.messages.len(),
            });
        }

        Ok(sessions)
    }

    /// Delete a session by name. Validates the name and checks existence.
    pub fn delete_session(&self, name: &str) -> Result<(), SessionError> {
        validate_session_name(name)?;
        let path = self.session_dir.join(format!("{name}.json"));
        if !path.exists() {
            return Err(SessionError::SessionNotFound(name.to_string()));
        }
        std::fs::remove_file(&path).map_err(SessionError::SessionWriteFailed)?;
        Ok(())
    }

    pub fn load(&self) -> Result<SessionRecord, SessionError> {
        let contents =
            std::fs::read_to_string(&self.file_path).map_err(SessionError::SessionReadFailed)?;
        serde_json::from_str(&contents).map_err(SessionError::SessionDeserializeFailed)
    }

    pub fn save(&self, record: &SessionRecord) -> Result<(), SessionError> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(SessionError::SessionDirectoryCreateFailed)?;
        }

        let contents =
            serde_json::to_string_pretty(record).map_err(SessionError::SessionSerializeFailed)?;

        atomic_write_file(&self.file_path, contents.as_bytes())
            .map_err(SessionError::SessionWriteFailed)?;

        tracing::debug!(path = %self.file_path.display(), "session saved (atomic)");
        Ok(())
    }
}

/// Atomic file write using the tmp-file + fsync + rename pattern.
/// Ensures crash-safe writes by first writing to a temporary file,
/// fsyncing it, then atomically renaming over the target path.
fn atomic_write_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    // with_extension("json.tmp") replaces the existing .json extension with .json.tmp
    let tmp_path = path.with_extension("json.tmp");

    // Step 1: Write to temporary file + fsync
    let mut file = std::fs::File::create(&tmp_path)?;
    if let Err(err) = file.write_all(contents).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    // Step 2: Atomic rename
    if let Err(err) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    Ok(())
}

#[derive(Debug)]
pub enum SessionError {
    SessionDirectoryCreateFailed(std::io::Error),
    SessionReadFailed(std::io::Error),
    SessionWriteFailed(std::io::Error),
    SessionSerializeFailed(serde_json::Error),
    SessionDeserializeFailed(serde_json::Error),
    InvalidSessionName(String),
    SessionNotFound(String),
    ActiveSessionCannotBeDeleted,
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionDirectoryCreateFailed(err) => {
                write!(f, "failed to create session directory: {err}")
            }
            Self::SessionReadFailed(err) => write!(f, "failed to read session: {err}"),
            Self::SessionWriteFailed(err) => write!(f, "failed to write session: {err}"),
            Self::SessionSerializeFailed(err) => {
                write!(f, "failed to serialize session: {err}")
            }
            Self::SessionDeserializeFailed(err) => {
                write!(f, "failed to deserialize session: {err}")
            }
            Self::InvalidSessionName(name) => {
                write!(
                    f,
                    "invalid session name: '{name}' (allowed: alphanumeric, hyphen, underscore, 1-64 chars)"
                )
            }
            Self::SessionNotFound(name) => {
                write!(f, "session not found: '{name}'")
            }
            Self::ActiveSessionCannotBeDeleted => {
                write!(f, "cannot delete the active session")
            }
        }
    }
}

impl std::error::Error for SessionError {}

pub fn new_user_message(id: impl Into<String>, content: impl Into<String>) -> SessionMessage {
    SessionMessage::new(MessageRole::User, "you", content).with_id(id)
}

pub fn new_assistant_message(
    id: impl Into<String>,
    content: impl Into<String>,
    status: MessageStatus,
) -> SessionMessage {
    SessionMessage::new(MessageRole::Assistant, "anvil", content)
        .with_id(id)
        .with_status(status)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn session_id_for_cwd(cwd: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("session_{:x}", hasher.finish())
}

/// Validate a session name: alphanumeric, hyphen, underscore only, 1-64 chars.
/// Rejects `.`, `..`, NUL bytes, and any other special characters.
pub fn validate_session_name(name: &str) -> Result<(), SessionError> {
    // The alphanumeric + hyphen + underscore check below already rejects
    // '.', '..', NUL bytes, and all other special characters, so no
    // separate guards are needed.
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        return Err(SessionError::InvalidSessionName(name.to_string()));
    }
    Ok(())
}

// ── Smart compact constants and functions ─────────────────────────────

/// Importance score constants (Issue #80).
const SCORE_SYSTEM_PROMPT: i32 = 10;
const SCORE_ERROR_BONUS: i32 = 5;
const SCORE_USER_INPUT: i32 = 3;
const SCORE_TOOL_RESULT_PENALTY: i32 = -2;
const SCORE_RECENCY_MAX: i32 = 10;

/// Compaction target: after compaction, aim for context_window * TARGET_TOKEN_RATIO tokens.
pub(crate) const TARGET_TOKEN_RATIO: f64 = 0.5;

/// Tool results above this estimated token count are replaced with summaries.
const TOOL_SUMMARY_THRESHOLD: usize = 500;

/// Sensitive keyword patterns for bash command masking.
const SENSITIVE_KEYWORDS: &[&str] = &[
    "authorization",
    "bearer",
    "api_key",
    "password",
    "token",
    "secret",
];

/// Compute importance scores for messages in the compaction range.
/// Returns a Vec<i32> with one score per message in messages[..compact_end].
pub fn compute_importance_scores(messages: &[SessionMessage], compact_end: usize) -> Vec<i32> {
    let mut scores = vec![0i32; compact_end];
    for (index, msg) in messages[..compact_end].iter().enumerate() {
        let mut score: i32 = 0;
        // Recency: linear interpolation within compact range
        if compact_end > 1 {
            score += (SCORE_RECENCY_MAX * index as i32) / (compact_end as i32 - 1);
        }
        if msg.is_error {
            score += SCORE_ERROR_BONUS;
        }
        match msg.role {
            MessageRole::User => score += SCORE_USER_INPUT,
            MessageRole::Tool => score += SCORE_TOOL_RESULT_PENALTY,
            MessageRole::System => score += SCORE_SYSTEM_PROMPT,
            MessageRole::Assistant => {}
        }
        scores[index] = score;
    }
    scores
}

/// Compute how many messages to keep from the end to fit within target_tokens.
pub fn compute_token_based_keep_recent(messages: &[SessionMessage], target_tokens: usize) -> usize {
    let mut cumulative = 0usize;
    let mut keep = 0usize;
    for msg in messages.iter().rev() {
        let kind = ContentKind::from_message_role(msg.role);
        cumulative += contracts_estimate_tokens(msg.effective_content(), kind);
        keep += 1;
        if cumulative >= target_tokens {
            break;
        }
    }
    keep
}

/// Generate a summary template for a tool result message.
/// Returns None if the message is not a Tool role or below the threshold.
pub fn summarize_tool_result(msg: &SessionMessage) -> Option<String> {
    if msg.role != MessageRole::Tool {
        return None;
    }
    let kind = ContentKind::from_message_role(msg.role);
    let tokens = contracts_estimate_tokens(msg.effective_content(), kind);
    if tokens < TOOL_SUMMARY_THRESHOLD {
        return None;
    }

    let tool_name = &msg.author;
    let sanitized_name = if !tool_name.is_empty()
        && tool_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        tool_name.as_str()
    } else {
        "unknown_tool"
    };
    Some(format!("[要約] {sanitized_name}: ({tokens}トークンの結果)"))
}

/// Replace tool results exceeding the threshold with summary templates.
pub fn replace_tool_results_with_summaries(messages: &mut [SessionMessage]) {
    for msg in messages.iter_mut() {
        if let Some(summary) = summarize_tool_result(msg) {
            msg.content = summary;
        }
    }
}

/// Mask sensitive patterns in a bash command string.
pub fn mask_sensitive_in_command(cmd: &str) -> String {
    let mut result = cmd.to_string();
    // Mask known secret prefixes
    for prefix in crate::config::KNOWN_SECRET_PREFIXES {
        if let Some(pos) = result.find(prefix) {
            // Find the end of the token (whitespace or end of string)
            let start = pos;
            let end = result[pos..]
                .find(char::is_whitespace)
                .map(|i| pos + i)
                .unwrap_or(result.len());
            result.replace_range(start..end, "***");
        }
    }
    // Mask sensitive keyword values
    let lower = result.to_ascii_lowercase();
    for keyword in SENSITIVE_KEYWORDS {
        if let Some(kw_pos) = lower.find(keyword) {
            // Find the value after the keyword (skip keyword + optional separator)
            let after_kw = kw_pos + keyword.len();
            if after_kw < result.len() {
                // Skip separators like '=', ':', ' '
                let value_start = result[after_kw..]
                    .find(|c: char| !matches!(c, '=' | ':' | ' ' | '"' | '\''))
                    .map(|i| after_kw + i);
                if let Some(vs) = value_start {
                    let value_end = result[vs..]
                        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                        .map(|i| vs + i)
                        .unwrap_or(result.len());
                    if value_end > vs {
                        result.replace_range(vs..value_end, "***");
                    }
                }
            }
        }
    }
    result
}

/// Convert an absolute path to a relative path based on cwd.
pub fn to_relative_path(absolute_path: &str, cwd: &str) -> String {
    absolute_path
        .strip_prefix(cwd)
        .map(|p| p.trim_start_matches('/').to_string())
        .unwrap_or_else(|| absolute_path.to_string())
}

/// Generate a compact summary from messages and their importance scores.
/// High-score messages get more detail; low-score ones are condensed.
pub(crate) fn generate_compact_summary(messages: &[SessionMessage], scores: &[i32]) -> String {
    let mut lines = vec!["[compacted session summary]".to_string()];
    let mut references = Vec::new();

    // Determine the score threshold for "high importance"
    let max_score = scores.iter().copied().max().unwrap_or(0);
    let high_threshold = max_score / 2;

    for (index, msg) in messages.iter().enumerate() {
        let score = scores.get(index).copied().unwrap_or(0);
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "you",
            MessageRole::Assistant => "anvil",
            MessageRole::Tool => "tool",
        };
        references.extend(extract_reference_like_tokens(&msg.content));

        if score >= high_threshold {
            // High importance: include more detail
            lines.push(format!(
                "- {}: {}",
                role,
                compact_preview(&msg.content, 120)
            ));
        } else if index < 12 {
            // Low importance but within first 12: brief summary
            lines.push(format!("- {}: {}", role, compact_preview(&msg.content, 60)));
        }
        // Beyond 12 low-importance messages: omitted
    }

    let omitted = messages
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let score = scores.get(*i).copied().unwrap_or(0);
            score < high_threshold && *i >= 12
        })
        .count();
    if omitted > 0 {
        lines.push(format!("- ... {omitted} more message(s)"));
    }

    references.sort();
    references.dedup();
    if !references.is_empty() {
        lines.push("- refs:".to_string());
        for reference in references.into_iter().take(5) {
            lines.push(format!("  - {reference}"));
        }
    }
    lines.join("\n")
}

fn compact_preview(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim().replace('\n', " ");
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= max_chars {
        trimmed
    } else {
        let preview: String = chars[..max_chars.saturating_sub(3)].iter().collect();
        format!("{preview}...")
    }
}

/// Build a plain-text representation of session messages for LLM summarization.
///
/// Applies three limits to keep output bounded:
/// - `max_messages`: only the most recent N messages are included
/// - `max_chars_per_msg`: each message content is truncated (CJK-safe via `chars().take()`)
/// - `max_total_chars`: the total output is capped at this character count
pub fn build_conversation_text_for_summary(
    messages: &[SessionMessage],
    max_messages: usize,
    max_chars_per_msg: usize,
    max_total_chars: usize,
) -> String {
    let start = messages.len().saturating_sub(max_messages);
    let mut result = String::new();
    for msg in &messages[start..] {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let truncated: String = msg.content.chars().take(max_chars_per_msg).collect();
        let line = format!("{role}: {truncated}\n");
        if result.chars().count() + line.chars().count() > max_total_chars {
            break;
        }
        result.push_str(&line);
    }
    result
}

/// Extract file-path-like references from compact target messages.
///
/// Uses `extract_reference_like_tokens()` on each message to find path-like
/// tokens. Deduplicates and sorts the results.
pub fn extract_file_targets(messages: &[SessionMessage]) -> Vec<String> {
    let mut refs = Vec::new();
    for msg in messages {
        refs.extend(extract_reference_like_tokens(&msg.content));
    }
    refs.sort();
    refs.dedup();
    refs
}

fn extract_reference_like_tokens(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .map(|token| token.trim_matches(|char: char| ",:;()[]{}<>\"'`".contains(char)))
        .filter(|token| token.contains('/') || token.contains('.'))
        .filter(|token| token.len() > 2)
        .filter(|token| !is_noise_token(token))
        .map(|token| token.to_string())
        .collect()
}

/// Reject tokens that look like prose punctuation rather than file paths
/// or code references.
fn is_noise_token(token: &str) -> bool {
    // Reject tokens that are just a trailing period on a word (e.g. "sentence.")
    if token.ends_with('.') && !token[..token.len() - 1].contains('.') {
        return true;
    }
    // Reject very short tokens that are likely abbreviations (e.g. "e.g.")
    if token.len() <= 4 && token.chars().filter(|&c| c == '.').count() >= 2 {
        return true;
    }
    false
}

// ── Session Notes (Issue #241) ───────────────────────────────────────

/// Category of a session note — represents tool operation types only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    FileEdit,
    FileRead,
    ShellExec,
    ErrorHit,
}

impl std::fmt::Display for NoteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoteKind::FileEdit => write!(f, "file_edit"),
            NoteKind::FileRead => write!(f, "file_read"),
            NoteKind::ShellExec => write!(f, "shell_exec"),
            NoteKind::ErrorHit => write!(f, "error_hit"),
        }
    }
}

/// A deterministic note extracted from a turn's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNote {
    pub kind: NoteKind,
    pub files: Vec<String>,
    pub summary: String,
}

/// Truncate a string at the given byte limit, respecting UTF-8 char boundaries.
/// Appends "..." when truncation occurs.
fn truncate_at_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Extract session notes from a slice of messages.
///
/// Scans Tool-role messages, classifies by author, aggregates by kind,
/// and produces summary strings. Unknown authors are ignored.
pub fn extract_session_notes(messages: &[SessionMessage]) -> Vec<SessionNote> {
    use std::collections::HashMap;

    struct Accumulator {
        files: Vec<String>,
        count: usize,
    }

    impl Accumulator {
        fn new() -> Self {
            Self {
                files: Vec::new(),
                count: 0,
            }
        }

        /// Merge unique file paths and bump the operation count.
        fn record(&mut self, paths: &[String]) {
            for f in paths {
                if !self.files.contains(f) {
                    self.files.push(f.clone());
                }
            }
            self.count += 1;
        }
    }

    let mut accumulators: HashMap<NoteKind, Accumulator> = HashMap::new();

    for msg in messages {
        if msg.role != MessageRole::Tool {
            continue;
        }

        let kind = match msg.author.as_str() {
            "file.edit" | "file.write" => Some(NoteKind::FileEdit),
            "file.read" => Some(NoteKind::FileRead),
            "shell.exec" => Some(NoteKind::ShellExec),
            _ => None,
        };

        // Extract file paths from this single message
        let file_targets = extract_file_targets(std::slice::from_ref(msg));
        // Sanitize: strip control chars, limit path length (128 chars), limit count (20 files)
        let sanitized: Vec<String> = file_targets
            .into_iter()
            .take(20)
            .map(|f| {
                let clean: String = f.chars().filter(|c| !c.is_control()).collect();
                truncate_at_boundary(&clean, 128)
            })
            .collect();

        // If is_error, always create an ErrorHit entry (regardless of known/unknown author)
        if msg.is_error {
            accumulators
                .entry(NoteKind::ErrorHit)
                .or_insert_with(Accumulator::new)
                .record(&sanitized);
        }

        // For known tools, also create the tool-type note
        let Some(k) = kind else { continue };

        accumulators
            .entry(k)
            .or_insert_with(Accumulator::new)
            .record(&sanitized);
    }

    // Build notes in a stable order
    let order = [
        NoteKind::FileEdit,
        NoteKind::FileRead,
        NoteKind::ShellExec,
        NoteKind::ErrorHit,
    ];

    let mut notes = Vec::new();
    for kind in order {
        if let Some(acc) = accumulators.remove(&kind) {
            let verb = match kind {
                NoteKind::FileEdit => "Edited",
                NoteKind::FileRead => "Read",
                NoteKind::ShellExec => "Executed shell command",
                NoteKind::ErrorHit => "Error in",
            };

            let summary = if acc.files.is_empty() {
                format!("{verb} {count} operation(s)", count = acc.count)
            } else {
                let file_list = acc.files.join(", ");
                format!(
                    "{verb} {count} file(s): {file_list}",
                    count = acc.files.len()
                )
            };

            let summary = truncate_at_boundary(&summary, 200);

            notes.push(SessionNote {
                kind,
                files: acc.files,
                summary,
            });
        }
    }

    notes
}

#[cfg(test)]
mod working_memory_tests {
    use super::*;

    #[test]
    fn default_all_fields_empty() {
        let wm = WorkingMemory::default();
        assert!(wm.active_task.is_none());
        assert!(wm.constraints.is_empty());
        assert!(wm.touched_files.is_empty());
        assert!(wm.unresolved_errors.is_empty());
        assert!(wm.recent_diffs.is_none());
        assert!(wm.is_empty());
    }

    #[test]
    fn touched_files_fifo_eviction_at_21() {
        let mut wm = WorkingMemory::default();
        for i in 0..21 {
            wm.update_touched_files(&format!("file_{i}.rs"));
        }
        assert_eq!(wm.touched_files.len(), 20);
        // oldest (file_0.rs) should have been evicted
        assert!(!wm.touched_files.contains(&"file_0.rs".to_string()));
        assert!(wm.touched_files.contains(&"file_20.rs".to_string()));
        assert_eq!(wm.touched_files[0], "file_1.rs");
    }

    #[test]
    fn touched_files_dedup_moves_to_end() {
        let mut wm = WorkingMemory::default();
        wm.update_touched_files("a.rs");
        wm.update_touched_files("b.rs");
        wm.update_touched_files("a.rs");
        assert_eq!(wm.touched_files.len(), 2);
        assert_eq!(wm.touched_files[0], "b.rs");
        assert_eq!(wm.touched_files[1], "a.rs");
    }

    #[test]
    fn unresolved_errors_fifo_eviction() {
        let mut wm = WorkingMemory::default();
        for i in 0..11 {
            wm.add_error(format!("error_{i}"));
        }
        assert_eq!(wm.unresolved_errors.len(), 10);
        assert!(!wm.unresolved_errors.contains(&"error_0".to_string()));
        assert!(wm.unresolved_errors.contains(&"error_10".to_string()));
    }

    #[test]
    fn remove_touched_file() {
        let mut wm = WorkingMemory::default();
        wm.update_touched_files("a.rs");
        wm.update_touched_files("b.rs");
        wm.remove_touched_file("a.rs");
        assert_eq!(wm.touched_files, vec!["b.rs".to_string()]);
    }

    #[test]
    fn format_for_prompt_empty_returns_none() {
        let wm = WorkingMemory::default();
        assert!(wm.format_for_prompt().is_none());
    }

    #[test]
    fn format_for_prompt_all_fields() {
        let mut wm = WorkingMemory::default();
        wm.set_active_task(Some("implement #130".to_string()));
        wm.add_constraint("no unsafe");
        wm.update_touched_files("src/main.rs");
        wm.add_error("file.edit: old_string not found");
        wm.set_recent_diffs(Some("- old\n+ new".to_string()));

        let prompt = wm.format_for_prompt().expect("should produce prompt");
        assert!(prompt.contains("## Working Memory (auto-maintained)"));
        assert!(prompt.contains("**Active task:** implement #130"));
        assert!(prompt.contains("**Constraints:**"));
        assert!(prompt.contains("- no unsafe"));
        assert!(prompt.contains("**Touched files:**"));
        assert!(prompt.contains("- src/main.rs"));
        assert!(prompt.contains("**Unresolved errors:**"));
        assert!(prompt.contains("- file.edit: old_string not found"));
        assert!(prompt.contains("**Recent diffs:**"));
        // Newlines are sanitized, so check for flattened content
        assert!(prompt.contains("- old+ new"));
    }

    #[test]
    fn set_recent_diffs_truncation() {
        let mut wm = WorkingMemory::default();
        // Create a very long diff string (~5000 chars => many tokens)
        let long_diff = "a".repeat(5000);
        wm.set_recent_diffs(Some(long_diff.clone()));
        let diffs = wm.recent_diffs.as_ref().expect("should have diffs");
        // Should be truncated (shorter than original)
        assert!(diffs.len() < long_diff.len());
        assert!(diffs.starts_with("[truncated]..."));
    }

    #[test]
    fn sanitize_for_prompt_entry_removes_markers() {
        let input = "ANVIL_TOOL some text ANVIL_FINAL";
        let sanitized = sanitize_for_prompt_entry(input);
        assert!(!sanitized.contains("ANVIL_TOOL"));
        assert!(!sanitized.contains("ANVIL_FINAL"));
        assert!(sanitized.contains("some text"));
    }

    #[test]
    fn sanitize_for_prompt_entry_removes_triple_backticks() {
        let input = "```rust\nfn main() {}\n```";
        let sanitized = sanitize_for_prompt_entry(input);
        assert!(!sanitized.contains("```"));
        assert!(sanitized.contains("fn main()"));
    }

    #[test]
    fn sanitize_for_prompt_entry_removes_control_chars() {
        let input = "hello\x01\x02world\ttab\nnewline";
        let sanitized = sanitize_for_prompt_entry(input);
        assert!(!sanitized.contains('\x01'));
        assert!(!sanitized.contains('\x02'));
        // Newlines and tabs are also removed to prevent prompt structure injection
        assert!(!sanitized.contains('\t'));
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.contains("helloworld"));
        assert!(sanitized.contains("tabnewline"));
    }

    #[test]
    fn sanitize_for_prompt_entry_truncates_long_lines() {
        let long_line = "x".repeat(600);
        let sanitized = sanitize_for_prompt_entry(&long_line);
        assert!(sanitized.len() < 600);
        assert!(sanitized.contains("...[truncated]"));
    }

    #[test]
    fn format_for_prompt_sanitizes_fields() {
        let mut wm = WorkingMemory::default();
        wm.set_active_task(Some("task ANVIL_TOOL inject".to_string()));
        let prompt = wm.format_for_prompt().expect("should produce prompt");
        assert!(!prompt.contains("ANVIL_TOOL"));
        assert!(prompt.contains("task  inject"));
    }

    #[test]
    fn sanitize_removes_newlines_and_tabs() {
        let input = "line1\nline2\ttab";
        let result = sanitize_for_prompt_entry(input);
        assert!(!result.contains('\n'));
        assert!(!result.contains('\t'));
        assert_eq!(result, "line1line2tab");
    }

    #[test]
    fn sanitize_utf8_safe_truncation() {
        // Create a string of 600 Japanese characters (each 3 bytes in UTF-8)
        let long_jp: String = "あ".repeat(600);
        let result = sanitize_for_prompt_entry(&long_jp);
        // Should not panic and should be truncated
        assert!(result.contains("...[truncated]"));
        // Should have 497 chars + "...[truncated]" suffix
        let prefix: String = result.strip_suffix("...[truncated]").unwrap().to_string();
        assert_eq!(prefix.chars().count(), 497);
    }

    #[test]
    fn sanitize_newline_in_file_path_prevented() {
        let mut wm = WorkingMemory::default();
        wm.update_touched_files("src/foo.rs\n## Injected Section\n- malicious");
        let prompt = wm.format_for_prompt().expect("should produce prompt");
        // Newlines are stripped so injected content cannot start a new line
        // The ## would be inline, not at start of a line as a heading
        assert!(!prompt.contains("\n## Injected Section"));
        // The sanitized version is flattened into a single line
        assert!(prompt.contains("src/foo.rs## Injected Section- malicious"));
    }

    // ── Issue #157: context_notice tests ──────────────────────────────

    #[test]
    fn context_notice_default_is_none() {
        let wm = WorkingMemory::default();
        assert!(wm.context_notice.is_none());
    }

    #[test]
    fn context_notice_set_and_clear() {
        let mut wm = WorkingMemory::default();
        wm.set_context_notice(Some("5 messages pruned".to_string()));
        assert_eq!(wm.context_notice.as_deref(), Some("5 messages pruned"));
        wm.set_context_notice(None);
        assert!(wm.context_notice.is_none());
    }

    #[test]
    fn is_empty_false_when_context_notice_set() {
        let mut wm = WorkingMemory::default();
        assert!(wm.is_empty());
        wm.set_context_notice(Some("notice".to_string()));
        assert!(!wm.is_empty());
    }

    #[test]
    fn format_for_prompt_with_context_notice() {
        let mut wm = WorkingMemory::default();
        wm.set_context_notice(Some("3 earlier messages were omitted".to_string()));
        let prompt = wm.format_for_prompt().expect("should produce prompt");
        assert!(prompt.contains("**Context notice:** 3 earlier messages were omitted"));
    }

    #[test]
    fn context_notice_serialize_roundtrip() {
        let mut wm = WorkingMemory::default();
        wm.set_context_notice(Some("10 messages pruned".to_string()));
        let json = serde_json::to_string(&wm).expect("serialize");
        let restored: WorkingMemory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(wm.context_notice, restored.context_notice);
    }

    #[test]
    fn context_notice_backward_compat_missing_field() {
        // JSON without context_notice field should deserialize to None
        let json = r#"{"active_task":null,"constraints":[],"touched_files":[],"unresolved_errors":[],"recent_diffs":null}"#;
        let wm: WorkingMemory = serde_json::from_str(json).expect("deserialize");
        assert!(wm.context_notice.is_none());
    }

    #[test]
    fn format_for_prompt_sanitizes_context_notice() {
        let mut wm = WorkingMemory::default();
        wm.set_context_notice(Some("notice ANVIL_TOOL inject".to_string()));
        let prompt = wm.format_for_prompt().expect("should produce prompt");
        assert!(!prompt.contains("ANVIL_TOOL"));
        assert!(prompt.contains("notice  inject"));
    }
}
