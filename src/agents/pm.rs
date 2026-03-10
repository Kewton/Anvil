use crate::agents::editor::EditorAgent;
use crate::agents::reader::ReaderAgent;
use crate::agents::reviewer::ReviewerAgent;
use crate::agents::tester::TesterAgent;
use crate::agents::{AgentResult, AgentTask};
use crate::models::client::ModelRequest;
use crate::models::routing::ModelRouter;
use crate::roles::EffectiveModels;

pub struct PmAgent {
    router: ModelRouter,
    reader: ReaderAgent,
    editor: EditorAgent,
    tester: TesterAgent,
    reviewer: ReviewerAgent,
}

impl Default for PmAgent {
    fn default() -> Self {
        Self {
            router: ModelRouter::default(),
            reader: ReaderAgent,
            editor: EditorAgent,
            tester: TesterAgent,
            reviewer: ReviewerAgent,
        }
    }
}

impl PmAgent {
    pub fn new(router: ModelRouter) -> Self {
        Self {
            router,
            reader: ReaderAgent,
            editor: EditorAgent,
            tester: TesterAgent,
            reviewer: ReviewerAgent,
        }
    }

    pub fn run_turn(
        &self,
        models: &EffectiveModels,
        user_prompt: &str,
        context: &str,
    ) -> anyhow::Result<PmTurnOutcome> {
        match decide_strategy(user_prompt) {
            ExecutionStrategy::FastPath => {
                let response = self.router.complete(&ModelRequest {
                    model: models.pm_model.clone(),
                    system_prompt: "PM fast-path".to_string(),
                    user_prompt: user_prompt.to_string(),
                })?;

                Ok(PmTurnOutcome {
                    delegated_role: None,
                    result: AgentResult::new(
                        "pm",
                        format!(
                            "PM handled the request directly using {}. Context bytes: {}. {}",
                            response.provider,
                            context.len(),
                            response.output
                        ),
                    ),
                })
            }
            ExecutionStrategy::Delegate(role) => {
                let task = AgentTask {
                    description: user_prompt.to_string(),
                    context: context.to_string(),
                };
                let result = match role {
                    AgentRole::Reader => self.reader.run(&task),
                    AgentRole::Editor => self.editor.run(&task),
                    AgentRole::Tester => self.tester.run(&task),
                    AgentRole::Reviewer => self.reviewer.run(&task),
                };

                Ok(PmTurnOutcome {
                    delegated_role: Some(role),
                    result,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AgentRole {
    Reader,
    Editor,
    Tester,
    Reviewer,
}

#[derive(Debug, Clone)]
pub struct PmTurnOutcome {
    pub delegated_role: Option<AgentRole>,
    pub result: AgentResult,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ExecutionStrategy {
    FastPath,
    Delegate(AgentRole),
}

fn decide_strategy(prompt: &str) -> ExecutionStrategy {
    let normalized = prompt.to_ascii_lowercase();

    if normalized.contains("test") || normalized.contains("lint") || normalized.contains("build") {
        return ExecutionStrategy::Delegate(AgentRole::Tester);
    }

    if normalized.contains("review") || normalized.contains("regression") {
        return ExecutionStrategy::Delegate(AgentRole::Reviewer);
    }

    if normalized.contains("edit")
        || normalized.contains("implement")
        || normalized.contains("change")
        || normalized.contains("fix")
    {
        return ExecutionStrategy::Delegate(AgentRole::Editor);
    }

    if normalized.contains("inspect")
        || normalized.contains("summarize")
        || normalized.contains("explain")
        || normalized.contains("read")
    {
        return ExecutionStrategy::Delegate(AgentRole::Reader);
    }

    ExecutionStrategy::FastPath
}
