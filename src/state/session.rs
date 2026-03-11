use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub root: PathBuf,
    #[serde(default)]
    pub rolling_summary: Option<String>,
    #[serde(default)]
    pub summarized_events: usize,
}

impl Session {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            id: format!("sess_{}", Uuid::new_v4().simple()),
            root: root.into(),
            rolling_summary: None,
            summarized_events: 0,
        }
    }

    pub fn update_summary(&mut self, summary: Option<String>, summarized_events: usize) {
        self.rolling_summary = summary;
        self.summarized_events = summarized_events;
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}
