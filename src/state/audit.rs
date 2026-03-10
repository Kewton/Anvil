use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::policy::permissions::{PermissionCategory, PermissionMode, PermissionRequirement};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActor {
    User,
    MainAgent,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSource {
    Interactive,
    OneShot,
    SlashCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditMetadata {
    pub schema_version: u16,
    pub event_id: String,
    pub ts: DateTime<Utc>,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub actor: AuditActor,
    pub source: AuditSource,
    pub cwd: Option<PathBuf>,
}

impl AuditMetadata {
    pub fn new(
        session_id: impl Into<String>,
        actor: AuditActor,
        source: AuditSource,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            schema_version: 1,
            event_id: format!("evt_{}", Uuid::new_v4().simple()),
            ts: Utc::now(),
            session_id: session_id.into(),
            turn_id: None,
            actor,
            source,
            cwd: Some(cwd.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEventData {
    SessionStarted {
        model: String,
        permission_mode: PermissionMode,
    },
    PermissionRequested {
        category: PermissionCategory,
        requirement: PermissionRequirement,
        target: String,
    },
    ToolExecution {
        tool_name: String,
        args_summary: BTreeMap<String, String>,
    },
    ToolResult {
        tool_name: String,
        status: ToolResultStatus,
        changed_files: Vec<PathBuf>,
    },
    MemoryUpdated {
        summary: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub meta: AuditMetadata,
    pub data: AuditEventData,
}

#[derive(Debug)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &AuditEvent) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = open_append(&self.path)?;
        let line = serde_json::to_string(event)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn load_all(&self) -> anyhow::Result<Vec<AuditEvent>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&self.path)?;
        let mut events = Vec::new();
        for line in text.lines() {
            events.push(serde_json::from_str(line)?);
        }
        Ok(events)
    }
}

fn open_append(path: &Path) -> anyhow::Result<File> {
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}
