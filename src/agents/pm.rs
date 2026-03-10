use crate::agents::editor::EditorAgent;
use crate::agents::reader::ReaderAgent;
use crate::agents::reviewer::ReviewerAgent;
use crate::agents::tester::TesterAgent;
use crate::agents::{AgentResult, AgentTask};
use crate::models::client::ModelRequest;
use crate::models::routing::ModelRouter;
use crate::roles::EffectiveModels;
use crate::runtime::engine::RuntimeEngine;

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
        runtime: &RuntimeEngine,
    ) -> anyhow::Result<PmTurnOutcome> {
        self.run_turn_with_stream(models, user_prompt, context, runtime, None)
    }

    pub fn run_turn_with_stream(
        &self,
        models: &EffectiveModels,
        user_prompt: &str,
        context: &str,
        runtime: &RuntimeEngine,
        mut on_chunk: Option<&mut dyn FnMut(&str)>,
    ) -> anyhow::Result<PmTurnOutcome> {
        match decide_strategy(user_prompt) {
            ExecutionStrategy::FastPath => {
                let request = ModelRequest {
                    model: models.pm_model.clone(),
                    system_prompt: "PM fast-path".to_string(),
                    user_prompt: user_prompt.to_string(),
                };
                let response = match on_chunk.as_mut() {
                    Some(callback) => self.router.stream_complete(&request, *callback)?,
                    None => self.router.complete(&request)?,
                };

                Ok(PmTurnOutcome {
                    delegated_role: None,
                    user_response: response.output.clone(),
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
                    workspace_root: runtime.workspace_root().to_path_buf(),
                };
                let result = match role {
                    AgentRole::Reader => self.reader.run(&task, runtime),
                    AgentRole::Editor => self.editor.run(&task, runtime),
                    AgentRole::Tester => self.tester.run(&task, runtime),
                    AgentRole::Reviewer => self.reviewer.run(&task, runtime),
                };
                let user_response = present_subagent_result(&result.summary);

                Ok(PmTurnOutcome {
                    delegated_role: Some(role),
                    result,
                    user_response,
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
    pub user_response: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ExecutionStrategy {
    FastPath,
    Delegate(AgentRole),
}

fn decide_strategy(prompt: &str) -> ExecutionStrategy {
    let normalized = prompt.to_ascii_lowercase();

    if normalized.contains("branch")
        || normalized.contains("commit")
        || normalized.contains("commits")
        || normalized.contains("log")
        || prompt.contains("ブランチ")
        || prompt.contains("コミット")
        || prompt.contains("履歴")
        || prompt.contains("解説")
    {
        return ExecutionStrategy::Delegate(AgentRole::Reader);
    }

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
        || normalized.contains("apply")
        || normalized.contains("update")
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

fn present_subagent_result(summary: &str) -> String {
    let trimmed = summary.trim();
    let body = ["Reader ", "Editor ", "Tester ", "Reviewer "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
        .unwrap_or(trimmed);
    capitalize_first(body)
}

fn capitalize_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}
