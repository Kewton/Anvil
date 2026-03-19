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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub cwd: PathBuf,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            dirty: false,
            cached_token_count: std::cell::Cell::new(None),
            auto_compact_threshold: 64,
        }
    }

    pub fn push_message(&mut self, message: SessionMessage) {
        // Update cached token count incrementally
        let kind = ContentKind::from_message_role(message.role);
        let mut msg_tokens = contracts_estimate_tokens(&message.content, kind);
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

    /// Run auto-compaction if message count exceeds the threshold.
    /// Called at turn boundaries (before flush) to avoid per-message overhead.
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
                let mut tokens = contracts_estimate_tokens(&message.content, kind);
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
        if self.messages.len() <= keep_recent {
            return false;
        }

        let messages_to_compact = self.messages.len() - keep_recent;
        tracing::debug!(
            compacted = messages_to_compact,
            kept = keep_recent,
            "compacting session history"
        );

        let split_at = self.messages.len() - keep_recent;
        let compacted = &self.messages[..split_at];
        let summary = summarize_messages(compacted);
        self.messages.drain(..split_at);
        self.cached_token_count.set(None); // Invalidate cache after drain
        self.messages.insert(
            0,
            SessionMessage::new(MessageRole::System, "anvil", summary)
                .with_id(format!("compact_{}", now_ms()))
                .with_status(MessageStatus::Committed),
        );
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

fn summarize_messages(messages: &[SessionMessage]) -> String {
    let mut lines = vec!["[compacted session summary]".to_string()];
    let mut references = Vec::new();
    for message in messages.iter().take(8) {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "you",
            MessageRole::Assistant => "anvil",
            MessageRole::Tool => "tool",
        };
        references.extend(extract_reference_like_tokens(&message.content));
        lines.push(format!(
            "- {}: {}",
            role,
            compact_preview(&message.content, 96)
        ));
    }
    if messages.len() > 8 {
        lines.push(format!("- ... {} more message(s)", messages.len() - 8));
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
