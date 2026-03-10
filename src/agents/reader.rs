use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::exec::ExecRequest;
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct ReaderAgent;

impl ReaderAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        if looks_like_git_inspection(&task.description) {
            return inspect_git_state(task, runtime);
        }
        if looks_like_repository_analysis(&task.description) {
            return inspect_repository(task, runtime);
        }

        let cwd = match runtime.checked_execute(ToolRequest::InspectEnv) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::EnvSnapshot(snapshot))) => {
                snapshot.cwd.display().to_string()
            }
            _ => task.workspace_root.display().to_string(),
        };

        let needle = select_search_term(&task.description);
        let matches = match runtime.checked_execute(ToolRequest::Search {
            root: task.workspace_root.clone(),
            needle: needle.clone(),
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::SearchMatches(matches))) => matches.len(),
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                return AgentResult::new("reader", format!("Reader blocked: {reason}"));
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
                return AgentResult::new(
                    "reader",
                    format!("Reader awaiting confirmation: {reason}"),
                );
            }
            _ => 0,
        };

        AgentResult::new(
            "reader",
            format!(
                "Reader inspected {} and found {} matches for \"{}\" while handling: {}",
                cwd, matches, needle, task.user_request
            ),
        )
        .with_next_recommendation(
            "Use the matched files to decide whether editing or review is needed",
        )
    }
}

fn inspect_repository(task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
    let files = match runtime.checked_execute(ToolRequest::Exec {
        request: ExecRequest {
            program: "rg".to_string(),
            args: vec!["--files".to_string(), ".".to_string()],
            cwd: task.workspace_root.clone(),
        },
    }) {
        Ok(RuntimeToolOutcome::Allowed(ToolResponse::ExecResult(result))) => result.stdout,
        Ok(RuntimeToolOutcome::Blocked(reason)) => {
            return AgentResult::new("reader", format!("Reader blocked: {reason}"));
        }
        Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
            return AgentResult::new(
                "reader",
                format!("Reader awaiting confirmation: {reason}"),
            );
        }
        Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => String::new(),
    };

    let file_list: Vec<&str> = files.lines().filter(|line| !line.is_empty()).collect();
    let sample: Vec<&str> = file_list.iter().take(6).copied().collect();
    let top_dirs = summarize_top_directories(&file_list);

    AgentResult::new(
        "reader",
        format!(
            "Reader inspected the repository for {}. It currently exposes {} tracked files. Main areas: {}. Representative paths: {}.",
            task.user_request,
            file_list.len(),
            if top_dirs.is_empty() {
                "top-level files".to_string()
            } else {
                top_dirs.join(", ")
            },
            if sample.is_empty() {
                "none".to_string()
            } else {
                sample.join(", ")
            }
        ),
    )
    .with_evidence(vec![(
        "tool-output".to_string(),
        format!(
            "rg --files: {}",
            sample.iter().take(3).copied().collect::<Vec<_>>().join(", ")
        ),
    )])
    .with_next_recommendation(
        "Use targeted inspection on the main directories or review the current diff for deeper analysis",
    )
}

fn summarize_top_directories(files: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for file in files {
        let top = file.split('/').next().unwrap_or(file);
        if top.is_empty() || names.iter().any(|existing| existing == top) {
            continue;
        }
        names.push(top.to_string());
        if names.len() == 4 {
            break;
        }
    }
    names
}

