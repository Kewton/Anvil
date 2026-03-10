use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::exec::ExecRequest;
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct TesterAgent;

impl TesterAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        let command = select_validation_command(&task.description);
        match runtime.checked_execute(ToolRequest::Exec {
            request: ExecRequest {
                program: command.program.to_string(),
                args: command.args.iter().map(|arg| arg.to_string()).collect(),
                cwd: task.workspace_root.clone(),
            },
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::ExecResult(result))) => {
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
                        command.display, result.exit_code, task.description
                    ),
                )
                .with_next_recommendation(
                    "Inspect the validation output and decide whether another focused check is needed",
                )
                .with_commands_run(vec![command.display.to_string()])
                .with_evidence(evidence)
            }
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

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
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
