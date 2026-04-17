use std::fs;
use std::path::PathBuf;

use crate::modes::plan_act::ModeState;
use crate::ollama::xml_fallback::ToolCall;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ConversationMessage {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
            tool_calls: Vec::new(),
            name: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            tool_calls: Vec::new(),
            name: None,
        }
    }

    pub fn assistant(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            tool_calls,
            name: None,
        }
    }

    pub fn tool(name: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content,
            tool_calls: Vec::new(),
            name: Some(name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct SessionSnapshot {
    pub mode_state: ModeState,
    pub messages: Vec<ConversationMessage>,
    pub checkpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_root: Option<PathBuf>,
    #[serde(default)]
    pub native_tools_disabled: bool,
}

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load_or_new(&self, fresh: bool) -> Result<SessionSnapshot, String> {
        if fresh || !self.path.exists() {
            return Ok(SessionSnapshot::default());
        }

        let contents = fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read session {}: {err}", self.path.display()))?;
        serde_json::from_str(&contents)
            .map_err(|err| format!("failed to parse session {}: {err}", self.path.display()))
    }

    pub fn save(&self, session: &SessionSnapshot) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        let contents = serde_json::to_string_pretty(session)
            .map_err(|err| format!("failed to serialize session: {err}"))?;
        fs::write(&self.path, contents)
            .map_err(|err| format!("failed to write session {}: {err}", self.path.display()))
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}
