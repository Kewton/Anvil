use std::fs;
use std::path::{Path, PathBuf};

use crate::modes::plan_act::{ExecutionMode, ModeState};
use crate::ollama::xml_fallback::ToolCall;
use crate::session::feedback::FeedbackFrame;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct WorkingMemory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_task: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_errors: Vec<String>,
}

impl WorkingMemory {
    const MAX_CONSTRAINTS: usize = 8;
    const MAX_TOUCHED_FILES: usize = 12;
    const MAX_UNRESOLVED_ERRORS: usize = 8;

    pub fn set_active_task(&mut self, task: Option<String>) {
        self.active_task = task.map(|value| truncate_entry(value, 240));
    }

    pub fn replace_constraints(&mut self, constraints: Vec<String>) {
        self.constraints = constraints
            .into_iter()
            .map(|entry| truncate_entry(entry, 180))
            .take(Self::MAX_CONSTRAINTS)
            .collect();
    }

    pub fn note_touched_file(&mut self, path: String) {
        let path = truncate_entry(path, 180);
        self.touched_files.retain(|entry| entry != &path);
        self.touched_files.push(path);
        while self.touched_files.len() > Self::MAX_TOUCHED_FILES {
            self.touched_files.remove(0);
        }
    }

    pub fn note_error(&mut self, error: String) {
        let error = truncate_entry(error, 220);
        self.unresolved_errors.retain(|entry| entry != &error);
        self.unresolved_errors.push(error);
        while self.unresolved_errors.len() > Self::MAX_UNRESOLVED_ERRORS {
            self.unresolved_errors.remove(0);
        }
    }

    pub fn format_for_prompt(&self) -> Option<String> {
        if self.active_task.is_none()
            && self.constraints.is_empty()
            && self.touched_files.is_empty()
            && self.unresolved_errors.is_empty()
        {
            return None;
        }

        let mut lines = vec!["[Working Memory]".to_string()];
        if let Some(task) = &self.active_task {
            lines.push(format!("Active task: {task}"));
        }
        if !self.constraints.is_empty() {
            lines.push("Constraints:".to_string());
            for item in &self.constraints {
                lines.push(format!("- {item}"));
            }
        }
        if !self.touched_files.is_empty() {
            lines.push("Touched files:".to_string());
            for item in &self.touched_files {
                lines.push(format!("- {item}"));
            }
        }
        if !self.unresolved_errors.is_empty() {
            lines.push("Unresolved errors:".to_string());
            for item in &self.unresolved_errors {
                lines.push(format!("- {item}"));
            }
        }
        Some(lines.join("\n"))
    }
}

fn truncate_entry(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let truncated = value.chars().take(max_chars).collect::<String>();
    format!("{truncated}...")
}

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
    #[serde(default)]
    pub working_memory: WorkingMemory,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub workspace_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_feedback: Option<FeedbackFrame>,
}

impl SessionSnapshot {
    /// Overwrite `last_feedback` with the given frame. The same-turn
    /// override rule (design 5.5) is satisfied by callers that invoke
    /// this once per detected feedback event; later calls win.
    pub fn record_feedback(&mut self, frame: FeedbackFrame) {
        self.last_feedback = Some(frame);
    }
}

pub struct SessionStore {
    path: PathBuf,
    state_root: PathBuf,
    session_id: String,
    workspace_key: String,
}

impl SessionStore {
    pub fn new(state_root: &Path, session_id: &str, workspace_key: &str) -> Self {
        let path = state_root
            .join("sessions")
            .join(session_id)
            .join("session.json");
        Self {
            path,
            state_root: state_root.to_path_buf(),
            session_id: session_id.to_string(),
            workspace_key: workspace_key.to_string(),
        }
    }

    pub fn load_or_new(&self, fresh: bool) -> Result<SessionSnapshot, String> {
        if fresh || !self.path.exists() {
            return Ok(SessionSnapshot {
                id: self.session_id.clone(),
                workspace_key: self.workspace_key.clone(),
                ..SessionSnapshot::default()
            });
        }

        let contents = fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read session {}: {err}", self.path.display()))?;
        let mut snapshot: SessionSnapshot = serde_json::from_str(&contents)
            .map_err(|err| format!("failed to parse session {}: {err}", self.path.display()))?;
        if snapshot.id.is_empty() {
            snapshot.id = self.session_id.clone();
        }
        if snapshot.workspace_key.is_empty() {
            snapshot.workspace_key = self.workspace_key.clone();
        }
        Ok(snapshot)
    }

    pub fn save(&self, session: &SessionSnapshot) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        let mut snapshot = session.clone();
        if snapshot.id.is_empty() {
            snapshot.id = self.session_id.clone();
        }
        if snapshot.workspace_key.is_empty() {
            snapshot.workspace_key = self.workspace_key.clone();
        }
        let contents = serde_json::to_string_pretty(&snapshot)
            .map_err(|err| format!("failed to serialize session: {err}"))?;
        fs::write(&self.path, contents)
            .map_err(|err| format!("failed to write session {}: {err}", self.path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn log_dir(&self) -> PathBuf {
        self.state_root
            .join("sessions")
            .join(&self.session_id)
            .join("logs")
    }

    pub fn plan_dir(&self) -> PathBuf {
        self.state_root
            .join("sessions")
            .join(&self.session_id)
            .join("plans")
    }

    pub fn log_dir_for(&self, session_id: &str) -> Option<PathBuf> {
        if uuid::Uuid::parse_str(session_id).is_err() {
            return None;
        }
        let candidate = self
            .state_root
            .join("sessions")
            .join(session_id)
            .join("logs");
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    }
}

/// Bring a restored `SessionSnapshot` back to a runnable state before it is
/// handed to `Agent::new`:
///   (1) If `active_root` points at a directory that no longer exists,
///       clear it so the agent falls back to the current cwd.
///   (2) If the session is mid-Plan but its `active_plan_path` file is gone,
///       downgrade to Act so the agent does not try to open a missing plan.
///
/// Emits a `warn:` line to stderr for each reconciliation. Idempotent.
pub fn reconcile_resume_state(session: &mut SessionSnapshot, _cwd: &Path) {
    if let Some(root) = &session.active_root
        && !root.is_dir()
    {
        eprintln!(
            "warn: session.active_root no longer exists ({}); falling back to cwd",
            root.display()
        );
        session.active_root = None;
    }

    if session.mode_state.mode == ExecutionMode::Plan {
        let missing_plan = session
            .mode_state
            .active_plan_path
            .as_ref()
            .is_some_and(|p| !p.exists());
        if missing_plan {
            eprintln!(
                "warn: plan file missing ({}); downgrading to Act mode",
                session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
            session.mode_state.mode = ExecutionMode::Act;
            session.mode_state.active_plan_path = None;
        }
    }
}
