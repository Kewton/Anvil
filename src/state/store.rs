use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::roles::RoleRegistry;
use crate::state::handoff::HandoffFile;
use crate::state::session::SessionState;
use crate::util::json::pretty;

#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new(default_root())
    }
}

impl StateStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn handoffs_dir(&self) -> PathBuf {
        self.root.join("handoffs")
    }

    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        fs::create_dir_all(self.sessions_dir()).context("failed to create sessions directory")?;
        fs::create_dir_all(self.handoffs_dir()).context("failed to create handoffs directory")?;
        Ok(())
    }

    pub fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    pub fn handoff_path(&self, session_id: &str) -> PathBuf {
        self.handoffs_dir().join(format!("{session_id}.json"))
    }

    pub fn load_session(
        &self,
        registry: &RoleRegistry,
        session_id: &str,
    ) -> anyhow::Result<SessionState> {
        let path = self.session_path(session_id);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read session file {}", path.display()))?;
        let session: SessionState = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse session file {}", path.display()))?;
        session.validate(registry)?;
        Ok(session)
    }

    pub fn save_session(
        &self,
        registry: &RoleRegistry,
        session: &SessionState,
    ) -> anyhow::Result<PathBuf> {
        self.ensure_layout()?;
        session.validate(registry)?;
        let path = self.session_path(&session.session_id);
        fs::write(&path, pretty(session)?)
            .with_context(|| format!("failed to write session file {}", path.display()))?;
        Ok(path)
    }

    pub fn load_handoff(
        &self,
        registry: &RoleRegistry,
        path: &Path,
    ) -> anyhow::Result<HandoffFile> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read handoff file {}", path.display()))?;
        let handoff: HandoffFile = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse handoff file {}", path.display()))?;
        handoff.validate(registry)?;
        Ok(handoff)
    }

    pub fn save_handoff(
        &self,
        registry: &RoleRegistry,
        handoff: &HandoffFile,
    ) -> anyhow::Result<PathBuf> {
        self.ensure_layout()?;
        handoff.validate(registry)?;
        let path = self.handoff_path(&handoff.session_id);
        fs::write(&path, pretty(handoff)?)
            .with_context(|| format!("failed to write handoff file {}", path.display()))?;
        Ok(path)
    }
}

fn default_root() -> PathBuf {
    if let Some(path) = env::var_os("ANVIL_HOME") {
        return PathBuf::from(path);
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".anvil");
    }

    PathBuf::from(".anvil")
}
