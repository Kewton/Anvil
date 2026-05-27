use std::collections::HashSet;
use std::path::Path;

use super::auto_test::{
    AutoTestPlan, AutoTestResult, AutoTestRunner, OwnedTestVerifierPlan, VerifierCommand,
};
use super::failure_packet::FailurePacketTimeoutKind;
use super::project_probe::ProjectUnit;
use super::task_contract::SafeStopReason;
use super::task_workspace_scope::TaskWorkspaceScope;

/// Normalized outcome of one task-contract verifier invocation.
///
/// This module owns result normalization only. It deliberately does not run
/// commands, mutate agent state, emit events, or decide actor-loop control
/// flow; those side effects stay in `turn.rs` while the verifier driver is
/// extracted behind smaller boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TaskContractVerifierOutcome {
    Passed {
        command: String,
    },
    Failed {
        command: String,
        output: String,
    },
    NoVerifier,
    Disabled,
    TransportError {
        error: String,
    },
    /// Structured-runner SafeStop. Produced when a verifier cannot be safely
    /// bound to the task-owned test artifacts.
    SafeStop {
        reason: SafeStopReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TaskContractVerifierSelection {
    StructuredRunnable {
        plan: AutoTestPlan,
        command: VerifierCommand,
        display_command: String,
        bound_test_artifacts_count: usize,
        bound_test_artifacts_paths: Vec<String>,
    },
    StructuredWeak {
        detected_source: &'static str,
        owned_test_artifacts_count: usize,
    },
    StructuredMissing {
        outcome: TaskContractVerifierOutcome,
        owned_test_artifacts_count: usize,
    },
    LegacyRunnable {
        plan: AutoTestPlan,
        command_for_log: String,
    },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierExternalImportContamination {
    pub(super) detected_count: usize,
    pub(super) truncated: bool,
    pub(super) hashed_entries: Vec<(String, &'static str)>,
}

impl VerifierExternalImportContamination {
    pub(super) fn borrowed_hashes(&self) -> Vec<(&str, &'static str)> {
        self.hashed_entries
            .iter()
            .map(|(hash, source)| (hash.as_str(), *source))
            .collect()
    }

    pub(super) fn to_failure_outcome(
        &self,
        result: AutoTestResult,
        created_markers: &[String],
    ) -> TaskContractVerifierOutcome {
        let marker_note = if created_markers.is_empty() {
            String::new()
        } else {
            format!(
                "\nCreated local Python package marker(s) to keep imports inside work_root: {}",
                created_markers.join(", ")
            )
        };
        TaskContractVerifierOutcome::Failed {
            command: result.command,
            output: format!(
                "Verifier environment contamination detected: {} external import path(s) outside work_root.{}\n{}",
                self.detected_count, marker_note, result.output
            ),
        }
    }
}

pub(super) fn task_contract_auto_test_result_to_outcome(
    result: AutoTestResult,
) -> TaskContractVerifierOutcome {
    if result.passed {
        TaskContractVerifierOutcome::Passed {
            command: result.command,
        }
    } else {
        TaskContractVerifierOutcome::Failed {
            command: result.command,
            output: result.output,
        }
    }
}

pub(super) fn detect_verifier_external_import_contamination(
    work_root: &Path,
    result: &AutoTestResult,
) -> Option<VerifierExternalImportContamination> {
    let detected = super::auto_test::detect_external_imports_in_output(
        work_root,
        &result.stdout,
        &result.stderr,
    );
    if detected.entries.is_empty() {
        return None;
    }
    let hashed_entries = detected
        .entries
        .iter()
        .map(|raw| {
            (
                crate::logging::stable_path_hash(&crate::session::feedback::mask_secrets(raw)),
                "stdout_stderr",
            )
        })
        .collect();
    Some(VerifierExternalImportContamination {
        detected_count: detected.total_count,
        truncated: detected.truncated,
        hashed_entries,
    })
}

pub(super) fn select_task_contract_verifier(
    work_root: &Path,
    changed_files: &[String],
    recent_successful_bash_commands: &[String],
    owned_test_artifacts: &[String],
    test_execution_required: bool,
    workspace_scope: Option<&TaskWorkspaceScope>,
    project_unit: Option<&ProjectUnit>,
) -> TaskContractVerifierSelection {
    if test_execution_required && workspace_scope.is_some() {
        let owned_plan = AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
            work_root,
            changed_files,
            recent_successful_bash_commands,
            owned_test_artifacts,
            project_unit,
        );
        return structured_selection_from_owned_plan(owned_plan, owned_test_artifacts.len());
    }

    let Some(plan) = AutoTestRunner::detect_with_project_unit(
        work_root,
        changed_files,
        recent_successful_bash_commands,
        project_unit,
    ) else {
        return TaskContractVerifierSelection::Missing;
    };
    let command_for_log = crate::session::feedback::mask_secrets(&plan.command);
    TaskContractVerifierSelection::LegacyRunnable {
        plan,
        command_for_log,
    }
}

fn structured_selection_from_owned_plan(
    owned_plan: OwnedTestVerifierPlan,
    owned_test_artifacts_count: usize,
) -> TaskContractVerifierSelection {
    match owned_plan {
        OwnedTestVerifierPlan::Runnable { plan, command } => {
            let display_command = command.to_display_string();
            let bound_test_artifacts_count = command.bound_test_artifacts().len();
            let bound_test_artifacts_paths = command.bound_test_artifacts().to_vec();
            TaskContractVerifierSelection::StructuredRunnable {
                plan,
                command,
                display_command,
                bound_test_artifacts_count,
                bound_test_artifacts_paths,
            }
        }
        OwnedTestVerifierPlan::Weak {
            detected_source, ..
        } => TaskContractVerifierSelection::StructuredWeak {
            detected_source,
            owned_test_artifacts_count,
        },
        OwnedTestVerifierPlan::Missing => TaskContractVerifierSelection::StructuredMissing {
            outcome: task_contract_structured_missing_outcome(owned_test_artifacts_count),
            owned_test_artifacts_count,
        },
    }
}

pub(super) fn task_contract_verifier_transport_error_to_outcome(
    command: String,
    error: String,
) -> TaskContractVerifierOutcome {
    if let Some(output) = verifier_timeout_failure_output(&command, &error) {
        TaskContractVerifierOutcome::Failed { command, output }
    } else {
        TaskContractVerifierOutcome::TransportError { error }
    }
}

pub(super) fn task_contract_structured_missing_outcome(
    owned_test_artifacts_count: usize,
) -> TaskContractVerifierOutcome {
    if owned_test_artifacts_count == 0 {
        TaskContractVerifierOutcome::SafeStop {
            reason: SafeStopReason::VerifierMissing,
        }
    } else {
        TaskContractVerifierOutcome::NoVerifier
    }
}

pub(super) fn select_task_contract_project_unit(
    work_root: &Path,
    active_request: Option<&str>,
    workspace_scope: Option<&TaskWorkspaceScope>,
    edited_files: &HashSet<String>,
) -> Option<ProjectUnit> {
    let scope = workspace_scope?;
    match active_request {
        Some(request) => super::project_probe::probe_project_unit_for_request(
            work_root,
            request,
            scope,
            edited_files,
        ),
        None => super::project_probe::probe_project_unit(work_root, scope, edited_files),
    }
}

pub(super) fn task_contract_verifier_outcome_label(
    outcome: &TaskContractVerifierOutcome,
) -> &'static str {
    match outcome {
        TaskContractVerifierOutcome::Failed { .. } => "verifier_timeout",
        TaskContractVerifierOutcome::TransportError { .. } => "transport_error",
        _ => "transport_error",
    }
}

pub(super) fn run_structured_task_contract_verifier(
    work_root: &Path,
    workspace_scope: &TaskWorkspaceScope,
    command: &VerifierCommand,
    display_command: &str,
) -> Result<AutoTestResult, String> {
    AutoTestRunner::run_structured(work_root, workspace_scope, command, display_command)
}

pub(super) fn run_legacy_task_contract_verifier(
    work_root: &Path,
    plan: &AutoTestPlan,
) -> Result<AutoTestResult, String> {
    AutoTestRunner::run(work_root, plan)
}

fn verifier_timeout_failure_output(command: &str, error: &str) -> Option<String> {
    let kind = classify_verifier_timeout(command, error)?;
    let masked_command = crate::session::feedback::redact_verifier_command_for_storage(command);
    let masked_error = crate::session::feedback::mask_secrets(error);
    Some(format!(
        "Verifier execution timed out before producing a pass/fail result. \
This is a bounded verifier timeout, not an LLM transport failure. \
timeout_kind={} command={} next_action_hint={}\n{}",
        kind.as_str(),
        masked_command,
        kind.repair_hint(),
        masked_error
    ))
}

pub(super) fn classify_verifier_timeout(
    command: &str,
    error: &str,
) -> Option<FailurePacketTimeoutKind> {
    if !error.contains("auto test command timed out after") {
        return None;
    }
    let lower_command = command.to_ascii_lowercase();
    let lower_error = error.to_ascii_lowercase();
    if verifier_timeout_is_dependency_setup(&lower_command, &lower_error) {
        return Some(FailurePacketTimeoutKind::DependencySetupTimeout);
    }
    if verifier_timeout_is_environment_stall(&lower_error) {
        return Some(FailurePacketTimeoutKind::EnvironmentStall);
    }
    if verifier_command_is_build(&lower_command) {
        return Some(FailurePacketTimeoutKind::BuildCommand);
    }
    if verifier_command_is_test_artifact_run(&lower_command) {
        return Some(FailurePacketTimeoutKind::GeneratedTestHang);
    }
    if verifier_command_is_test_like(&lower_command) {
        return Some(FailurePacketTimeoutKind::LongRunningVerifier);
    }
    Some(FailurePacketTimeoutKind::Unknown)
}

fn verifier_timeout_is_dependency_setup(lower_command: &str, lower_error: &str) -> bool {
    lower_error.contains("dependency setup") || lower_command.contains("pip install")
}

fn verifier_timeout_is_environment_stall(lower_error: &str) -> bool {
    ["environment", "external_pythonpath", "stalled"]
        .iter()
        .any(|marker| lower_error.contains(marker))
}

fn verifier_command_is_build(lower_command: &str) -> bool {
    [
        "cargo build",
        "npm run build",
        "pnpm build",
        "yarn build",
        "make build",
        "cargo check",
    ]
    .iter()
    .any(|prefix| lower_command.starts_with(prefix))
}

fn verifier_command_is_test_artifact_run(lower_command: &str) -> bool {
    [
        "--test ",
        " tests/",
        " tests\\",
        "-m pytest",
        "npm test",
        "node --test",
    ]
    .iter()
    .any(|marker| lower_command.contains(marker))
}

fn verifier_command_is_test_like(lower_command: &str) -> bool {
    lower_command.contains("test") || lower_command.contains("pytest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::auto_test::AutoTestResult;
    use tempfile::tempdir;

    fn auto_test_result(command: &str, passed: bool, output: &str) -> AutoTestResult {
        AutoTestResult {
            command: command.to_string(),
            passed,
            output: output.to_string(),
            exit_code: Some(if passed { 0 } else { 1 }),
            stdout: String::new(),
            stderr: output.to_string(),
        }
    }

    #[test]
    fn auto_test_result_maps_pass_and_failure_without_side_effects() {
        assert_eq!(
            task_contract_auto_test_result_to_outcome(auto_test_result("cargo test", true, "")),
            TaskContractVerifierOutcome::Passed {
                command: "cargo test".to_string()
            }
        );
        assert_eq!(
            task_contract_auto_test_result_to_outcome(auto_test_result(
                "python3 -m pytest",
                false,
                "FAILED tests/test_main.py"
            )),
            TaskContractVerifierOutcome::Failed {
                command: "python3 -m pytest".to_string(),
                output: "FAILED tests/test_main.py".to_string()
            }
        );
    }

    #[test]
    fn external_import_contamination_builds_masked_failure_outcome() {
        let result = auto_test_result(
            "python3 -B -m pytest",
            false,
            "ImportError: cannot import name 'app' from '/external/repo/app/main.py'",
        );
        let contamination = VerifierExternalImportContamination {
            detected_count: 1,
            truncated: false,
            hashed_entries: vec![("hash-1".to_string(), "stdout_stderr")],
        };
        let borrowed = contamination.borrowed_hashes();
        assert_eq!(borrowed, vec![("hash-1", "stdout_stderr")]);

        let outcome = contamination.to_failure_outcome(result, &["app/__init__.py".to_string()]);
        match outcome {
            TaskContractVerifierOutcome::Failed { command, output } => {
                assert_eq!(command, "python3 -B -m pytest");
                assert!(output.contains(
                    "Verifier environment contamination detected: 1 external import path(s) outside work_root."
                ));
                assert!(output.contains(
                    "Created local Python package marker(s) to keep imports inside work_root: app/__init__.py"
                ));
                assert!(output.contains("ImportError"));
            }
            other => panic!("expected contamination failure outcome, got {other:?}"),
        }
    }

    #[test]
    fn external_import_contamination_detection_hashes_raw_paths() {
        let dir = tempdir().unwrap();
        let mut result = auto_test_result(
            "python3 -B -m pytest",
            false,
            "ImportError: cannot import name 'app' from '/external/repo/app/main.py'",
        );
        result.stderr = result.output.clone();

        let contamination =
            detect_verifier_external_import_contamination(dir.path(), &result).unwrap();

        assert_eq!(contamination.detected_count, 1);
        assert!(!contamination.truncated);
        assert_eq!(contamination.hashed_entries.len(), 1);
        assert_eq!(contamination.hashed_entries[0].1, "stdout_stderr");
        assert_ne!(
            contamination.hashed_entries[0].0,
            "/external/repo/app/main.py"
        );
    }

    #[test]
    fn timeout_transport_error_becomes_repairable_verifier_failure() {
        let outcome = task_contract_verifier_transport_error_to_outcome(
            "python3 -m pytest tests/test_main.py".to_string(),
            "auto test command timed out after 30s".to_string(),
        );
        match outcome {
            TaskContractVerifierOutcome::Failed { output, .. } => {
                assert!(output.contains("timeout_kind=generated_test_hang"));
                assert!(output.contains("next_action_hint="));
            }
            other => panic!("expected verifier failure, got {other:?}"),
        }
    }

    #[test]
    fn non_timeout_transport_error_remains_transport_error() {
        let outcome = task_contract_verifier_transport_error_to_outcome(
            "cargo test".to_string(),
            "connection refused".to_string(),
        );
        assert_eq!(
            outcome,
            TaskContractVerifierOutcome::TransportError {
                error: "connection refused".to_string()
            }
        );
    }

    #[test]
    fn outcome_label_is_stable_for_transport_logging() {
        assert_eq!(
            task_contract_verifier_outcome_label(&TaskContractVerifierOutcome::Failed {
                command: "pytest".to_string(),
                output: "timeout".to_string(),
            }),
            "verifier_timeout"
        );
        assert_eq!(
            task_contract_verifier_outcome_label(&TaskContractVerifierOutcome::TransportError {
                error: "connection refused".to_string(),
            }),
            "transport_error"
        );
        assert_eq!(
            task_contract_verifier_outcome_label(&TaskContractVerifierOutcome::NoVerifier),
            "transport_error"
        );
    }

    #[test]
    fn structured_missing_distinguishes_no_owned_tests_from_unbound_runner() {
        assert_eq!(
            task_contract_structured_missing_outcome(0),
            TaskContractVerifierOutcome::SafeStop {
                reason: SafeStopReason::VerifierMissing
            }
        );
        assert_eq!(
            task_contract_structured_missing_outcome(1),
            TaskContractVerifierOutcome::NoVerifier
        );
    }

    #[test]
    fn project_unit_selection_requires_scope() {
        let edited_files = HashSet::new();
        assert_eq!(
            select_task_contract_project_unit(
                Path::new("/tmp/nonexistent"),
                Some("add tests"),
                None,
                &edited_files
            ),
            None
        );
    }

    #[test]
    fn verifier_selection_uses_structured_missing_when_tests_required_without_owned_tests() {
        let dir = tempdir().unwrap();
        let scope = TaskWorkspaceScope::detect(dir.path(), "run tests");
        assert_eq!(
            select_task_contract_verifier(dir.path(), &[], &[], &[], true, Some(&scope), None),
            TaskContractVerifierSelection::StructuredMissing {
                outcome: TaskContractVerifierOutcome::SafeStop {
                    reason: SafeStopReason::VerifierMissing
                },
                owned_test_artifacts_count: 0
            }
        );
    }

    #[test]
    fn verifier_selection_without_candidates_is_missing() {
        let dir = tempdir().unwrap();
        assert_eq!(
            select_task_contract_verifier(dir.path(), &[], &[], &[], false, None, None),
            TaskContractVerifierSelection::Missing
        );
    }
}
