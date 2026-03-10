use crate::agents::pm::AgentRole;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TaskKind {
    Conversational,
    RepositoryAnalysis,
    BranchAnalysis,
    Review,
    Change,
    Validation,
    Inspection,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskAnalysis {
    pub kind: TaskKind,
    pub wants_validation: bool,
    pub wants_review: bool,
    pub needs_repo_grounding: bool,
}

#[derive(Debug, Clone)]
pub struct TurnPlan {
    pub kind: TaskKind,
    pub allow_fast_path: bool,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub role: AgentRole,
    pub objective: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TaskAnalyzer;

#[derive(Debug, Default, Clone, Copy)]
pub struct TurnPlanner;

impl TaskAnalyzer {
    pub fn analyze(&self, prompt: &str) -> TaskAnalysis {
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
        } else if inspectish || repoish || branchish || explainish {
            TaskKind::Inspection
        } else {
            TaskKind::Conversational
        };

        TaskAnalysis {
            kind,
            wants_validation: validationish,
            wants_review: reviewish || matches!(kind, TaskKind::BranchAnalysis),
            needs_repo_grounding: !matches!(kind, TaskKind::Conversational),
        }
    }
}

impl TurnPlanner {
    pub fn build(&self, prompt: &str, analysis: &TaskAnalysis) -> TurnPlan {
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