fn inspect_git_state(task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
    let branch = match run_safe_git(
        runtime,
        &task.workspace_root,
        &["status", "--short", "--branch"],
    ) {
        Ok(output) => output,
        Err(result) => return result,
    };

    let recent_commits = match run_safe_git(
        runtime,
        &task.workspace_root,
        &["log", "--oneline", "--decorate", "-5"],
    ) {
        Ok(output) => output,
        Err(result) => return result,
    };

    let branch_line = branch.lines().next().unwrap_or("branch status unavailable");
    let status_entries: Vec<&str> = branch.lines().skip(1).filter(|line| !line.is_empty()).collect();
    let diff_stat = match run_safe_git(runtime, &task.workspace_root, &["diff", "--stat", "--", "."]) {
        Ok(output) => output,
        Err(result) => return result,
    };
    let parsed_branch = parse_branch_line(branch_line);
    let top_commit = recent_commits
        .lines()
        .next()
        .unwrap_or("no recent commits available");
    let diff_summary = summarize_diff_stat(&diff_stat, status_entries.len());

    AgentResult::new(
        "reader",
        format!(
            "{}。直近のコミットは {}。{}。",
            parsed_branch, top_commit, diff_summary
        ),
    )
    .with_evidence(vec![
        ("tool-output".to_string(), format!("git status: {branch_line}")),
        (
            "tool-output".to_string(),
            format!("git log: {}", truncate_line(top_commit)),
        ),
    ])
    .with_next_recommendation("Use git diff or review if you need a deeper explanation of the changes")
}

fn run_safe_git(
    runtime: &RuntimeEngine,
    cwd: &std::path::Path,
    args: &[&str],
) -> Result<String, AgentResult> {
    match runtime.checked_execute(ToolRequest::Exec {
        request: ExecRequest {
            program: "git".to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: cwd.to_path_buf(),
        },
    }) {
        Ok(RuntimeToolOutcome::Allowed(ToolResponse::ExecResult(result))) => Ok(result.stdout),
        Ok(RuntimeToolOutcome::Blocked(reason)) => {
            Err(AgentResult::new("reader", format!("Reader blocked: {reason}")))
        }
        Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => Err(AgentResult::new(
            "reader",
            format!("Reader awaiting confirmation: {reason}"),
        )),
        Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => Err(AgentResult::new(
            "reader",
            "Reader could not inspect the current git state",
        )),
    }
}

fn select_search_term(description: &str) -> String {
    description
        .split(|char: char| !char.is_alphanumeric())
        .find(|token| token.len() >= 4)
        .unwrap_or("fn")
        .to_ascii_lowercase()
}

fn looks_like_git_inspection(description: &str) -> bool {
    let normalized = description.to_ascii_lowercase();
    normalized.contains("branch")
        || normalized.contains("commit")
        || normalized.contains("commits")
        || normalized.contains("log")
        || description.contains("ブランチ")
        || description.contains("コミット")
        || description.contains("履歴")
        || description.contains("このブランチ")
}

fn looks_like_repository_analysis(description: &str) -> bool {
    let normalized = description.to_ascii_lowercase();
    normalized.contains("repository")
        || normalized.contains("repo")
        || normalized.contains("codebase")
        || description.contains("リポジトリ")
        || description.contains("コードベース")
}

fn truncate_line(line: &str) -> String {
    line.chars().take(200).collect()
}

fn parse_branch_line(line: &str) -> String {
    let trimmed = line.trim().trim_start_matches("## ").trim();
    let (branch_name, upstream) = trimmed
        .split_once("...")
        .map(|(branch, upstream)| (branch.trim(), Some(upstream.trim())))
        .unwrap_or((trimmed, None));

    let mut summary = format!("現在のブランチは `{branch_name}` です");
    if let Some(upstream) = upstream {
        if upstream.contains("[gone]") {
            let remote = upstream.split_whitespace().next().unwrap_or(upstream);
            summary.push_str(&format!("。追跡先の `{remote}` は現在見つかりません"));
        } else if !upstream.is_empty() {
            summary.push_str(&format!("。追跡先は `{}` です", upstream));
        }
    }
    summary
}

fn summarize_diff_stat(diff_stat: &str, status_count: usize) -> String {
    let diff_line = diff_stat
        .lines()
        .rev()
        .find(|line| line.contains("file changed") || line.contains("files changed"))
        .map(str::trim);

    if let Some(diff_line) = diff_line {
        return format!("ワークツリー差分は {diff_line}");
    }

    if status_count == 0 {
        "ワークツリーはクリーンです".to_string()
    } else {
        format!("未コミット変更は {status_count} 件あります")
    }
}
