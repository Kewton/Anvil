use crate::agents::{AgentResult, AgentTask};

#[derive(Debug, Default)]
pub struct TesterAgent;

impl TesterAgent {
    pub fn run(&self, task: &AgentTask) -> AgentResult {
        AgentResult::new(
            "tester",
            format!("Tester queued focused validation for: {}", task.description),
        )
    }
}
