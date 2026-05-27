use std::path::Path;

use serde_json::Value;

use super::tool_policy::workspace_relative_path_for_tool_arg;
use crate::util::workspace_paths::is_ignored_workspace_display_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepoEditOperation {
    Write,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepoEditEvidence {
    raw_path: String,
    relative_path: Option<String>,
    operation: RepoEditOperation,
    artifact_evidence_allowed: bool,
}

impl RepoEditEvidence {
    pub(super) fn raw_path(&self) -> &str {
        &self.raw_path
    }

    #[allow(dead_code)]
    pub(super) fn relative_path(&self) -> Option<&str> {
        self.relative_path.as_deref()
    }

    #[allow(dead_code)]
    pub(super) fn operation(&self) -> RepoEditOperation {
        self.operation
    }

    pub(super) fn artifact_evidence_allowed(&self) -> bool {
        self.artifact_evidence_allowed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadEvidence {
    #[allow(dead_code)]
    raw_path: Option<String>,
    #[allow(dead_code)]
    relative_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandEvidence {
    #[allow(dead_code)]
    command_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolRejection {
    #[allow(dead_code)]
    tool_name: String,
    #[allow(dead_code)]
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolFailure {
    #[allow(dead_code)]
    tool_name: String,
    #[allow(dead_code)]
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NoopReason {
    UnknownToolKind,
    MissingPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolExecutionOutcome {
    RepoEdit(RepoEditEvidence),
    Read(ReadEvidence),
    Command(CommandEvidence),
    Rejected(ToolRejection),
    Failed(ToolFailure),
    Noop(NoopReason),
}

impl ToolExecutionOutcome {
    pub(super) fn repo_edit_evidence(&self) -> Option<&RepoEditEvidence> {
        match self {
            Self::RepoEdit(evidence) => Some(evidence),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(super) fn satisfies_artifact_edit_evidence(&self) -> bool {
        self.repo_edit_evidence()
            .is_some_and(RepoEditEvidence::artifact_evidence_allowed)
    }
}

pub(super) fn success_outcome_for_call(
    name: &str,
    arguments: &Value,
    work_root: &Path,
) -> ToolExecutionOutcome {
    match name {
        "Write" | "Edit" => repo_edit_success_outcome(name, arguments, work_root),
        "Read" => read_success_outcome(arguments, work_root),
        "Glob" | "Grep" => ToolExecutionOutcome::Read(ReadEvidence {
            raw_path: None,
            relative_path: None,
        }),
        "Bash" => ToolExecutionOutcome::Command(CommandEvidence {
            command_present: arguments
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| !command.trim().is_empty()),
        }),
        _ => ToolExecutionOutcome::Noop(NoopReason::UnknownToolKind),
    }
}

pub(super) fn rejected_outcome_for_call(name: &str, reason: &str) -> ToolExecutionOutcome {
    ToolExecutionOutcome::Rejected(ToolRejection {
        tool_name: name.to_string(),
        reason: reason.to_string(),
    })
}

pub(super) fn failed_outcome_for_call(name: &str, error: &str) -> ToolExecutionOutcome {
    ToolExecutionOutcome::Failed(ToolFailure {
        tool_name: name.to_string(),
        error: error.to_string(),
    })
}

fn repo_edit_success_outcome(
    name: &str,
    arguments: &Value,
    work_root: &Path,
) -> ToolExecutionOutcome {
    let Some(raw_path) = arguments.get("path").and_then(Value::as_str) else {
        return ToolExecutionOutcome::Noop(NoopReason::MissingPath);
    };
    let relative_path = workspace_relative_path_for_tool_arg(work_root, raw_path);
    let artifact_evidence_allowed = relative_path
        .as_deref()
        .is_some_and(|relative| !is_ignored_workspace_display_path(relative));
    ToolExecutionOutcome::RepoEdit(RepoEditEvidence {
        raw_path: raw_path.to_string(),
        relative_path,
        operation: if name == "Write" {
            RepoEditOperation::Write
        } else {
            RepoEditOperation::Edit
        },
        artifact_evidence_allowed,
    })
}

fn read_success_outcome(arguments: &Value, work_root: &Path) -> ToolExecutionOutcome {
    let raw_path = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string);
    let relative_path = raw_path
        .as_deref()
        .and_then(|path| workspace_relative_path_for_tool_arg(work_root, path));
    ToolExecutionOutcome::Read(ReadEvidence {
        raw_path,
        relative_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn read_only_tool_does_not_satisfy_artifact_edit_evidence() {
        let tmp = TempDir::new().unwrap();
        let outcome =
            success_outcome_for_call("Read", &json!({ "path": "src/main.rs" }), tmp.path());

        assert!(!outcome.satisfies_artifact_edit_evidence());
        assert!(matches!(outcome, ToolExecutionOutcome::Read(_)));
    }

    #[test]
    fn repo_edit_satisfies_artifact_evidence_for_workspace_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        let outcome =
            success_outcome_for_call("Write", &json!({ "path": "src/main.rs" }), tmp.path());

        let evidence = outcome.repo_edit_evidence().unwrap();
        assert_eq!(evidence.raw_path(), "src/main.rs");
        assert_eq!(evidence.relative_path(), Some("src/main.rs"));
        assert_eq!(evidence.operation(), RepoEditOperation::Write);
        assert!(outcome.satisfies_artifact_edit_evidence());
    }

    #[test]
    fn controller_owned_state_is_repo_edit_but_not_artifact_evidence() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".anvil-state")).unwrap();
        let outcome = success_outcome_for_call(
            "Edit",
            &json!({ "path": ".anvil-state/session.json" }),
            tmp.path(),
        );

        let evidence = outcome.repo_edit_evidence().unwrap();
        assert_eq!(evidence.operation(), RepoEditOperation::Edit);
        assert!(!evidence.artifact_evidence_allowed());
        assert!(!outcome.satisfies_artifact_edit_evidence());
    }

    #[test]
    fn command_output_does_not_satisfy_artifact_edit_evidence() {
        let tmp = TempDir::new().unwrap();
        let outcome =
            success_outcome_for_call("Bash", &json!({ "command": "cargo test" }), tmp.path());

        assert!(matches!(outcome, ToolExecutionOutcome::Command(_)));
        assert!(!outcome.satisfies_artifact_edit_evidence());
    }

    #[test]
    fn rejected_and_failed_outcomes_are_typed() {
        assert!(matches!(
            rejected_outcome_for_call("Write", "out of scope"),
            ToolExecutionOutcome::Rejected(_)
        ));
        assert!(matches!(
            failed_outcome_for_call("Edit", "missing old_string"),
            ToolExecutionOutcome::Failed(_)
        ));
    }
}
