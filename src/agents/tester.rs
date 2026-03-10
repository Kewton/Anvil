use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::exec::ExecRequest;
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct TesterAgent;

impl TesterAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        match runtime.checked_execute(ToolRequest::Exec {
            request: ExecRequest {
                program: "cargo".to_string(),
                args: vec!["--version".to_string()],
                cwd: task.workspace_root.clone(),
            },
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::ExecResult(result))) => AgentResult::new(
                "tester",
                format!(
                    "Tester ran `cargo --version` with exit code {:?} while handling: {}",
                    result.exit_code, task.description
                ),
            ),
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                AgentResult::new("tester", format!("Tester blocked: {reason}"))
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
                AgentResult::new("tester", format!("Tester awaiting confirmation: {reason}"))
            }
            Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => AgentResult::new(
                "tester",
                format!(
                    "Tester could not summarize command execution for: {}",
                    task.description
                ),
            ),
        }
    }
}
