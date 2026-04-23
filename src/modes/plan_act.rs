use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExecutionMode {
    Plan,
    Act,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum TaskProfile {
    #[default]
    Generic,
    Coding,
    Content,
    Ui,
    Research,
}

impl TaskProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskProfile::Generic => "generic",
            TaskProfile::Coding => "coding",
            TaskProfile::Content => "content",
            TaskProfile::Ui => "ui",
            TaskProfile::Research => "research",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum PlanStage {
    #[default]
    Stage1,
    Stage2,
    Stage3,
    Ready,
}

impl PlanStage {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStage::Stage1 => "stage1",
            PlanStage::Stage2 => "stage2",
            PlanStage::Stage3 => "stage3",
            PlanStage::Ready => "ready",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlanStage::Stage1 => "Stage 1",
            PlanStage::Stage2 => "Stage 2",
            PlanStage::Stage3 => "Stage 3",
            PlanStage::Ready => "Ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModeState {
    pub mode: ExecutionMode,
    pub active_plan_path: Option<PathBuf>,
    #[serde(default)]
    pub task_profile: TaskProfile,
    #[serde(default)]
    pub plan_stage: PlanStage,
}

impl Default for ModeState {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::Act,
            active_plan_path: None,
            task_profile: TaskProfile::Generic,
            plan_stage: PlanStage::Stage1,
        }
    }
}

impl ModeState {
    pub fn enter_plan(
        &mut self,
        plan_dir: PathBuf,
        task_profile: TaskProfile,
    ) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("failed to build timestamp: {err}"))?
            .as_secs();
        let path = plan_dir.join(format!("plan-{ts}.md"));
        self.mode = ExecutionMode::Plan;
        self.active_plan_path = Some(path.clone());
        self.task_profile = task_profile;
        self.plan_stage = PlanStage::Stage1;
        Ok(path)
    }

    pub fn approve(&mut self) {
        self.mode = ExecutionMode::Act;
        self.plan_stage = PlanStage::Ready;
    }
}
