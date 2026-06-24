use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

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
        Self {
            role: "tool".to_string(),
            content: content.into(),
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
    pub id: String,
    pub messages: Vec<ConversationMessage>,
    #[serde(default)]
    pub native_tools_disabled: bool,
}

impl SessionSnapshot {
    pub fn new() -> Self {
        Self {
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
        serde_json::from_str(&text).context("failed to parse session")
    }

    pub fn save(&self, session: &SessionSnapshot) -> anyhow::Result<PathBuf> {
        validate_resume_id(&session.id)?;
        let dir = self.session_dir(&session.id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create session dir {}", dir.display()))?;
        let path = dir.join("session.json");
        let text = serde_json::to_string_pretty(session)?;
        std::fs::write(&path, text)
            .with_context(|| format!("failed to save {}", path.display()))?;
        Ok(path)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.root.join("sessions").join(id)
    }

    fn session_file(&self, id: &str) -> PathBuf {
        self.session_dir(id).join("session.json")
    }
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
}
