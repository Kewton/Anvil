use std::path::Path;

use crate::config::repo_instructions::RepoInstructions;
use crate::prompts::context::{render_context_blocks, ContextBlock};
use crate::runtime::events::RuntimeEvent;
use crate::runtime::sandbox::{PermissionDecision, SandboxPolicy};
use crate::runtime::trust::SourceType;
use crate::tools::registry::{ToolRegistry, ToolRequest, ToolResponse};

#[derive(Debug)]
pub struct RuntimeEngine {
    sandbox: SandboxPolicy,
    tools: ToolRegistry,
    repo_instructions: RepoInstructions,
}

impl RuntimeEngine {
    pub fn new(
        sandbox: SandboxPolicy,
        tools: ToolRegistry,
        repo_instructions: RepoInstructions,
    ) -> Self {
        Self {
            sandbox,
            tools,
            repo_instructions,
        }
    }

    pub fn execute(&self, request: ToolRequest) -> anyhow::Result<RuntimeEvent> {
        match self.sandbox.evaluate(&request) {
            PermissionDecision::Allowed => {
                let response = self.tools.execute(request)?;
                Ok(RuntimeEvent {
                    message: describe_response(&response),
                })
            }
            PermissionDecision::NeedsConfirmation(reason) => Ok(RuntimeEvent {
                message: format!("confirmation required: {reason}"),
            }),
            PermissionDecision::Blocked(reason) => Ok(RuntimeEvent {
                message: format!("blocked: {reason}"),
            }),
        }
    }

    pub fn build_context(&self, user_prompt: &str, repo_file_blocks: Vec<ContextBlock>) -> String {
        let mut blocks = vec![ContextBlock::new(SourceType::User, user_prompt)];
        if let Some(instructions) = self.repo_instructions.as_context_block() {
            blocks.push(instructions);
        }
        blocks.extend(repo_file_blocks);
        render_context_blocks(&blocks)
    }

    pub fn workspace_root(&self) -> &Path {
        self.sandbox.workspace_root()
    }
}

fn describe_response(response: &ToolResponse) -> String {
    match response {
        ToolResponse::FileContents(result) => format!("read file {}", result.path.display()),
        ToolResponse::WriteResult(result) => format!(
            "wrote {} bytes to {}",
            result.bytes_written,
            result.path.display()
        ),
        ToolResponse::SearchMatches(matches) => {
            format!("search produced {} matches", matches.len())
        }
        ToolResponse::ExecResult(result) => {
            format!("command completed with exit code {:?}", result.exit_code)
        }
        ToolResponse::EnvSnapshot(snapshot) => {
            format!("environment cwd {}", snapshot.cwd.display())
        }
        ToolResponse::Diff(diff) => format!("diff produced {} bytes", diff.len()),
    }
}
