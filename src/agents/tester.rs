use crate::agents::{AgentFact, AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::state::session::{PendingAction, PendingConfirmation};
use crate::tools::exec::ExecRequest;
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct TesterAgent;

impl TesterAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        let command = select_validation_command(&task.description);
        let request = ExecRequest {
            program: command.program.to_string(),
            args: command.args.iter().map(|arg| arg.to_string()).collect(),
            cwd: task.workspace_root.clone(),
        };
        match runtime.checked_execute(ToolRequest::Exec {
            request: request.clone(),
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::ExecResult(result))) => {
                summarize_exec_result(&task.user_request, command.display, result)
            }
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                AgentResult::blocked("tester", format!("Tester blocked: {reason}"))
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
                AgentResult::awaiting_confirmation(
                    "tester",
                    format!("Tester awaiting confirmation: {reason}"),
                )
                    .with_pending_confirmation(PendingConfirmation {
                        role: "tester".to_string(),
                        task: task.user_request.clone(),
                        summary: format!(
                            "Tester is waiting to run `{}` for: {}",
                            command.display, task.user_request
                        ),
                        reason,
                        action: PendingAction::Exec {
                            program: request.program,
                            args: request.args,
                            cwd: request.cwd.display().to_string(),
                            display: command.display.to_string(),
                        },
                    })
            }
            Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => AgentResult::new(
                "tester",
                format!(
                    "Tester could not summarize command execution for: {}",
                    task.user_request
                ),
            ),
        }
    }

    pub fn approve_pending(
        &self,
        runtime: &RuntimeEngine,
        task_description: &str,
        request: ExecRequest,
        display: &str,
    ) -> AgentResult {
        match runtime.execute_confirmed(ToolRequest::Exec { request }) {
            Ok(ToolResponse::ExecResult(result)) => {
                summarize_exec_result(task_description, display, result)
            }
            Ok(_) | Err(_) => AgentResult::new(
                "tester",
                format!(
                    "Tester could not execute the approved command for: {}",
                    task_description
                ),
            ),
        }
    }
}

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}

fn summarize_exec_result(
    task_description: &str,
    display: &str,
    result: crate::tools::exec::ExecResult,
) -> AgentResult {
    let mut evidence = Vec::new();
    if let Some(stdout) = first_non_empty_line(&result.stdout) {
        evidence.push(("tool-output".to_string(), format!("stdout: {stdout}")));
    }
    if let Some(stderr) = first_non_empty_line(&result.stderr) {
        evidence.push(("tool-output".to_string(), format!("stderr: {stderr}")));
    }

    AgentResult::new(
        "tester",
        format!(
            "Tester ran `{}` with exit code {:?} while handling: {}",
            display, result.exit_code, task_description
        ),
    )
    .with_facts(vec![
        AgentFact {
            key: "validation.command".to_string(),
            value: display.to_string(),
        },
        AgentFact {
            key: "validation.exit_code".to_string(),
            value: result
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        },
    ])
    .with_next_recommendation(
        "Inspect the validation output and decide whether another focused check is needed",
    )
    .with_commands_run(vec![display.to_string()])
    .with_evidence(evidence)
}

struct ValidationCommand {
    program: &'static str,
    args: &'static [&'static str],
    display: &'static str,
}

fn select_validation_command(description: &str) -> ValidationCommand {
    let normalized = description.to_ascii_lowercase();

    if normalized.contains("clean")
        || normalized.contains("reset")
        || normalized.contains("remove")
        || normalized.contains("delete")
    {
        return ValidationCommand {
            program: "git",
            args: &["clean", "-fd"],
            display: "git clean -fd",
        };
    }

    if normalized.contains("network")
        || normalized.contains("download")
        || normalized.contains("fetch")
    {
        return ValidationCommand {
            program: "curl",
            args: &["-I", "https://example.com"],
            display: "curl -I https://example.com",
        };
    }

    if normalized.contains("lint") || normalized.contains("clippy") {
        return ValidationCommand {
            program: "cargo",
            args: &["clippy", "--no-deps"],
            display: "cargo clippy --no-deps",
        };
    }

    if normalized.contains("format") || normalized.contains("fmt") {
        return ValidationCommand {
            program: "cargo",
            args: &["fmt", "--", "--check"],
            display: "cargo fmt -- --check",
        };
    }

    if normalized.contains("build") {
        return ValidationCommand {
            program: "cargo",
            args: &["build"],
            display: "cargo build",
        };
    }

    if normalized.contains("test")
        || normalized.contains("check")
        || normalized.contains("validate")
    {
        return ValidationCommand {
            program: "cargo",
            args: &["check"],
            display: "cargo check",
        };
    }

    ValidationCommand {
        program: "cargo",
        args: &["--version"],
        display: "cargo --version",
    }
}
