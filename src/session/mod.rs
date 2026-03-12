use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleMessageRole, ConsoleMessageView, ConsoleRenderContext,
};
use serde::{Deserialize, Serialize};
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
}

impl SessionRecord {
    pub fn new(cwd: PathBuf) -> Self {
        let now = now_ms();
        Self {
            metadata: SessionMetadata {
                session_id: session_id_for_cwd(&cwd),
                cwd,
                created_at_ms: now,
                updated_at_ms: now,
            },
            messages: Vec::new(),
            last_snapshot: None,
            session_event: None,
            event_log: Vec::new(),
        }
    }

    pub fn push_message(&mut self, message: SessionMessage) {
        self.messages.push(message);
        self.touch();
    }

    pub fn set_last_snapshot(&mut self, snapshot: AppStateSnapshot) {
        self.last_snapshot = Some(snapshot);
        self.touch();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn recent_message_views(&self, limit: usize) -> Vec<ConsoleMessageView> {
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

    pub fn console_render_context(
        &self,
        snapshot: &AppStateSnapshot,
        model_name: &str,
        visible_message_limit: usize,
    ) -> ConsoleRenderContext {
        let messages = self.recent_message_views(visible_message_limit);
        let history_summary = self.recent_history_summary(messages.len());

        ConsoleRenderContext {
            snapshot: snapshot.clone(),
            model_name: model_name.to_string(),
            messages,
            history_summary,
        }
    }

    pub fn estimated_token_count(&self) -> usize {
        self.messages
            .iter()
            .map(|message| estimate_tokens(&message.content))
            .sum()
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
    }

    fn touch(&mut self) {
        self.metadata.updated_at_ms = now_ms();
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    file_path: PathBuf,
}

impl SessionStore {
    pub fn new(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self::new(config.paths.session_file.clone())
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn load_or_create(&self, cwd: &Path) -> Result<SessionRecord, SessionError> {
        if self.file_path.exists() {
            match self.load() {
                Ok(mut record) => {
                    record.record_event(AppEvent::SessionLoaded);
                    return Ok(record);
                }
                Err(SessionError::SessionDeserializeFailed(_)) => {
                    let mut record = SessionRecord::new(cwd.to_path_buf());
                    record.record_event(AppEvent::SessionLoaded);
                    self.save(&record)?;
                    return Ok(record);
                }
                Err(err) => return Err(err),
            }
        }

        let mut record = SessionRecord::new(cwd.to_path_buf());
        record.record_event(AppEvent::SessionLoaded);
        self.save(&record)?;
        Ok(record)
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
        std::fs::write(&self.file_path, contents).map_err(SessionError::SessionWriteFailed)
    }
}

#[derive(Debug)]
pub enum SessionError {
    SessionDirectoryCreateFailed(std::io::Error),
    SessionReadFailed(std::io::Error),
    SessionWriteFailed(std::io::Error),
    SessionSerializeFailed(serde_json::Error),
    SessionDeserializeFailed(serde_json::Error),
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

fn session_id_for_cwd(cwd: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("session_{:x}", hasher.finish())
}

fn estimate_tokens(content: &str) -> usize {
    let chars = content.chars().count();
    chars.div_ceil(4).max(1)
}
