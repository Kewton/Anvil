use crate::agents::{AgentResult, AgentTask};

#[derive(Debug, Default)]
pub struct EditorAgent;

impl EditorAgent {
    pub fn run(&self, task: &AgentTask) -> AgentResult {
        AgentResult::new(
            "editor",
            format!(
                "Editor prepared a bounded edit plan for: {}",
                task.description
            ),
        )
    }
}
