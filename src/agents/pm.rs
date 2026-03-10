use crate::agents::executor::StepExecutor;
use crate::agents::planning::{TaskAnalyzer, TaskKind, TurnPlan, TurnPlanner};
use crate::agents::editor::EditorAgent;
use crate::agents::reader::ReaderAgent;
use crate::agents::reviewer::ReviewerAgent;
use crate::agents::tester::TesterAgent;
use crate::agents::{AgentResult, AgentTask};
use crate::models::client::ModelRequest;
use crate::models::profile::LocalModelProfile;
use crate::models::routing::ModelRouter;
use crate::roles::EffectiveModels;
use crate::runtime::engine::RuntimeEngine;

pub struct PmAgent {
    router: ModelRouter,
    analyzer: TaskAnalyzer,
    planner: TurnPlanner,
    executor: StepExecutor,
    reader: ReaderAgent,
    editor: EditorAgent,
    tester: TesterAgent,
    reviewer: ReviewerAgent,
}

impl Default for PmAgent {
    fn default() -> Self {
        Self {
            router: ModelRouter::default(),
            analyzer: TaskAnalyzer,
            planner: TurnPlanner,
            executor: StepExecutor,
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
            analyzer: TaskAnalyzer,
            planner: TurnPlanner,
            executor: StepExecutor,
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
        let analysis = self.analyzer.analyze(user_prompt);
        let plan = self.planner.build(user_prompt, &analysis);

        if plan.allow_fast_path
            && LocalModelProfile::from_model_name(&models.pm_model).allows_fast_path(user_prompt)
        {
            let request = ModelRequest {
                model: models.pm_model.clone(),
                system_prompt: "PM fast-path".to_string(),
                user_prompt: user_prompt.to_string(),
            };
            let response = match on_chunk.as_mut() {
                Some(callback) => self.router.stream_complete(&request, *callback)?,
                None => self.router.complete(&request)?,
            };

            return Ok(PmTurnOutcome {
                delegated_roles: Vec::new(),
                user_response: response.output.clone(),
                results: vec![AgentResult::new(
                    "pm",
                    format!(
                        "PM handled the request directly using {}. Context bytes: {}. {}",
                        response.provider,
                        context.len(),
                        response.output
                    ),
                )],
            });
        }

        let trace = self.executor.execute(&plan.steps, context, |step, step_context| {
            self.run_agent(
                step.role,
                user_prompt,
                &step.objective,
                step_context,
                runtime,
            )
        });

        let user_response = synthesize_plan_response(user_prompt, &plan, &trace.results);

        Ok(PmTurnOutcome {
            delegated_roles: trace.delegated_roles,
            results: trace.results,
            user_response,
        })
    }

    fn run_agent(
        &self,
        role: AgentRole,
        user_request: &str,
        objective: &str,
        context: &str,
        runtime: &RuntimeEngine,
    ) -> AgentResult {
        let task = AgentTask {
            description: objective.to_string(),
            user_request: user_request.to_string(),
            context: context.to_string(),
            workspace_root: runtime.workspace_root().to_path_buf(),
        };
        match role {
            AgentRole::Reader => self.reader.run(&task, runtime),
            AgentRole::Editor => self.editor.run(&task, runtime),
            AgentRole::Tester => self.tester.run(&task, runtime),
            AgentRole::Reviewer => self.reviewer.run(&task, runtime),
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
    pub delegated_roles: Vec<AgentRole>,
    pub results: Vec<AgentResult>,
    pub user_response: String,
}

fn synthesize_plan_response(prompt: &str, plan: &TurnPlan, results: &[AgentResult]) -> String {
    if results.is_empty() {
        return format!("{} に向けて調査を始めましたが、まだ十分な結果を得られていません。", prompt);
    }

    if let Some(result) = results.iter().find(|result| result.is_blocked()) {
        return format!(
            "{} 作業はここで止まっています。",
            present_subagent_result(&result.summary)
        );
    }

    if let Some(result) = results.iter().find(|result| result.needs_confirmation()) {
        let mut completed = results
            .iter()
            .take_while(|item| !std::ptr::eq(*item, result))
            .map(|item| present_subagent_result(&item.summary))
            .collect::<Vec<_>>();
        let pending = present_subagent_result(&result.summary);
        if completed.is_empty() {
            return format!("{pending} 実行を続けるには `/approve`、取りやめるには `/deny` を使ってください。");
        }
        completed.push(format!(
            "{pending} 実行を続けるには `/approve`、取りやめるには `/deny` を使ってください。"
        ));
        return join_sentences(&completed);
    }

    let presented = results
        .iter()
        .map(|result| present_subagent_result(&result.summary))
        .collect::<Vec<_>>();

    match plan.kind {
        TaskKind::RepositoryAnalysis => {
            let mut parts = Vec::new();
            if let Some(tracked) = latest_fact(results, "repo.tracked_files") {
                if let Some(areas) = latest_fact(results, "repo.top_areas") {
                    parts.push(format!(
                        "リポジトリには {} 個の tracked file があり、主な領域は {} です",
                        tracked, areas
                    ));
                }
            } else {
                parts.push(presented[0].clone());
            }
            if let Some(changed) = latest_fact(results, "diff.changed_files") {
                let additions = latest_fact(results, "diff.additions").unwrap_or("0");
                let deletions = latest_fact(results, "diff.deletions").unwrap_or("0");
                parts.push(format!(
                    "差分は {} files で、+{} / -{} です",
                    changed, additions, deletions
                ));
            } else if let Some(review) = presented.get(1) {
                parts.push(format!("差分観点では {review}"));
            }
            parts.push("必要ならこのまま主要ディレクトリ、変更点、テスト経路を順に掘り下げられます。".to_string());
            join_sentences(&parts)
        }
        TaskKind::BranchAnalysis => {
            let mut parts = Vec::new();
            if let Some(branch) = latest_fact(results, "git.branch") {
                let mut line = format!("現在のブランチは `{}` です", branch);
                if let Some(commit) = latest_fact(results, "git.latest_commit") {
                    line.push_str(&format!("。直近のコミットは {} です", commit));
                }
                parts.push(line);
            } else {
                parts.push(presented[0].clone());
            }
            if let Some(diff_summary) = latest_fact(results, "git.diff_summary") {
                parts.push(diff_summary.to_string());
            } else if let Some(review) = presented.get(1) {
                parts.push(format!("差分観点では {review}"));
            }
            parts.push("必要なら次に変更ファイル単位で要点を分解します。".to_string());
            join_sentences(&parts)
        }
        TaskKind::Change => {
            let mut parts = Vec::new();
            if let Some(first) = presented.first() {
                parts.push(format!("まず {first}"));
            }
            if let Some(edit) = presented.get(1) {
                parts.push(format!("その上で {edit}"));
            }
            if let Some(test) = presented.get(2) {
                parts.push(format!("検証として {test}"));
            }
            if let Some(review) = presented.get(3) {
                parts.push(format!("最後に {review}"));
            }
            join_sentences(&parts)
        }
        TaskKind::Validation => {
            if let Some(command) = latest_fact(results, "validation.command") {
                let exit_code = latest_fact(results, "validation.exit_code").unwrap_or("unknown");
                return format!(
                    "`{}` を実行し、exit code は {} でした。必要なら出力を掘り下げます。",
                    command, exit_code
                );
            }
            join_sentences(&presented)
        }
        TaskKind::Review => {
            if let Some(changed) = latest_fact(results, "diff.changed_files") {
                let additions = latest_fact(results, "diff.additions").unwrap_or("0");
                let deletions = latest_fact(results, "diff.deletions").unwrap_or("0");
                return format!(
                    "差分は {} files で、+{} / -{} です。必要なら高リスク箇所をさらに分解します。",
                    changed, additions, deletions
                );
            }
            join_sentences(&presented)
        }
        TaskKind::Inspection | TaskKind::Conversational => join_sentences(&presented),
    }
}

fn latest_fact<'a>(results: &'a [AgentResult], key: &str) -> Option<&'a str> {
    results
        .iter()
        .rev()
        .find_map(|result| {
            result
                .facts
                .iter()
                .rev()
                .find(|fact| fact.key == key)
                .map(|fact| fact.value.as_str())
        })
}

fn join_sentences(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.trim().is_empty())
        .map(|part| ensure_sentence(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.ends_with('。') || trimmed.ends_with('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}。")
    }
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
