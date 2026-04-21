use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExecutionMode {
    Plan,
    Act,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModeState {
    pub mode: ExecutionMode,
    pub active_plan_path: Option<PathBuf>,
}

impl Default for ModeState {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::Act,
            active_plan_path: None,
        }
    }
}

impl ModeState {
    pub fn enter_plan(&mut self, plan_dir: PathBuf) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("failed to build timestamp: {err}"))?
            .as_secs();
        let path = plan_dir.join(format!("plan-{ts}.md"));
        self.mode = ExecutionMode::Plan;
        self.active_plan_path = Some(path.clone());
        Ok(path)
    }

    pub fn approve(&mut self) {
        self.mode = ExecutionMode::Act;
    }
}
