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
        let analysis = analyze_task(user_prompt);
        let plan = build_turn_plan(user_prompt, &analysis);

        if plan.allow_fast_path {
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

        let mut delegated_roles = Vec::new();
        let mut results = Vec::new();
        let mut step_context = context.to_string();

        for step in &plan.steps {
            let result = self.run_agent(
                step.role,
                user_prompt,
                &step.objective,
                &step_context,
                runtime,
            );
            delegated_roles.push(step.role);
            step_context.push_str("\n[source=subagent]\n");
            step_context.push_str(&result.role);
            step_context.push_str(": ");
            step_context.push_str(&result.summary);
            let should_stop = result.is_blocked() || result.needs_confirmation();
            results.push(result);
            if should_stop {
                break;
            }
        }

        let user_response = synthesize_plan_response(user_prompt, &plan, &results);

        Ok(PmTurnOutcome {
            delegated_roles,
            results,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TaskKind {
    Conversational,
    RepositoryAnalysis,
    BranchAnalysis,
    Review,
    Change,
    Validation,
    Inspection,
}

#[derive(Debug, Clone, Copy)]
struct TaskAnalysis {
    kind: TaskKind,
    wants_validation: bool,
    wants_review: bool,
}

#[derive(Debug, Clone)]
struct TurnPlan {
    kind: TaskKind,
    allow_fast_path: bool,
    steps: Vec<PlanStep>,
}

#[derive(Debug, Clone)]
struct PlanStep {
    role: AgentRole,
    objective: String,
}

fn analyze_task(prompt: &str) -> TaskAnalysis {
    let normalized = prompt.to_ascii_lowercase();
    let branchish = contains_any(&normalized, &["branch", "commit", "log", "history"])
        || contains_any(prompt, &["ブランチ", "コミット", "履歴"]);
    let repoish = contains_any(&normalized, &["repository", "repo", "codebase"])
        || contains_any(prompt, &["リポジトリ", "コードベース"]);
    let explainish = contains_any(&normalized, &["analy", "explain", "summarize"])
        || contains_any(prompt, &["分析", "解説", "解析", "要約"]);
    let inspectish = contains_any(&normalized, &["inspect", "read"])
        || contains_any(prompt, &["調査", "確認"]);
    let reviewish = contains_any(&normalized, &["review", "regression", "diff"])
        || contains_any(prompt, &["レビュー", "差分"]);
    let changeish = contains_any(
        &normalized,
        &["edit", "implement", "change", "fix", "apply", "update", "write"],
    ) || contains_any(prompt, &["修正", "変更", "実装", "更新"]);
    let validationish = contains_any(
        &normalized,
        &["test", "lint", "build", "check", "validate", "clippy", "format", "fmt"],
    ) || contains_any(prompt, &["テスト", "ビルド", "検証", "確認"]);
    let contextual = repoish || branchish || reviewish;

    let kind = if branchish && (explainish || reviewish) {
        TaskKind::BranchAnalysis
    } else if repoish && explainish {
        TaskKind::RepositoryAnalysis
    } else if changeish {
        TaskKind::Change
    } else if validationish {
        TaskKind::Validation
    } else if reviewish {
        TaskKind::Review
    } else if inspectish || contextual || explainish {
        TaskKind::Inspection
    } else {
        TaskKind::Conversational
    };

    TaskAnalysis {
        kind,
        wants_validation: validationish,
        wants_review: reviewish || matches!(kind, TaskKind::BranchAnalysis),
    }
}

fn build_turn_plan(prompt: &str, analysis: &TaskAnalysis) -> TurnPlan {
    use AgentRole::{Editor, Reader, Reviewer, Tester};

    let mut steps = Vec::new();

    match analysis.kind {
        TaskKind::Conversational => {
            return TurnPlan {
                kind: analysis.kind,
                allow_fast_path: true,
                steps,
            };
        }
        TaskKind::RepositoryAnalysis => {
            steps.push(plan_step(
                Reader,
                format!("inspect the repository layout and summarize the main areas for: {prompt}"),
            ));
            steps.push(plan_step(
                Reviewer,
                format!("review the current diff and highlight notable risk areas for: {prompt}"),
            ));
        }
        TaskKind::BranchAnalysis => {
            steps.push(plan_step(
                Reader,
                format!("inspect the current branch and recent commits for: {prompt}"),
            ));
            steps.push(plan_step(
                Reviewer,
                format!("review the current diff and highlight notable risk areas for: {prompt}"),
            ));
        }
        TaskKind::Review => {
            steps.push(plan_step(
                Reviewer,
                format!("review the current diff for: {prompt}"),
            ));
        }
        TaskKind::Change => {
            steps.push(plan_step(
                Reader,
                format!("inspect the relevant code paths before editing for: {prompt}"),
            ));
            steps.push(plan_step(
                Editor,
                format!("implement or update the relevant file for: {prompt}"),
            ));
            if analysis.wants_validation || prompt_mentions_fix(prompt) {
                steps.push(plan_step(
                    Tester,
                    format!("validate the recent change for: {prompt}"),
                ));
            }
            if analysis.wants_review {
                steps.push(plan_step(
                    Reviewer,
                    format!("review the resulting diff for: {prompt}"),
                ));
            }
        }
        TaskKind::Validation => {
            steps.push(plan_step(
                Tester,
                format!("run the appropriate validation command for: {prompt}"),
            ));
        }
        TaskKind::Inspection => {
            steps.push(plan_step(
                Reader,
                format!("inspect the relevant repository context for: {prompt}"),
            ));
            if analysis.wants_review {
                steps.push(plan_step(
                    Reviewer,
                    format!("review the current diff for: {prompt}"),
                ));
            }
        }
    }

    TurnPlan {
        kind: analysis.kind,
        allow_fast_path: false,
        steps,
    }
}

fn plan_step(role: AgentRole, objective: String) -> PlanStep {
    PlanStep { role, objective }
}

fn prompt_mentions_fix(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    contains_any(&normalized, &["fix", "implement", "repair"]) || contains_any(prompt, &["修正", "直す"])
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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
            let mut parts = vec![presented[0].clone()];
            if let Some(review) = presented.get(1) {
                parts.push(format!("差分観点では {review}"));
            }
            parts.push("必要ならこのまま主要ディレクトリ、変更点、テスト経路を順に掘り下げられます。".to_string());
            join_sentences(&parts)
        }
        TaskKind::BranchAnalysis => {
            let mut parts = vec![presented[0].clone()];
            if let Some(review) = presented.get(1) {
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
        TaskKind::Validation => join_sentences(
            &presented
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    if index == 0 && results.len() > 1 {
                        format!("事前確認として {item}")
                    } else {
                        item
                    }
                })
                .collect::<Vec<_>>(),
        ),
        TaskKind::Review | TaskKind::Inspection | TaskKind::Conversational => join_sentences(&presented),
    }
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
