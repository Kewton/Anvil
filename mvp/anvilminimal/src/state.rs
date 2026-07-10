use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const SESSION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: Uuid::now_v7().to_string(),
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ConversationMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls,
        }
    }

    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool_result(name, None::<String>, content)
    }

    pub fn tool_result(
        name: impl Into<String>,
        tool_call_id: Option<impl Into<String>>,
        content: impl Into<String>,
    ) -> Self {
        let content = crate::eval_events::body_snippet_whole_tokens(&content.into());
        Self {
            role: "tool".to_string(),
            content,
            name: Some(name.into()),
            tool_call_id: tool_call_id.map(Into::into),
            tool_calls: Vec::new(),
        }
    }

    fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default = "default_session_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub messages: Vec<ConversationMessage>,
    #[serde(default)]
    pub native_tools_disabled: bool,
}

impl SessionSnapshot {
    pub fn new() -> Self {
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            id: Uuid::now_v7().to_string(),
            messages: Vec::new(),
            native_tools_disabled: false,
        }
    }
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load_or_create(&self, resume: Option<&str>) -> anyhow::Result<SessionSnapshot> {
        match resume {
            Some(id) => self.load(id),
            None => Ok(SessionSnapshot::new()),
        }
    }

    pub fn load(&self, id: &str) -> anyhow::Result<SessionSnapshot> {
        validate_resume_id(id)?;
        let path = self.session_file(id);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read session {}", path.display()))?;
        let value: Value = serde_json::from_str(&text).context("failed to parse session")?;
        validate_session_schema_version(&value)?;
        serde_json::from_value(value).context("failed to parse session")
    }

    pub fn save(&self, session: &SessionSnapshot) -> anyhow::Result<PathBuf> {
        validate_resume_id(&session.id)?;
        let dir = self.session_dir(&session.id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create session dir {}", dir.display()))?;
        let path = dir.join("session.json");
        let text = serde_json::to_string_pretty(session)?;
        atomic_write_session(&dir, &path, text.as_bytes())?;
        Ok(path)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.root.join("sessions").join(id)
    }

    fn session_file(&self, id: &str) -> PathBuf {
        self.session_dir(id).join("session.json")
    }
}

fn default_session_schema_version() -> u32 {
    SESSION_SCHEMA_VERSION
}

fn validate_session_schema_version(value: &Value) -> anyhow::Result<()> {
    let Some(raw_version) = value.get("schema_version") else {
        return Ok(());
    };
    let Some(version) = raw_version.as_u64() else {
        bail!("unsupported session schema_version: expected integer {SESSION_SCHEMA_VERSION}");
    };
    if version != u64::from(SESSION_SCHEMA_VERSION) {
        bail!("unsupported session schema_version {version}; expected {SESSION_SCHEMA_VERSION}");
    }
    Ok(())
}

fn atomic_write_session(dir: &Path, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = dir.join(format!(
        "session.json.tmp-{}-{}",
        std::process::id(),
        Uuid::now_v7()
    ));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("failed to create temp session {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write temp session {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync temp session {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "failed to atomically replace session {} from {}",
                path.display(),
                tmp.display()
            )
        })?;
        fsync_dir(dir)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

fn fsync_dir(dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)
            .with_context(|| format!("failed to open session dir {}", dir.display()))?
            .sync_all()
            .with_context(|| format!("failed to fsync session dir {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

fn validate_resume_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        || id.contains("..")
    {
        bail!("invalid resume id");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_id_cannot_escape_state_root() {
        assert!(validate_resume_id("../x").is_err());
        assert!(validate_resume_id("/tmp/x").is_err());
        assert!(validate_resume_id("abc-123").is_ok());
    }

    #[test]
    fn state_root_is_anvilminimal() {
        let root = crate::config::default_state_dir();
        assert!(root.ends_with("anvilminimal"));
    }

    #[test]
    fn session_save_writes_schema_version_and_loads_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut session = SessionSnapshot::new();
        session.messages.push(ConversationMessage::user("hello"));

        let path = store.save(&session).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"schema_version\": 1"), "{text}");

        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.schema_version, SESSION_SCHEMA_VERSION);
        assert_eq!(loaded.messages, session.messages);
    }

    #[test]
    fn session_load_rejects_unknown_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let session = SessionSnapshot::new();
        let session_dir = dir.path().join("sessions").join(&session.id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("session.json"),
            format!(
                r#"{{"schema_version":999,"id":"{}","messages":[],"native_tools_disabled":false}}"#,
                session.id
            ),
        )
        .unwrap();

        let err = store.load(&session.id).unwrap_err().to_string();
        assert!(
            err.contains("unsupported session schema_version 999"),
            "{err}"
        );
    }
}
