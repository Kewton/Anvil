use crate::agents::{AgentResult, AgentTask};

#[derive(Debug, Default)]
pub struct ReaderAgent;

impl ReaderAgent {
    pub fn run(&self, task: &AgentTask) -> AgentResult {
        AgentResult::new(
            "reader",
            format!("Reader inspected scoped context for: {}", task.description),
        )
    }
}
