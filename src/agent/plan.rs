use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMode {
    Plan,
    Act,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDocument {
    pub path: PathBuf,
    pub body: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct PlanState {
    pub mode: AgentMode,
    active: Option<PlanDocument>,
}

impl Default for PlanState {
    fn default() -> Self {
        Self {
            mode: AgentMode::Plan,
            active: None,
        }
    }
}

impl PlanDocument {
    pub fn new(path: PathBuf, body: String, summary: Option<String>) -> Self {
        let summary = summary.unwrap_or_else(|| summarize_plan(&body));
        Self {
            path,
            body,
            summary,
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let body = std::fs::read_to_string(path)?;
        Ok(Self::new(path.to_path_buf(), body, None))
    }
}

impl PlanState {
    pub fn create_plan(root: &Path, slug: &str, body: &str) -> anyhow::Result<PlanDocument> {
        let dir = root.join(".anvil/plans");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{slug}.md"));
        std::fs::write(&path, body)?;
        Ok(PlanDocument::new(path, body.to_string(), None))
    }

    pub fn enter_plan(doc: PlanDocument) -> Self {
        Self {
            mode: AgentMode::Plan,
            active: Some(doc),
        }
    }

    pub fn activate(doc: PlanDocument) -> Self {
        Self {
            mode: AgentMode::Act,
            active: Some(doc),
        }
    }

    pub fn injection(&self) -> Option<String> {
        self.active
            .as_ref()
            .map(|doc| format!("Active plan summary:\n{}", doc.summary))
    }

    pub fn show(&self) -> Option<String> {
        self.active.as_ref().map(|doc| doc.body.clone())
    }

    pub fn active_document(&self) -> Option<&PlanDocument> {
        self.active.as_ref()
    }

    pub fn active_path_display(&self) -> String {
        self.active
            .as_ref()
            .map(|doc| doc.path.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    }
}

fn summarize_plan(body: &str) -> String {
    body.lines().take(8).collect::<Vec<_>>().join(" | ")
}
