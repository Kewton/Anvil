use crate::agents::{AgentResult, AgentTask};

#[derive(Debug, Default)]
pub struct ReviewerAgent;

impl ReviewerAgent {
    pub fn run(&self, task: &AgentTask) -> AgentResult {
        AgentResult::new(
            "reviewer",
            format!("Reviewer prepared a risk pass for: {}", task.description),
        )
    }
}
