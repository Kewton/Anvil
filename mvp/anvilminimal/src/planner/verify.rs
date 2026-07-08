use std::path::Path;

use crate::minimal_loop::build_verifier::{
    self, BuildVerifierLifecycleObservation, BuildVerifierStatus, CompileError,
};
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::verifier_env;
use crate::planner::step_plan::{ExpectedResult, PlanStep};
use crate::tools::path_guard::{resolve_existing, validate_workspace_relative};
use crate::{
    eval_events,
    tools::bash::{BashOutcome, BashOutcomeKind},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyStatus {
    Pass,
    MissingPath(String),
    CommandFailed(String),
    VerifierCommandFalseNegative(String),
    DependencyMissing(String),
    ProfileContractFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub status: VerifyStatus,
    pub missing_paths: Vec<String>,
    pub command_failures: Vec<CommandFailure>,
    pub verifier_command_false_negatives: Vec<VerifierCommandFalseNegative>,
    pub runtime_command_normalizations: Vec<VerifyCommandRuntimeNormalization>,
    pub dependency_missing: Vec<String>,
    pub profile_failures: Vec<String>,
    pub compile_errors: Vec<CompileError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFailure {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifierCommandFalseNegative {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifyCommandRuntimeNormalization {
    pub original: String,
    pub repaired: String,
}

pub const OUTPUT_PIPE_STRIPPED_REASON: &str = "output_pipe_stripped: verifier output is already captured and excerpted; trailing head/tail pipes mask the base command exit status";
const STDERR_MERGE_STRIPPED_REASON: &str =
    "stderr_merge_stripped: verifier output already captures stderr; trailing 2>&1 is redundant";
const EXIT_CODE_ECHO_STRIPPED_REASON: &str = "exit_code_echo_stripped: trailing exit-code echo masks the base command exit status; verifier already records status";
const FALLBACK_TRUE_STRIPPED_REASON: &str =
    "fallback_true_stripped: trailing `|| true` masks the base command exit status";
const SUCCESS_FAILURE_ECHO_STRIPPED_REASON: &str = "success_failure_echo_stripped: verifier exit status already records pass/fail; trailing echo branches mask the base command";
const WORKSPACE_CD_NORMALIZED_REASON: &str =
    "workspace_cd_normalized: absolute workspace cd rewritten to workspace-relative verifier form";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyCommandViolationKind {
    Empty,
    Blocked,
    ShellControlSyntax,
    SetupOrDevServer,
    WorkspaceEscape,
    GrepDashPattern,
    PackageJsonScriptGrep,
    OutputPipeStripped,
    StderrMergeStripped,
    ExitCodeEchoStripped,
    FallbackTrueStripped,
    SuccessFailureEchoStripped,
    WorkspaceCdNormalized,
}

impl VerifyCommandViolationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Blocked => "blocked",
            Self::ShellControlSyntax => "shell_control_syntax",
            Self::SetupOrDevServer => "setup_or_dev_server",
            Self::WorkspaceEscape => "workspace_escape",
            Self::GrepDashPattern => "grep_dash_pattern",
            Self::PackageJsonScriptGrep => "package_json_script_grep",
            Self::OutputPipeStripped => "output_pipe_stripped",
            Self::StderrMergeStripped => "stderr_merge_stripped",
            Self::ExitCodeEchoStripped => "exit_code_echo_stripped",
            Self::FallbackTrueStripped => "fallback_true_stripped",
            Self::SuccessFailureEchoStripped => "success_failure_echo_stripped",
            Self::WorkspaceCdNormalized => "workspace_cd_normalized",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "verify command is empty",
            Self::Blocked => "verify command is blocked",
            Self::ShellControlSyntax => {
                "verify command may not use shell control syntax; allowed alternatives: use one deterministic command such as `npm run build`, `cargo test`, `python -m compileall -q src`, or `test -f relative/path`; split multiple checks into separate verify commands"
            }
            Self::SetupOrDevServer => {
                "verify command may not perform setup or start a dev server; allowed alternatives: put dependency setup in a setup step, then verify with `npm run build` or `test -f relative/path`"
            }
            Self::WorkspaceEscape => {
                "verify command manifest path escapes workspace; allowed alternative: use workspace-relative paths such as `test -f src/app/page.tsx` or `cd app && npm run build`"
            }
            Self::GrepDashPattern => "grep pattern begins with '-' but command lacks `--` or `-e`",
            Self::PackageJsonScriptGrep => {
                "grep package.json script assertion should use JSON parser"
            }
            Self::OutputPipeStripped => "verify command output-limiting pipe should be stripped",
            Self::StderrMergeStripped => "verify command stderr merge should be stripped",
            Self::ExitCodeEchoStripped => "verify command exit-code echo should be stripped",
            Self::FallbackTrueStripped => "verify command fallback true should be stripped",
            Self::SuccessFailureEchoStripped => {
                "verify command success/failure echo branches should be stripped"
            }
            Self::WorkspaceCdNormalized => {
                "verify command absolute workspace cd should be normalized"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCommandDiagnosis {
    pub normalized: String,
    pub violation: Option<VerifyCommandViolationKind>,
    pub reason: Option<String>,
}

/// A verifier command admitted by the shared normalization pipeline.
///
/// Raw strings cannot be passed to verifier execution boundaries:
///
/// ```compile_fail
/// use anvilminimal::minimal_loop::verifier_env::run_checked;
///
/// let raw = "npm run build";
/// let root = std::path::Path::new(".");
/// let _ = run_checked(raw, root, false);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedVerifyCommand(String);

impl NormalizedVerifyCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }

    fn new(normalized: String) -> Self {
        Self(normalized)
    }
}

impl std::fmt::Display for NormalizedVerifyCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl VerificationReport {
    pub fn pass() -> Self {
        Self {
            status: VerifyStatus::Pass,
            missing_paths: Vec::new(),
            command_failures: Vec::new(),
            verifier_command_false_negatives: Vec::new(),
            runtime_command_normalizations: Vec::new(),
            dependency_missing: Vec::new(),
            profile_failures: Vec::new(),
            compile_errors: Vec::new(),
        }
    }

    pub fn is_pass(&self) -> bool {
        self.status == VerifyStatus::Pass
            && self.missing_paths.is_empty()
            && self.command_failures.is_empty()
            && self.verifier_command_false_negatives.is_empty()
            && self.dependency_missing.is_empty()
            && self.profile_failures.is_empty()
            && self.compile_errors.is_empty()
    }

    pub fn missing_path(path: impl Into<String>) -> Self {
        let mut report = Self::pass();
        report.push_missing_path(path);
        report
    }

    pub fn command_failed(command: impl Into<String>, reason: impl Into<String>) -> Self {
        let mut report = Self::pass();
        report.push_command_failure(command, reason);
        report
    }

    pub fn dependency_missing(reason: impl Into<String>) -> Self {
        let mut report = Self::pass();
        report.push_dependency_missing(reason);
        report
    }

    pub fn profile_failed(reason: impl Into<String>) -> Self {
        let mut report = Self::pass();
        report.push_profile_failure(reason);
        report
    }

    pub fn push_missing_path(&mut self, path: impl Into<String>) {
        self.missing_paths.push(path.into());
        self.refresh_status();
    }

    pub fn push_command_failure(&mut self, command: impl Into<String>, reason: impl Into<String>) {
        self.command_failures.push(CommandFailure {
            command: command.into(),
            reason: reason.into(),
        });
        self.refresh_status();
    }

    pub fn push_verifier_command_false_negative(
        &mut self,
        command: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.verifier_command_false_negatives
            .push(VerifierCommandFalseNegative {
                command: command.into(),
                reason: reason.into(),
            });
        self.refresh_status();
    }

    fn push_runtime_command_normalization(
        &mut self,
        original: impl Into<String>,
        repaired: impl Into<String>,
    ) {
        self.runtime_command_normalizations
            .push(VerifyCommandRuntimeNormalization {
                original: original.into(),
                repaired: repaired.into(),
            });
    }

    pub fn push_dependency_missing(&mut self, reason: impl Into<String>) {
        self.dependency_missing.push(reason.into());
        self.refresh_status();
    }

    pub fn push_profile_failure(&mut self, reason: impl Into<String>) {
        self.profile_failures.push(reason.into());
        self.refresh_status();
    }

    pub fn push_compile_errors(&mut self, command: impl Into<String>, errors: Vec<CompileError>) {
        if errors.is_empty() {
            return;
        }
        for error in &errors {
            if !self.compile_errors.contains(error) {
                self.compile_errors.push(error.clone());
            }
        }
        let reason = format!(
            "implementation_compile_error: {}",
            errors
                .iter()
                .map(CompileError::summary)
                .collect::<Vec<_>>()
                .join("; ")
        );
        self.push_command_failure(command, reason);
    }

    pub fn primary_reason(&self) -> String {
        self.missing_paths
            .first()
            .cloned()
            .or_else(|| self.dependency_missing.first().cloned())
            .or_else(|| {
                self.command_failures
                    .first()
                    .map(|failure| failure.reason.clone())
            })
            .or_else(|| {
                self.verifier_command_false_negatives
                    .first()
                    .map(|failure| failure.reason.clone())
            })
            .or_else(|| {
                self.compile_errors
                    .first()
                    .map(|error| format!("implementation_compile_error: {}", error.summary()))
            })
            .or_else(|| self.profile_failures.first().cloned())
            .unwrap_or_else(|| "pass".to_string())
    }

    pub fn refresh_status(&mut self) {
        self.status = if let Some(path) = self.missing_paths.first() {
            VerifyStatus::MissingPath(path.clone())
        } else if let Some(reason) = self.dependency_missing.first() {
            VerifyStatus::DependencyMissing(reason.clone())
        } else if let Some(failure) = self.command_failures.first() {
            VerifyStatus::CommandFailed(failure.reason.clone())
        } else if let Some(failure) = self.verifier_command_false_negatives.first() {
            VerifyStatus::VerifierCommandFalseNegative(failure.reason.clone())
        } else if let Some(error) = self.compile_errors.first() {
            VerifyStatus::CommandFailed(format!(
                "implementation_compile_error: {}",
                error.summary()
            ))
        } else if let Some(reason) = self.profile_failures.first() {
            VerifyStatus::ProfileContractFailed(reason.clone())
        } else {
            VerifyStatus::Pass
        };
    }
}

pub fn verify_step(root: &Path, step: &PlanStep) -> VerificationReport {
    verify_step_with_setup(root, step, NodeDependencySetupAuthority::None)
}

pub fn verify_step_with_setup(
    root: &Path,
    step: &PlanStep,
    setup_authority: NodeDependencySetupAuthority,
) -> VerificationReport {
    verify_step_with_setup_observed(root, step, setup_authority).0
}

pub fn verify_step_with_setup_observed(
    root: &Path,
    step: &PlanStep,
    setup_authority: NodeDependencySetupAuthority,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_step_with_setup_observed_with_offline(root, step, setup_authority, false)
}

pub fn verify_step_with_setup_observed_with_offline(
    root: &Path,
    step: &PlanStep,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_step_with_setup_observed_with_offline_and_events(
        root,
        step,
        setup_authority,
        offline,
        None,
    )
}

pub fn verify_step_with_profile_setup_observed_with_offline(
    root: &Path,
    step: &PlanStep,
    profile: Option<&str>,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_step_with_profile_setup_observed_with_offline_and_events(
        root,
        step,
        profile,
        setup_authority,
        offline,
        None,
    )
}

pub fn verify_step_with_setup_observed_with_offline_and_events(
    root: &Path,
    step: &PlanStep,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
    eval_events_path: Option<&Path>,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_step_with_profile_setup_observed_with_offline_and_events(
        root,
        step,
        None,
        setup_authority,
        offline,
        eval_events_path,
    )
}

pub fn verify_step_with_profile_setup_observed_with_offline_and_events(
    root: &Path,
    step: &PlanStep,
    profile: Option<&str>,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
    eval_events_path: Option<&Path>,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_step_with_setup_observed_with_options(
        root,
        step,
        profile,
        setup_authority,
        Path::new("npm"),
        offline,
        eval_events_path,
    )
}

pub fn verify_setup_dependency_state_with_setup_observed_with_offline(
    root: &Path,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_setup_dependency_state_with_setup_observed_with_options(
        root,
        setup_authority,
        Path::new("npm"),
        offline,
    )
}

#[cfg(test)]
pub(crate) fn verify_setup_dependency_state_with_setup_observed_with_options(
    root: &Path,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_setup_dependency_state_with_setup_observed_inner(
        root,
        setup_authority,
        npm_program,
        offline,
    )
}

#[cfg(not(test))]
pub(crate) fn verify_setup_dependency_state_with_setup_observed_with_options(
    root: &Path,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    verify_setup_dependency_state_with_setup_observed_inner(
        root,
        setup_authority,
        npm_program,
        offline,
    )
}

fn verify_setup_dependency_state_with_setup_observed_inner(
    root: &Path,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    let mut report = VerificationReport::pass();
    let mut build_lifecycles = Vec::new();
    if let Some(requirement) = build_verifier::requirement_from_dependency_state(
        root,
        "test -d node_modules",
        None,
        "setup step completed with declared dependencies but missing node_modules",
        setup_authority.as_str(),
        "required",
    ) {
        let lifecycle =
            build_verifier::observe_requirement_lifecycle_with_setup_program_and_offline(
                root,
                &requirement,
                setup_authority,
                npm_program,
                offline,
            );
        record_build_lifecycle_result(&mut report, &requirement.command, &lifecycle);
        build_lifecycles.push(lifecycle);
    }
    (report, build_lifecycles)
}

fn verify_step_with_setup_observed_with_options(
    root: &Path,
    step: &PlanStep,
    profile: Option<&str>,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
    eval_events_path: Option<&Path>,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    let mut report = VerificationReport::pass();
    let mut build_lifecycles = Vec::new();
    for path in &step.expected_paths {
        if resolve_existing(root, path).is_err() {
            report.push_missing_path(path.clone());
        }
    }
    for command in &step.verify {
        let normalized_command = match normalize_verify_command(command) {
            Ok(command) => command,
            Err(err) => {
                report.push_command_failure(command.clone(), err.to_string());
                continue;
            }
        };
        let command = normalized_command.as_str();
        if let Some(requirement) = build_verifier::requirement_from_deferred(
            command,
            build_verifier_profile(profile, command),
            "step verify requires build lifecycle",
            setup_authority.as_str(),
            "required",
        ) {
            let lifecycle =
                build_verifier::observe_requirement_lifecycle_with_setup_program_and_offline(
                    root,
                    &requirement,
                    setup_authority,
                    npm_program,
                    offline,
                );
            let passed = record_build_lifecycle_result(&mut report, command, &lifecycle);
            build_lifecycles.push(lifecycle);
            if passed {
                continue;
            }
            continue;
        }
        if setup_authority == NodeDependencySetupAuthority::PlanSetupStep
            && let Some(requirement) = build_verifier::requirement_from_dependency_state(
                root,
                command,
                build_verifier_profile(profile, command),
                "step verify requires dependency setup before command execution",
                setup_authority.as_str(),
                "required",
            )
        {
            let lifecycle =
                build_verifier::observe_requirement_lifecycle_with_setup_program_and_offline(
                    root,
                    &requirement,
                    setup_authority,
                    npm_program,
                    offline,
                );
            let passed = record_build_lifecycle_result(&mut report, command, &lifecycle);
            build_lifecycles.push(lifecycle);
            if passed {
                continue;
            }
            continue;
        }
        match run_verify_command_with_runtime_oracle(
            &normalized_command,
            root,
            profile,
            false,
            eval_events_path,
        ) {
            VerifyCommandRunResult::Passed {
                output,
                normalization,
            } => {
                if let Some(normalization) = normalization {
                    report.push_runtime_command_normalization(
                        normalization.original,
                        normalization.repaired,
                    );
                }
                if step.expected_result_kind() == ExpectedResult::Fail {
                    report.push_command_failure(
                        command.to_string(),
                        "expected command to fail but it passed",
                    );
                } else if command.contains("npm") && output.contains("0 tests") {
                    report.push_command_failure(command.to_string(), "Node 0 tests rejected");
                }
            }
            VerifyCommandRunResult::Failed {
                command: failed_command,
                reason,
            } => {
                if build_verifier::is_dependency_missing_output(&reason) {
                    if setup_authority.allows_setup() {
                        let requirement =
                            build_verifier::requirement_from_dependency_missing_output(
                                command,
                                build_verifier_profile(profile, command),
                                "verify command failed with dependency-missing output",
                                setup_authority.as_str(),
                                "required",
                            );
                        let lifecycle =
                            build_verifier::observe_dependency_missing_output_lifecycle_with_setup_program_and_offline(
                                root,
                                &requirement,
                                setup_authority,
                                &reason,
                                npm_program,
                                offline,
                            );
                        let passed =
                            record_build_lifecycle_result(&mut report, command, &lifecycle);
                        build_lifecycles.push(lifecycle);
                        if passed {
                            continue;
                        }
                    } else {
                        report.push_dependency_missing(format!(
                            "dependency_setup_authority_required: {command}"
                        ));
                    }
                } else if step.expected_result_kind() != ExpectedResult::Fail {
                    report.push_command_failure(failed_command, reason);
                }
            }
            VerifyCommandRunResult::FalseNegative {
                command: failed_command,
                reason,
            } => {
                if step.expected_result_kind() != ExpectedResult::Fail {
                    report.push_verifier_command_false_negative(failed_command, reason);
                }
            }
        }
    }
    (report, build_lifecycles)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeCommandNormalization {
    original: String,
    repaired: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerifyCommandRunResult {
    Passed {
        output: String,
        normalization: Option<RuntimeCommandNormalization>,
    },
    Failed {
        command: String,
        reason: String,
    },
    FalseNegative {
        command: String,
        reason: String,
    },
}

fn run_verify_command_with_runtime_oracle(
    command: &NormalizedVerifyCommand,
    root: &Path,
    profile: Option<&str>,
    offline: bool,
    eval_events_path: Option<&Path>,
) -> VerifyCommandRunResult {
    match verifier_env::run_structured_for_verify_with_profile(command, root, profile, offline) {
        Ok(outcome) if outcome.is_success() => VerifyCommandRunResult::Passed {
            output: verifier_env::format_verify_outcome(&outcome),
            normalization: None,
        },
        Ok(outcome) => {
            handle_failed_verify_command(command, root, profile, offline, eval_events_path, outcome)
        }
        Err(err) => VerifyCommandRunResult::Failed {
            command: command.as_str().to_string(),
            reason: format!("failed to run verifier command: {err}"),
        },
    }
}

fn handle_failed_verify_command(
    command: &NormalizedVerifyCommand,
    root: &Path,
    profile: Option<&str>,
    offline: bool,
    eval_events_path: Option<&Path>,
    outcome: BashOutcome,
) -> VerifyCommandRunResult {
    let command_text = command.as_str();
    let formatted = verifier_env::format_verify_outcome(&outcome);
    if outcome.kind == BashOutcomeKind::Timeout {
        return handle_verify_command_timeout(
            command,
            root,
            profile,
            offline,
            eval_events_path,
            &formatted,
            outcome.elapsed_ms,
        );
    }
    if !is_verify_command_tool_usage_error(command_text, &outcome) {
        return VerifyCommandRunResult::Failed {
            command: command_text.to_string(),
            reason: format!("command failed: {command_text}\n{formatted}"),
        };
    }
    let Some(repair) = normalize_verify_command_for_oracle_repair(command_text) else {
        return VerifyCommandRunResult::FalseNegative {
            command: command_text.to_string(),
            reason: verify_command_false_negative_reason(command_text, &formatted),
        };
    };
    if repair.normalized == command_text {
        return VerifyCommandRunResult::FalseNegative {
            command: command_text.to_string(),
            reason: verify_command_false_negative_reason(command_text, &formatted),
        };
    }
    let repaired_command = match normalize_verify_command(&repair.normalized) {
        Ok(command) => command,
        Err(err) => {
            return VerifyCommandRunResult::FalseNegative {
                command: repair.normalized,
                reason: verify_command_false_negative_reason(command_text, &err.to_string()),
            };
        }
    };
    match verifier_env::run_structured_for_verify_with_profile(
        &repaired_command,
        root,
        profile,
        offline,
    ) {
        Ok(repaired_outcome) if repaired_outcome.is_success() => {
            eval_events::emit(
                eval_events_path,
                serde_json::json!({
                    "event": "verify_command_normalized_at_runtime",
                    "classification": "verify_command_false_negative_candidate",
                    "normalization_source": repair.kind,
                    "original": eval_events::body_snippet(command_text),
                    "repaired": eval_events::body_snippet(repaired_command.as_str()),
                }),
            );
            VerifyCommandRunResult::Passed {
                output: verifier_env::format_verify_outcome(&repaired_outcome),
                normalization: Some(RuntimeCommandNormalization {
                    original: command_text.to_string(),
                    repaired: repaired_command.into_string(),
                }),
            }
        }
        Ok(repaired_outcome) => {
            let repaired_formatted = verifier_env::format_verify_outcome(&repaired_outcome);
            if is_verify_command_tool_usage_error(repaired_command.as_str(), &repaired_outcome) {
                VerifyCommandRunResult::FalseNegative {
                    command: repaired_command.into_string(),
                    reason: verify_command_false_negative_reason(command_text, &repaired_formatted),
                }
            } else {
                VerifyCommandRunResult::Failed {
                    command: repaired_command.as_str().to_string(),
                    reason: format!(
                        "command failed: {}\n{}",
                        repaired_command.as_str(),
                        repaired_formatted
                    ),
                }
            }
        }
        Err(err) => VerifyCommandRunResult::FalseNegative {
            command: repaired_command.into_string(),
            reason: verify_command_false_negative_reason(command_text, &err.to_string()),
        },
    }
}

fn handle_verify_command_timeout(
    command: &NormalizedVerifyCommand,
    root: &Path,
    profile: Option<&str>,
    offline: bool,
    eval_events_path: Option<&Path>,
    formatted: &str,
    elapsed_ms: u128,
) -> VerifyCommandRunResult {
    let command_text = command.as_str();
    let substitution = verify_timeout_substitution(command_text, root, profile);
    if let Some(substitution) = substitution {
        eval_events::emit(
            eval_events_path,
            serde_json::json!({
                "event": "verify_command_timeout",
                "classification": "OracleError",
                "repair_target": "verifier_command",
                "command": eval_events::body_snippet(command_text),
                "elapsed_ms": elapsed_ms.min(u128::from(u64::MAX)) as u64,
                "guidance": "the verify command hangs - replace it with a bounded check",
                "substitution_attempted": true,
                "substitution_command": eval_events::body_snippet(substitution.as_str()),
            }),
        );
        match verifier_env::run_structured_for_verify_with_profile(
            &substitution,
            root,
            profile,
            offline,
        ) {
            Ok(substitution_outcome) if substitution_outcome.is_success() => {
                eval_events::emit(
                    eval_events_path,
                    serde_json::json!({
                        "event": "verify_command_timeout_substitution",
                        "classification": "OracleError",
                        "original": eval_events::body_snippet(command_text),
                        "substitution": eval_events::body_snippet(substitution.as_str()),
                        "status": "passed",
                    }),
                );
                return VerifyCommandRunResult::Passed {
                    output: verifier_env::format_verify_outcome(&substitution_outcome),
                    normalization: Some(RuntimeCommandNormalization {
                        original: command_text.to_string(),
                        repaired: substitution.into_string(),
                    }),
                };
            }
            Ok(substitution_outcome) => {
                let substitution_formatted =
                    verifier_env::format_verify_outcome(&substitution_outcome);
                return VerifyCommandRunResult::FalseNegative {
                    command: command.to_string(),
                    reason: verify_command_timeout_reason(
                        command_text,
                        formatted,
                        Some(substitution.as_str()),
                        Some(&substitution_formatted),
                    ),
                };
            }
            Err(err) => {
                return VerifyCommandRunResult::FalseNegative {
                    command: command.to_string(),
                    reason: verify_command_timeout_reason(
                        command_text,
                        formatted,
                        Some(substitution.as_str()),
                        Some(&err.to_string()),
                    ),
                };
            }
        }
    }
    eval_events::emit(
        eval_events_path,
        serde_json::json!({
            "event": "verify_command_timeout",
            "classification": "OracleError",
            "repair_target": "verifier_command",
            "command": eval_events::body_snippet(command_text),
            "elapsed_ms": elapsed_ms.min(u128::from(u64::MAX)) as u64,
            "guidance": "the verify command hangs - replace it with a bounded check",
            "substitution_attempted": false,
        }),
    );
    VerifyCommandRunResult::FalseNegative {
        command: command_text.to_string(),
        reason: verify_command_timeout_reason(command_text, formatted, None, None),
    }
}

fn verify_command_timeout_reason(
    command: &str,
    tool_error: &str,
    substitution: Option<&str>,
    substitution_error: Option<&str>,
) -> String {
    let mut reason = format!(
        "OracleError: verify_command_timeout:{command}: the verify command hangs - replace it with a bounded check; tool_error={}",
        eval_events::body_snippet(tool_error)
    );
    if let Some(substitution) = substitution {
        reason.push_str("; substitution_attempted=");
        reason.push_str(substitution);
    }
    if let Some(error) = substitution_error {
        reason.push_str("; substitution_error=");
        reason.push_str(&eval_events::body_snippet(error));
    }
    reason
}

fn verify_timeout_substitution(
    command: &str,
    root: &Path,
    profile: Option<&str>,
) -> Option<NormalizedVerifyCommand> {
    if profile != Some("python-cli") || !is_pytest_verify_command(command) {
        return None;
    }
    root.join("src")
        .is_dir()
        .then(|| normalize_verify_command("python -m compileall -q src").ok())
        .flatten()
}

fn is_pytest_verify_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "pytest"
        || lower.starts_with("pytest ")
        || lower == "python -m pytest"
        || lower.starts_with("python -m pytest ")
        || lower == "python3 -m pytest"
        || lower.starts_with("python3 -m pytest ")
}

fn verify_command_false_negative_reason(command: &str, tool_error: &str) -> String {
    format!(
        "verify_command_false_negative: the verify command is malformed; the artifact may already satisfy the requirement; command=`{}`; tool_error={}",
        command,
        eval_events::body_snippet(tool_error)
    )
}

fn is_verify_command_tool_usage_error(command: &str, outcome: &BashOutcome) -> bool {
    if grep_command_name(command).is_some() && outcome_exit_code(outcome) == Some(2) {
        return true;
    }
    stderr_has_tool_usage_signature(&outcome.stderr)
}

fn stderr_has_tool_usage_signature(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("invalid option")
        || lower.contains("unrecognized option")
        || lower.contains("unknown option")
        || lower
            .lines()
            .any(|line| line.trim_start().starts_with("usage:"))
}

fn outcome_exit_code(outcome: &BashOutcome) -> Option<i32> {
    outcome
        .status
        .as_deref()?
        .split_whitespace()
        .next_back()?
        .parse()
        .ok()
}

fn record_build_lifecycle_result(
    report: &mut VerificationReport,
    command: &str,
    lifecycle: &BuildVerifierLifecycleObservation,
) -> bool {
    let observation = lifecycle.final_observation();
    match observation.status {
        BuildVerifierStatus::Passed => true,
        BuildVerifierStatus::DependencyMissing => {
            report.push_dependency_missing(format!(
                "dependency_setup_missing: {}",
                dependency_lifecycle_report_reason(lifecycle)
            ));
            false
        }
        BuildVerifierStatus::PolicyRejected => {
            report.push_command_failure(
                command.to_string(),
                format!("build_verify_policy_rejected: {}", lifecycle.final_reason),
            );
            false
        }
        BuildVerifierStatus::Blocked => {
            report.push_profile_failure(format!(
                "build_verify_blocked: command `{}` reason `{}`",
                command, lifecycle.final_reason
            ));
            false
        }
        BuildVerifierStatus::Failed => {
            let compile_errors = lifecycle.final_observation().compile_errors.clone();
            if compile_errors.is_empty() {
                report.push_command_failure(
                    command.to_string(),
                    format!(
                        "dependency_setup_lifecycle_failed: {}",
                        lifecycle_failure_with_setup_output(lifecycle)
                    ),
                );
            } else {
                report.push_compile_errors(command.to_string(), compile_errors);
            }
            false
        }
    }
}

fn dependency_lifecycle_report_reason(lifecycle: &BuildVerifierLifecycleObservation) -> String {
    if let Some(setup) = lifecycle.setup.as_ref()
        && setup.primary_reason == "dependency_setup_blocked_offline"
    {
        return setup.primary_reason.clone();
    }
    if let Some(setup) = lifecycle.setup.as_ref()
        && matches!(setup.status.as_str(), "failed" | "timed_out")
    {
        let mut reason = setup.primary_reason.clone();
        if !setup.output_snippet.trim().is_empty() {
            reason.push_str("; setup_output: ");
            reason.push_str(&setup.output_snippet);
        }
        return reason;
    }
    lifecycle.final_reason.clone()
}

fn lifecycle_failure_with_setup_output(lifecycle: &BuildVerifierLifecycleObservation) -> String {
    let mut reason = lifecycle.final_reason.clone();
    if let Some(setup) = lifecycle.setup.as_ref()
        && !setup.output_snippet.trim().is_empty()
    {
        reason.push_str("; setup_output: ");
        reason.push_str(&setup.output_snippet);
    }
    reason
}

pub fn validate_verify_command(command: &str) -> anyhow::Result<()> {
    normalize_verify_command(command).map(|_| ())
}

pub fn normalize_verify_command(command: &str) -> anyhow::Result<NormalizedVerifyCommand> {
    let diagnosis = diagnose_verify_command(command);
    if let Some(violation) = diagnosis.violation {
        if matches!(
            violation,
            VerifyCommandViolationKind::GrepDashPattern
                | VerifyCommandViolationKind::PackageJsonScriptGrep
                | VerifyCommandViolationKind::OutputPipeStripped
                | VerifyCommandViolationKind::StderrMergeStripped
                | VerifyCommandViolationKind::ExitCodeEchoStripped
                | VerifyCommandViolationKind::FallbackTrueStripped
                | VerifyCommandViolationKind::SuccessFailureEchoStripped
                | VerifyCommandViolationKind::WorkspaceCdNormalized
        ) {
            return Ok(NormalizedVerifyCommand::new(diagnosis.normalized));
        }
        anyhow::bail!(
            "{}",
            diagnosis
                .reason
                .unwrap_or_else(|| violation.message().to_string())
        );
    }
    Ok(NormalizedVerifyCommand::new(diagnosis.normalized))
}

pub fn normalize_planner_verify_command(command: &str) -> anyhow::Result<Vec<String>> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if !contains_shell_control_syntax(trimmed) {
        return Ok(vec![normalize_verify_command(trimmed)?.into_string()]);
    }
    normalize_planner_shell_and_verify_command(trimmed)
}

pub fn diagnose_verify_command(command: &str) -> VerifyCommandDiagnosis {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return verify_command_violation(normalized, VerifyCommandViolationKind::Empty, None);
    }
    if crate::tools::bash::blocked_reason(&normalized, false).is_some() {
        return verify_command_violation(normalized, VerifyCommandViolationKind::Blocked, None);
    }
    if let Some(repair) = normalize_verify_command_for_oracle_repair(&normalized) {
        let violation = match repair.kind {
            "package_json_script_assertion" => VerifyCommandViolationKind::PackageJsonScriptGrep,
            "output_pipe_stripped" => VerifyCommandViolationKind::OutputPipeStripped,
            "stderr_merge_stripped" => VerifyCommandViolationKind::StderrMergeStripped,
            "exit_code_echo_stripped" => VerifyCommandViolationKind::ExitCodeEchoStripped,
            "fallback_true_stripped" => VerifyCommandViolationKind::FallbackTrueStripped,
            "success_failure_echo_stripped" => {
                VerifyCommandViolationKind::SuccessFailureEchoStripped
            }
            "workspace_cd_normalized" => VerifyCommandViolationKind::WorkspaceCdNormalized,
            _ => VerifyCommandViolationKind::GrepDashPattern,
        };
        return verify_command_violation(repair.normalized, violation, Some(repair.reason));
    }
    let lower = normalized.to_ascii_lowercase();
    if is_setup_or_dev_server_verify_command(&lower) {
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::SetupOrDevServer,
            None,
        );
    }
    if contains_shell_control_syntax(&normalized) {
        if let Some(diagnosis) = diagnose_leading_cd_verify_command(&normalized) {
            return diagnosis;
        }
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::ShellControlSyntax,
            None,
        );
    }
    if is_lone_cd_command(&normalized) {
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::ShellControlSyntax,
            Some("verify command may not be a standalone directory change".to_string()),
        );
    }
    if let Some(path) = manifest_path_arg(&normalized)
        && let Err(err) = validate_workspace_relative(path)
    {
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::WorkspaceEscape,
            Some(err.to_string()),
        );
    }
    VerifyCommandDiagnosis {
        normalized,
        violation: None,
        reason: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCommandOracleRepair {
    pub normalized: String,
    pub reason: String,
    pub kind: &'static str,
}

pub fn normalize_verify_command_for_oracle_repair(
    command: &str,
) -> Option<VerifyCommandOracleRepair> {
    if let Some(stripped) = strip_output_truncation_pipe(command) {
        return Some(VerifyCommandOracleRepair {
            normalized: stripped,
            reason: OUTPUT_PIPE_STRIPPED_REASON.to_string(),
            kind: "output_pipe_stripped",
        });
    }
    if let Some(stripped) = strip_exit_code_echo(command) {
        return Some(VerifyCommandOracleRepair {
            normalized: stripped,
            reason: EXIT_CODE_ECHO_STRIPPED_REASON.to_string(),
            kind: "exit_code_echo_stripped",
        });
    }
    if let Some(stripped) = strip_fallback_true(command) {
        return Some(VerifyCommandOracleRepair {
            normalized: stripped,
            reason: FALLBACK_TRUE_STRIPPED_REASON.to_string(),
            kind: "fallback_true_stripped",
        });
    }
    if let Some(stripped) = strip_success_failure_echo(command) {
        return Some(VerifyCommandOracleRepair {
            normalized: stripped,
            reason: SUCCESS_FAILURE_ECHO_STRIPPED_REASON.to_string(),
            kind: "success_failure_echo_stripped",
        });
    }
    if let Some(stripped) = strip_redundant_stderr_merge(command) {
        return Some(VerifyCommandOracleRepair {
            normalized: stripped,
            reason: STDERR_MERGE_STRIPPED_REASON.to_string(),
            kind: "stderr_merge_stripped",
        });
    }
    let tokens = shell_words_with_spans(command)?;
    if let Some(script_check) = package_json_script_grep_check_command(&tokens) {
        return Some(VerifyCommandOracleRepair {
            normalized: script_check,
            reason: "grep package.json script assertion replaced with JSON parser check"
                .to_string(),
            kind: "package_json_script_assertion",
        });
    }
    let grep = grep_dash_pattern(&tokens)?;
    Some(VerifyCommandOracleRepair {
        normalized: insert_grep_separator(command, tokens[grep.pattern_index].start),
        reason: "grep pattern begins with '-' but command lacks `--` or `-e`".to_string(),
        kind: "grep_dash_pattern_separator",
    })
}

pub fn normalize_verify_command_for_oracle_repair_with_root(
    command: &str,
    root: &Path,
) -> Option<VerifyCommandOracleRepair> {
    let mut current = command.trim().to_string();
    if current.is_empty() {
        return None;
    }
    let mut reasons = Vec::new();
    let mut kinds = Vec::new();
    for _ in 0..8 {
        if let Some(repair) = normalize_verify_command_for_oracle_repair(&current)
            && repair.normalized != current
        {
            current = repair.normalized;
            reasons.push(repair.reason);
            kinds.push(repair.kind);
            continue;
        }
        if let Some(repair) = normalize_workspace_cd_verify_command(&current, root)
            && repair.normalized != current
        {
            current = repair.normalized;
            reasons.push(repair.reason);
            kinds.push(repair.kind);
            continue;
        }
        break;
    }
    if current == command.trim() {
        return None;
    }
    let kind = kinds
        .last()
        .copied()
        .unwrap_or("mechanical_verify_normalized");
    Some(VerifyCommandOracleRepair {
        normalized: current,
        reason: reasons.join("; "),
        kind,
    })
}

pub fn normalize_workspace_cd_verify_command(
    command: &str,
    root: &Path,
) -> Option<VerifyCommandOracleRepair> {
    let trimmed = command.trim();
    let (cd_part, verify_part) = split_once_outside_quotes_sequence(trimmed, "&&")?;
    if find_outside_quotes_sequence(verify_part, "&&").is_some() {
        return None;
    }
    let cd_tokens = shell_words_with_spans(cd_part.trim())?;
    if cd_tokens.len() != 2 || cd_tokens[0].value != "cd" {
        return None;
    }
    let cd_path = Path::new(&cd_tokens[1].value);
    if !cd_path.is_absolute() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let cd_path = cd_path.canonicalize().ok()?;
    let relative = cd_path.strip_prefix(&root).ok()?;
    let verify = verify_part.trim();
    if verify.is_empty() {
        return None;
    }
    let normalized = if relative.as_os_str().is_empty() {
        verify.to_string()
    } else {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.chars().any(char::is_whitespace) {
            return None;
        }
        format!("cd {relative} && {verify}")
    };
    Some(VerifyCommandOracleRepair {
        normalized: normalized.split_whitespace().collect::<Vec<_>>().join(" "),
        reason: WORKSPACE_CD_NORMALIZED_REASON.to_string(),
        kind: "workspace_cd_normalized",
    })
}

pub fn strip_output_truncation_pipe(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let pipe_index = single_unquoted_pipe_index(trimmed)?;
    let base = trimmed[..pipe_index].trim_end();
    let limiter = trimmed[(pipe_index + 1)..].trim();
    if base.is_empty() || !is_output_limiter_command(limiter) {
        return None;
    }
    let stripped = strip_trailing_stderr_merge(base).trim();
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn strip_exit_code_echo(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let (base, suffix) = split_last_outside_quotes(trimmed, ';')?;
    if !is_exit_code_echo(suffix.trim()) {
        return None;
    }
    let stripped = strip_trailing_stderr_merge(base.trim()).trim();
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn strip_fallback_true(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let (base, suffix) = split_once_outside_quotes_sequence(trimmed, "||")?;
    if suffix.trim() != "true" {
        return None;
    }
    let stripped = strip_trailing_stderr_merge(base.trim()).trim();
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn strip_success_failure_echo(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let (success_part, failure_part) = split_once_outside_quotes_sequence(trimmed, "||")?;
    if !is_plain_status_echo(failure_part.trim(), &["fail", "failed", "false"]) {
        return None;
    }
    let (base, success_echo) = split_once_outside_quotes_sequence(success_part.trim(), "&&")?;
    if !is_plain_status_echo(
        success_echo.trim(),
        &["pass", "passed", "success", "ok", "true"],
    ) {
        return None;
    }
    let base = strip_trailing_stderr_merge(base.trim()).trim();
    if base.is_empty() {
        return None;
    }
    Some(base.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_plain_status_echo(command: &str, allowed: &[&str]) -> bool {
    let Some(tokens) = shell_words_with_spans(command) else {
        return false;
    };
    if tokens.len() != 2 || tokens[0].value != "echo" {
        return false;
    }
    let value = tokens[1].value.to_ascii_lowercase();
    allowed.iter().any(|allowed| value == *allowed)
}

fn strip_redundant_stderr_merge(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let stripped = strip_trailing_stderr_merge(trimmed).trim();
    if stripped == trimmed || stripped.is_empty() {
        return None;
    }
    Some(stripped.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_exit_code_echo(command: &str) -> bool {
    let Some(tokens) = shell_words_with_spans(command) else {
        return false;
    };
    if tokens.len() != 2 || tokens[0].value != "echo" {
        return false;
    }
    matches!(
        tokens[1].value.as_str(),
        "EXIT_CODE=$?" | "exit_code=$?" | "status=$?" | "STATUS=$?"
    )
}

fn single_unquoted_pipe_index(command: &str) -> Option<usize> {
    let mut found = None;
    let mut single = false;
    let mut double = false;
    for (index, ch) in command.char_indices() {
        match ch {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '|' if !single && !double => {
                if found.is_some() {
                    return None;
                }
                found = Some(index);
            }
            _ => {}
        }
    }
    if single || double { None } else { found }
}

fn split_last_outside_quotes(text: &str, needle: char) -> Option<(&str, &str)> {
    let mut found = None;
    let mut single = false;
    let mut double = false;
    for (index, ch) in text.char_indices() {
        match ch {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ if ch == needle && !single && !double => found = Some(index),
            _ => {}
        }
    }
    let index = found?;
    Some((&text[..index], &text[index + needle.len_utf8()..]))
}

fn split_once_outside_quotes_sequence<'a>(
    text: &'a str,
    needle: &str,
) -> Option<(&'a str, &'a str)> {
    let index = find_outside_quotes_sequence(text, needle)?;
    Some((&text[..index], &text[index + needle.len()..]))
}

fn find_outside_quotes_sequence(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' if !double => {
                single = !single;
                index += 1;
            }
            b'"' if !single => {
                double = !double;
                index += 1;
            }
            _ if !single && !double && bytes[index..].starts_with(needle_bytes) => {
                return Some(index);
            }
            _ => index += 1,
        }
    }
    None
}

fn strip_trailing_stderr_merge(base: &str) -> &str {
    let trimmed = base.trim_end();
    let Some(prefix) = trimmed.strip_suffix("2>&1") else {
        return trimmed;
    };
    if prefix
        .chars()
        .next_back()
        .is_some_and(|ch| !ch.is_whitespace())
    {
        return trimmed;
    }
    prefix.trim_end()
}

fn is_output_limiter_command(command: &str) -> bool {
    let Some(tokens) = shell_words_with_spans(command) else {
        return false;
    };
    match tokens.as_slice() {
        [program, count] => is_head_or_tail(&program.value) && is_line_count(&count.value),
        [program, flag, count] => {
            is_head_or_tail(&program.value) && flag.value == "-n" && is_line_count(&count.value)
        }
        _ => false,
    }
}

fn is_head_or_tail(program: &str) -> bool {
    matches!(program, "head" | "tail")
}

fn is_line_count(value: &str) -> bool {
    let digits = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellWord {
    value: String,
    start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepDashPattern {
    pattern_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepPattern {
    pattern_index: usize,
    pattern: String,
}

fn shell_words_with_spans(command: &str) -> Option<Vec<ShellWord>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start = None;
    let mut single = false;
    let mut double = false;
    for (index, ch) in command.char_indices() {
        if single {
            if ch == '\'' {
                single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if double {
            if ch == '"' {
                double = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() {
            if let Some(word_start) = start.take() {
                out.push(ShellWord {
                    value: std::mem::take(&mut current),
                    start: word_start,
                });
            }
            continue;
        }
        if start.is_none() {
            start = Some(index);
        }
        match ch {
            '\'' => single = true,
            '"' => double = true,
            _ => current.push(ch),
        }
    }
    if single || double {
        return None;
    }
    if let Some(word_start) = start {
        out.push(ShellWord {
            value: current,
            start: word_start,
        });
    }
    Some(out)
}

fn grep_dash_pattern(tokens: &[ShellWord]) -> Option<GrepDashPattern> {
    let program = tokens.first()?.value.as_str();
    grep_command_name(program)?;
    let mut index = 1usize;
    while index < tokens.len() {
        let value = tokens[index].value.as_str();
        if value == "--" || grep_arg_is_explicit_pattern_flag(value) {
            return None;
        }
        if grep_arg_is_known_option(value) {
            index += grep_option_arity(value);
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            return Some(GrepDashPattern {
                pattern_index: index,
            });
        }
        return None;
    }
    None
}

fn grep_pattern(tokens: &[ShellWord]) -> Option<GrepPattern> {
    let program = tokens.first()?.value.as_str();
    grep_command_name(program)?;
    let mut index = 1usize;
    while index < tokens.len() {
        let value = tokens[index].value.as_str();
        if value == "--" {
            let pattern = tokens.get(index + 1)?;
            return Some(GrepPattern {
                pattern_index: index + 1,
                pattern: pattern.value.clone(),
            });
        }
        if value == "-e" || value == "--regexp" {
            let pattern = tokens.get(index + 1)?;
            return Some(GrepPattern {
                pattern_index: index + 1,
                pattern: pattern.value.clone(),
            });
        }
        if let Some(pattern) = value.strip_prefix("-e").filter(|value| !value.is_empty()) {
            return Some(GrepPattern {
                pattern_index: index,
                pattern: pattern.to_string(),
            });
        }
        if let Some(pattern) = value
            .strip_prefix("--regexp=")
            .filter(|value| !value.is_empty())
        {
            return Some(GrepPattern {
                pattern_index: index,
                pattern: pattern.to_string(),
            });
        }
        if grep_arg_is_known_option(value) {
            index += grep_option_arity(value);
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            return None;
        }
        return Some(GrepPattern {
            pattern_index: index,
            pattern: value.to_string(),
        });
    }
    None
}

fn grep_command_name(command: &str) -> Option<&'static str> {
    match command.split_whitespace().next().unwrap_or(command) {
        "grep" => Some("grep"),
        "egrep" => Some("egrep"),
        "fgrep" => Some("fgrep"),
        _ => None,
    }
}

fn grep_arg_is_explicit_pattern_flag(value: &str) -> bool {
    value == "-e"
        || value.starts_with("-e")
        || value == "--regexp"
        || value.starts_with("--regexp=")
}

fn grep_arg_is_known_option(value: &str) -> bool {
    if !value.starts_with('-') || value == "-" {
        return false;
    }
    if matches!(
        value,
        "-A" | "-B" | "-C" | "-m" | "-f" | "--file" | "--label"
    ) {
        return true;
    }
    if value.starts_with("--") {
        return matches!(
            value.split('=').next().unwrap_or(value),
            "--quiet"
                | "--silent"
                | "--ignore-case"
                | "--invert-match"
                | "--fixed-strings"
                | "--extended-regexp"
                | "--basic-regexp"
                | "--recursive"
                | "--line-number"
                | "--word-regexp"
                | "--line-regexp"
                | "--count"
                | "--files-with-matches"
                | "--files-without-match"
                | "--with-filename"
                | "--no-filename"
                | "--only-matching"
                | "--include"
                | "--exclude"
                | "--exclude-dir"
                | "--max-count"
                | "--after-context"
                | "--before-context"
                | "--context"
                | "--binary-files"
        );
    }
    let flags = value.trim_start_matches('-');
    !flags.is_empty()
        && flags.chars().all(|ch| {
            matches!(
                ch,
                'q' | 's'
                    | 'i'
                    | 'v'
                    | 'F'
                    | 'E'
                    | 'G'
                    | 'r'
                    | 'R'
                    | 'n'
                    | 'w'
                    | 'x'
                    | 'c'
                    | 'l'
                    | 'L'
                    | 'h'
                    | 'H'
                    | 'o'
                    | 'a'
                    | 'I'
            )
        })
}

fn grep_option_arity(value: &str) -> usize {
    if matches!(
        value,
        "-A" | "-B" | "-C" | "-m" | "-f" | "--file" | "--label"
    ) {
        1
    } else {
        0
    }
}

fn insert_grep_separator(command: &str, pattern_start: usize) -> String {
    let mut out = String::new();
    out.push_str(command[..pattern_start].trim_end());
    out.push_str(" -- ");
    out.push_str(&command[pattern_start..]);
    out
}

fn package_json_arg_present(tokens: &[ShellWord]) -> bool {
    tokens
        .iter()
        .any(|token| matches!(token.value.as_str(), "package.json" | "./package.json"))
}

fn package_json_script_grep_check_command(tokens: &[ShellWord]) -> Option<String> {
    let grep = grep_pattern(tokens)?;
    if !package_json_arg_present(&tokens[(grep.pattern_index + 1)..]) {
        return None;
    }
    package_json_script_check_command(&grep.pattern)
}

fn package_json_script_check_command(pattern: &str) -> Option<String> {
    let lower = pattern.to_ascii_lowercase();
    let script = if lower.contains("next build") || lower.contains("scripts.build") {
        Some("build")
    } else if lower.contains("next dev")
        || lower.contains("scripts.dev")
        || lower.contains("\"dev\"")
        || lower.contains("'dev'")
    {
        Some("dev")
    } else if lower.contains("next start")
        || lower.contains("scripts.start")
        || lower.contains("\"start\"")
        || lower.contains("'start'")
    {
        Some("start")
    } else {
        None
    }?;
    if pattern.contains('\'') || pattern.contains('\\') {
        return None;
    }
    Some(format!(
        "node -p \"String(require('./package.json').scripts.{script}).includes('{pattern}') ? true : process.exit(1)\""
    ))
}

fn is_setup_or_dev_server_verify_command(lower: &str) -> bool {
    if lower.starts_with("node -p ") || lower.starts_with("node --print ") {
        return false;
    }
    lower.contains("npm install")
        || lower.contains("pnpm install")
        || lower.contains("yarn install")
        || lower.contains("cargo install")
        || lower.contains("npm run dev")
        || lower.contains("pnpm dev")
        || lower.contains("yarn dev")
        || lower.contains("next dev")
        || lower.contains("vite --host")
        || lower.contains("vite --port")
        || (lower.contains("curl ") && is_localhost_reference(lower))
        || (lower.contains("wget ") && is_localhost_reference(lower))
        || lower.contains("python -m http.server")
        || lower.contains("python3 -m http.server")
        || lower.contains("server start")
        || lower.contains("serve ")
}

fn is_localhost_reference(lower: &str) -> bool {
    lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0")
        || lower.contains("[::1]")
}

fn normalize_planner_shell_and_verify_command(command: &str) -> anyhow::Result<Vec<String>> {
    if has_unsupported_shell_control_for_planner_split(command) || !command.contains("&&") {
        anyhow::bail!(
            "{}",
            VerifyCommandViolationKind::ShellControlSyntax.message()
        );
    }
    let mut out = Vec::new();
    for part in command.split("&&") {
        let normalized = normalize_verify_command(part.trim())?;
        if !is_safe_split_verify_fragment(normalized.as_str()) {
            anyhow::bail!(
                "verify command shell split contains unsupported fragment: {}",
                normalized
            );
        }
        out.push(normalized.into_string());
    }
    if out.is_empty() {
        anyhow::bail!(
            "{}",
            VerifyCommandViolationKind::ShellControlSyntax.message()
        );
    }
    Ok(out)
}

fn has_unsupported_shell_control_for_planner_split(command: &str) -> bool {
    if command.contains("$(") {
        return true;
    }
    if command.bytes().any(|byte| {
        matches!(
            byte,
            b';' | b'|' | b'<' | b'>' | b'`' | b'\n' | b'\r' | b'\\'
        )
    }) {
        return true;
    }
    contains_single_ampersand(command)
}

fn contains_single_ampersand(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'&' {
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'&' {
            index += 2;
            continue;
        }
        return true;
    }
    false
}

fn is_safe_split_verify_fragment(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower == "npm test"
        || lower.starts_with("npm test ")
        || lower == "npm run test"
        || lower.starts_with("npm run test ")
        || lower == "npm run build"
        || lower.starts_with("npm run build ")
        || lower == "npm run lint"
        || lower.starts_with("npm run lint ")
        || lower == "npm run typecheck"
        || lower.starts_with("npm run typecheck ")
        || lower == "pnpm test"
        || lower.starts_with("pnpm test ")
        || lower == "pnpm build"
        || lower.starts_with("pnpm build ")
        || lower == "pnpm lint"
        || lower.starts_with("pnpm lint ")
        || lower == "yarn test"
        || lower.starts_with("yarn test ")
        || lower == "yarn build"
        || lower.starts_with("yarn build ")
        || lower == "yarn lint"
        || lower.starts_with("yarn lint ")
        || lower == "next build"
        || lower.starts_with("next build ")
        || lower == "cargo test"
        || lower.starts_with("cargo test ")
        || lower == "cargo check"
        || lower.starts_with("cargo check ")
        || lower == "cargo build"
        || lower.starts_with("cargo build ")
        || lower == "cargo fmt --check"
        || lower.starts_with("cargo fmt --check ")
        || lower == "pytest"
        || lower.starts_with("pytest ")
        || lower == "python -m pytest"
        || lower.starts_with("python -m pytest ")
        || lower == "python3 -m pytest"
        || lower.starts_with("python3 -m pytest ")
        || lower == "python -m unittest"
        || lower.starts_with("python -m unittest ")
        || lower == "python3 -m unittest"
        || lower.starts_with("python3 -m unittest ")
        || lower.starts_with("python -m py_compile ")
        || lower.starts_with("python3 -m py_compile ")
        || lower == "tsc --noemit"
        || lower.starts_with("tsc --noemit ")
        || lower == "npx tsc --noemit"
        || lower.starts_with("npx tsc --noemit ")
        || lower.starts_with("node --check ")
        || is_safe_test_path_command(command)
}

fn is_safe_test_path_command(command: &str) -> bool {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "test" || !matches!(parts[1], "-f" | "-d" | "-s") {
        return false;
    }
    validate_workspace_relative(parts[2]).is_ok()
}

fn verify_command_violation(
    normalized: String,
    violation: VerifyCommandViolationKind,
    reason: Option<String>,
) -> VerifyCommandDiagnosis {
    VerifyCommandDiagnosis {
        normalized,
        violation: Some(violation),
        reason: Some(reason.unwrap_or_else(|| violation.message().to_string())),
    }
}

fn diagnose_leading_cd_verify_command(command: &str) -> Option<VerifyCommandDiagnosis> {
    let (cd_part, verify_part) = split_once_outside_quotes(command, "&&")?;
    if find_outside_quotes(verify_part, "&&").is_some() {
        return None;
    }
    let cd_tokens = shell_words_with_spans(cd_part.trim())?;
    if cd_tokens.len() != 2 || cd_tokens[0].value != "cd" {
        return None;
    }
    let cd_path = cd_tokens[1].value.as_str();
    if let Err(err) = validate_workspace_relative(cd_path) {
        return Some(verify_command_violation(
            command.to_string(),
            VerifyCommandViolationKind::WorkspaceEscape,
            Some(err.to_string()),
        ));
    }
    if cd_path.chars().any(char::is_whitespace) {
        return None;
    }
    let verify = verify_part.trim();
    if verify.is_empty() {
        return Some(verify_command_violation(
            command.to_string(),
            VerifyCommandViolationKind::Empty,
            None,
        ));
    }
    let verify_diagnosis = diagnose_verify_command(verify);
    if verify_diagnosis.violation.is_some() {
        return Some(verify_diagnosis);
    }
    if !is_safe_split_verify_fragment(&verify_diagnosis.normalized) {
        return Some(verify_command_violation(
            command.to_string(),
            VerifyCommandViolationKind::ShellControlSyntax,
            Some(format!(
                "verify command shell split contains unsupported fragment: {}",
                verify_diagnosis.normalized
            )),
        ));
    }
    Some(VerifyCommandDiagnosis {
        normalized: format!("cd {cd_path} && {}", verify_diagnosis.normalized),
        violation: None,
        reason: None,
    })
}

fn is_lone_cd_command(command: &str) -> bool {
    let Some(tokens) = shell_words_with_spans(command) else {
        return false;
    };
    matches!(tokens.as_slice(), [program, _path] if program.value == "cd")
}

fn split_once_outside_quotes<'a>(text: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let index = find_outside_quotes(text, needle)?;
    Some((&text[..index], &text[(index + needle.len())..]))
}

fn find_outside_quotes(text: &str, needle: &str) -> Option<usize> {
    let needle_bytes = needle.as_bytes();
    let bytes = text.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' if !double => {
                single = !single;
                index += 1;
            }
            b'"' if !single => {
                double = !double;
                index += 1;
            }
            _ if !single && !double && bytes[index..].starts_with(needle_bytes) => {
                return Some(index);
            }
            _ => index += 1,
        }
    }
    None
}

fn contains_shell_control_syntax(command: &str) -> bool {
    command.bytes().any(|byte| {
        matches!(
            byte,
            b';' | b'&' | b'|' | b'<' | b'>' | b'`' | b'\n' | b'\r' | b'\\'
        )
    }) || command.contains("$(")
}

fn build_verifier_profile<'a>(profile: Option<&'a str>, command: &str) -> Option<&'a str> {
    if let Some(profile) = profile
        && crate::planner::profile::build_oracle_for_command(Some(profile), command).is_some()
    {
        return Some(profile);
    }
    let (_, oracle) = crate::planner::profile::build_oracle_for_command(None, command)?;
    match oracle.profile.as_deref() {
        Some("nextjs") => Some("nextjs"),
        _ => None,
    }
}

fn manifest_path_arg(command: &str) -> Option<&str> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    parts
        .windows(2)
        .find(|pair| pair[0] == "--manifest-path")
        .map(|pair| pair[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::repair_target::{RepairTarget, classify_repair_target};
    use crate::planner::step_plan::PlanStep;
    use std::path::Path;

    fn write_executable(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    }

    fn write_fake_next_build_workspace(root: &Path, npm_script: &str) {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/next")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("node_modules/next/package.json"), "{}").unwrap();
        write_executable(&root.join("node_modules/.bin/next"), "#!/bin/sh\nexit 0\n");
        write_executable(&root.join("node_modules/.bin/npm"), npm_script);
        std::fs::write(
            root.join("src/app/page.tsx"),
            "export default function Page(){return <main>plain route</main>;}\n",
        )
        .unwrap();
    }

    #[test]
    fn missing_path_before_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "work".to_string(),
            expected_result: "pass".to_string(),
            instruction: "x".to_string(),
            expected_paths: vec!["missing.txt".to_string()],
            verify: vec!["false".to_string()],
        };
        assert!(matches!(
            verify_step(dir.path(), &step).status,
            VerifyStatus::MissingPath(_)
        ));
    }

    #[test]
    fn rust_manifest_path_escape_rejected() {
        assert!(validate_verify_command("cargo test --manifest-path ../Cargo.toml").is_err());
    }

    #[test]
    fn verify_command_diagnoses_shell_control_syntax() {
        let diagnosis = diagnose_verify_command("npm test && npm run build");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::ShellControlSyntax)
        );
        let reason = diagnosis.reason.as_deref().unwrap_or_default();
        assert!(
            reason.starts_with("verify command may not use shell control syntax"),
            "{reason}"
        );
        assert!(
            reason.contains("split multiple checks into separate verify commands"),
            "{reason}"
        );
    }

    #[test]
    fn verify_command_diagnoses_setup_or_dev_server() {
        let diagnosis = diagnose_verify_command("next dev -p 3011");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::SetupOrDevServer)
        );
        let reason = diagnosis.reason.as_deref().unwrap_or_default();
        assert!(
            reason.starts_with("verify command may not perform setup or start a dev server"),
            "{reason}"
        );
        assert!(
            reason.contains("put dependency setup in a setup step"),
            "{reason}"
        );
    }

    #[test]
    fn verify_command_diagnoses_dev_server_probe_before_shell_syntax() {
        let diagnosis = diagnose_verify_command("npm run dev & curl http://localhost:3011");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::SetupOrDevServer)
        );
    }

    #[test]
    fn verify_command_diagnoses_empty_command() {
        let diagnosis = diagnose_verify_command("   ");
        assert_eq!(diagnosis.violation, Some(VerifyCommandViolationKind::Empty));
        assert_eq!(diagnosis.normalized, "");
    }

    #[test]
    fn verify_command_normalizes_safe_whitespace_only() {
        let normalized = normalize_verify_command("  cargo   test   --locked  ").unwrap();
        assert_eq!(normalized.as_str(), "cargo test --locked");
    }

    #[test]
    fn verify_command_normalizes_grep_dash_pattern() {
        let diagnosis = diagnose_verify_command(r#"grep -q "-p 3011" package.json"#);

        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::GrepDashPattern)
        );
        assert_eq!(diagnosis.normalized, r#"grep -q -- "-p 3011" package.json"#);
    }

    #[test]
    fn verify_command_prefers_json_parser_for_package_script_grep() {
        let diagnosis = diagnose_verify_command(r#"grep -q "next dev -p 3011" package.json"#);

        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::PackageJsonScriptGrep)
        );
        assert_eq!(
            diagnosis.normalized,
            r#"node -p "String(require('./package.json').scripts.dev).includes('next dev -p 3011') ? true : process.exit(1)""#
        );
    }

    #[test]
    fn verify_command_normalizes_output_truncation_pipe() {
        let diagnosis = diagnose_verify_command("npm run build 2>&1 | tail -80");

        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::OutputPipeStripped)
        );
        assert_eq!(diagnosis.normalized, "npm run build");
        assert_eq!(
            normalize_verify_command("npm run build | head -n 20")
                .unwrap()
                .as_str(),
            "npm run build"
        );
        assert_eq!(
            normalize_verify_command("npm run build | tail 80")
                .unwrap()
                .as_str(),
            "npm run build"
        );
        assert!(
            diagnosis
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("mask the base command exit status"),
            "{diagnosis:?}"
        );
    }

    #[test]
    fn verify_command_normalizes_stderr_exit_and_true_wrappers() {
        let stderr = diagnose_verify_command("npm run build 2>&1");
        assert_eq!(
            stderr.violation,
            Some(VerifyCommandViolationKind::StderrMergeStripped)
        );
        assert_eq!(stderr.normalized, "npm run build");

        let exit_echo =
            diagnose_verify_command(r#"python -m compileall -q src 2>&1; echo "EXIT_CODE=$?""#);
        assert_eq!(
            exit_echo.violation,
            Some(VerifyCommandViolationKind::ExitCodeEchoStripped)
        );
        assert_eq!(exit_echo.normalized, "python -m compileall -q src");

        let fallback = diagnose_verify_command(r#"python3 -c "print(min(10, None))" 2>&1 || true"#);
        assert_eq!(
            fallback.violation,
            Some(VerifyCommandViolationKind::FallbackTrueStripped)
        );
        assert_eq!(fallback.normalized, r#"python3 -c "print(min(10, None))""#);

        let status_echo =
            diagnose_verify_command(r#"test -f src/app/page.tsx && echo "pass" || echo "fail""#);
        assert_eq!(
            status_echo.violation,
            Some(VerifyCommandViolationKind::SuccessFailureEchoStripped)
        );
        assert_eq!(status_echo.normalized, "test -f src/app/page.tsx");
    }

    #[test]
    fn verify_command_normalizes_absolute_workspace_cd_with_root() {
        let dir = tempfile::tempdir().unwrap();
        let repair = normalize_verify_command_for_oracle_repair_with_root(
            &format!(
                "cd {} && npm run build 2>&1; echo \"EXIT_CODE=$?\"",
                dir.path().display()
            ),
            dir.path(),
        )
        .expect("root-aware repair");

        assert_eq!(repair.normalized, "npm run build");
        assert_eq!(repair.kind, "workspace_cd_normalized");
        assert!(repair.reason.contains("exit-code echo"), "{repair:?}");
        assert!(repair.reason.contains("workspace cd"), "{repair:?}");
    }

    #[test]
    fn verify_command_rejection_feedback_names_allowed_alternative() {
        let setup = diagnose_verify_command("npm install");
        assert_eq!(
            setup.violation,
            Some(VerifyCommandViolationKind::SetupOrDevServer)
        );
        assert!(
            setup
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("allowed alternatives"),
            "{setup:?}"
        );

        let pipe = diagnose_verify_command("npm run build | grep error");
        assert_eq!(
            pipe.violation,
            Some(VerifyCommandViolationKind::ShellControlSyntax)
        );
        assert!(
            pipe.reason
                .as_deref()
                .unwrap_or_default()
                .contains("split multiple checks into separate verify commands"),
            "{pipe:?}"
        );
    }

    #[test]
    fn verify_command_rejects_non_output_truncation_pipe() {
        let diagnosis = diagnose_verify_command("npm run build | grep error");

        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::ShellControlSyntax)
        );
        assert!(validate_verify_command("npm run build | grep error").is_err());
    }

    #[test]
    fn verify_command_accepts_narrow_leading_cd_for_safe_verify() {
        assert!(validate_verify_command("cd app && npm run build").is_ok());
        assert_eq!(
            diagnose_verify_command("cd app && npm run build").normalized,
            "cd app && npm run build"
        );
        assert!(validate_verify_command("cd ../app && npm run build").is_err());
        assert!(validate_verify_command("cd app && echo ok").is_err());
    }

    #[test]
    fn verify_command_keeps_real_check_instead_of_weak_downgrade() {
        assert!(normalize_verify_command("npm run build").is_ok());
        assert!(validate_verify_command("test -f package.json").is_ok());
        let diagnosis = diagnose_verify_command("npm run build && test -f package.json");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::ShellControlSyntax)
        );
        let normalized =
            normalize_planner_verify_command("npm run build && test -f package.json").unwrap();
        assert_eq!(normalized, vec!["npm run build", "test -f package.json"]);
        assert!(validate_verify_command("npm run build && test -f package.json").is_err());
    }

    #[test]
    fn verify_command_diagnoses_manifest_path_escape() {
        let diagnosis = diagnose_verify_command("cargo test --manifest-path ../Cargo.toml");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::WorkspaceEscape)
        );
    }

    #[test]
    fn verify_command_rejects_shell_control_syntax() {
        for command in [
            "npm test && npm run build",
            "npm run build && test -f package.json",
            "cargo test | cat",
            "npm test; echo ok",
            "cargo test > out.log",
            "cargo test \\; echo ok",
            "cargo test $(whoami)",
        ] {
            assert!(validate_verify_command(command).is_err(), "{command}");
        }
    }

    #[test]
    fn planner_verify_normalization_splits_only_allowlisted_and_commands() {
        let normalized = normalize_planner_verify_command(
            "npm test && npm run build && test -f src/app/page.tsx",
        )
        .unwrap();
        assert_eq!(
            normalized,
            vec!["npm test", "npm run build", "test -f src/app/page.tsx"]
        );
    }

    #[test]
    fn planner_verify_normalization_rejects_unsafe_shell_syntax() {
        for command in [
            "npm test || npm run build",
            "npm test; npm run build",
            "npm test | cat",
            "npm test > out.log",
            "npm test && echo ok",
            "npm test && test -f ../secret",
        ] {
            assert!(
                normalize_planner_verify_command(command).is_err(),
                "{command}"
            );
        }
    }

    #[test]
    fn verify_command_rejects_install_or_dev_server() {
        for command in ["npm install", "pnpm install", "next dev -p 3011"] {
            assert!(validate_verify_command(command).is_err(), "{command}");
        }
    }

    #[test]
    fn verify_command_nonzero_fails() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "work".to_string(),
            expected_result: "pass".to_string(),
            instruction: "x".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["false".to_string()],
        };
        assert!(matches!(
            verify_step(dir.path(), &step).status,
            VerifyStatus::CommandFailed(_)
        ));
    }

    #[test]
    fn raw_verify_command_uses_normalized_env() {
        let status = run_ignored_verify_harness(
            "planner::verify::tests::raw_verify_command_uses_normalized_env_child",
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn raw_verify_command_uses_normalized_env_child() {
        let dir = tempfile::tempdir().unwrap();
        let checker = dir.path().join("check-env.sh");
        write_executable(
            &checker,
            "#!/bin/sh\n\
             test -z \"${NODE_ENV+x}\" || exit 42\n\
             test -z \"${NODE_OPTIONS+x}\" || exit 43\n\
             test \"$NEXT_TELEMETRY_DISABLED\" = \"1\" || exit 44\n\
             exit 0\n",
        );
        let step = PlanStep {
            id: "env".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "verify normalized env".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["./check-env.sh".to_string()],
        };

        let report = verify_step(dir.path(), &step);
        assert!(report.is_pass(), "{report:?}");
    }

    #[test]
    fn nextjs_build_missing_next_binary_is_dependency_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"x","react":"x","react-dom":"x"}}"#,
        )
        .unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "x".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };
        assert!(matches!(
            verify_step(dir.path(), &step).status,
            VerifyStatus::DependencyMissing(_)
        ));
    }

    #[test]
    fn nextjs_build_missing_manifest_is_dependency_boundary_not_command_execution() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let step = PlanStep {
            id: "final-verify".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run deterministic Next.js build".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };

        let (report, lifecycles) =
            verify_step_with_setup_observed(dir.path(), &step, NodeDependencySetupAuthority::None);

        assert!(matches!(report.status, VerifyStatus::DependencyMissing(_)));
        assert!(
            report
                .primary_reason()
                .contains("package.json missing before Next.js build verifier"),
            "{report:?}"
        );
        assert_eq!(lifecycles.len(), 1);
        assert!(!lifecycles[0].before_setup.attempted);
        assert_eq!(lifecycles[0].setup_status(), "blocked");
        assert!(
            lifecycles[0]
                .lifecycle_stages()
                .contains(&"verification_dependency_missing")
        );
    }

    #[test]
    fn nextjs_build_step_returns_dependency_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"x","react":"x","react-dom":"x"}}"#,
        )
        .unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "x".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };
        let (report, lifecycles) =
            verify_step_with_setup_observed(dir.path(), &step, NodeDependencySetupAuthority::None);
        assert!(report.primary_reason().contains("dependency_setup_missing"));
        assert_eq!(lifecycles.len(), 1);
        assert!(lifecycles[0].lifecycle_stages().contains(&"setup_blocked"));
    }

    #[test]
    fn nextjs_type_error_build_failure_is_implementation_compile_error() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_next_build_workspace(
            dir.path(),
            "#!/bin/sh\n\
             if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
               echo './src/components/SpaceInvaders.tsx:137:28' >&2\n\
               echo \"Type error: Cannot find name 'reset'.\" >&2\n\
               exit 1\n\
             fi\n\
             exit 2\n",
        );
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            "export function SpaceInvaders(){return <button onClick={reset}>Restart</button>;}\n",
        )
        .unwrap();
        let step = PlanStep {
            id: "build".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run production build".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };

        let (report, lifecycles) =
            verify_step_with_setup_observed(dir.path(), &step, NodeDependencySetupAuthority::None);

        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
        assert!(report.dependency_missing.is_empty(), "{report:?}");
        assert_eq!(report.compile_errors.len(), 1, "{report:?}");
        assert_eq!(
            report.compile_errors[0].path,
            "src/components/SpaceInvaders.tsx"
        );
        assert_eq!(report.compile_errors[0].line, 137);
        assert_eq!(report.compile_errors[0].symbol.as_deref(), Some("reset"));
        assert_eq!(report.compile_errors[0].route_bound, Some(false));
        assert!(
            report
                .primary_reason()
                .contains("implementation_compile_error"),
            "{report:?}"
        );
        let feedback = crate::minimal_loop::completion::format_verify_feedback(&report);
        assert!(
            feedback.contains("src/components/SpaceInvaders.tsx:137:28"),
            "{feedback}"
        );
        assert!(feedback.contains("define reset"), "{feedback}");
        assert!(
            feedback.contains("replace the reference with an existing handler"),
            "{feedback}"
        );
        assert!(feedback.contains("remove the dead code"), "{feedback}");
        assert!(
            feedback.contains("the file is not imported by any route"),
            "{feedback}"
        );
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].final_status, BuildVerifierStatus::Failed);
    }

    #[test]
    fn nextjs_swc_lifecycle_frame_is_compile_error_not_dependency_lifecycle_failure() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_next_build_workspace(
            dir.path(),
            "#!/bin/sh\n\
             if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
               cat >&2 <<'EOF'\n\
./src/app/game.ts\n\
Error:\n\
  x Expected ',', got '}'\n\
\n\
   ,-[./src/app/game.ts:631:1]\n\
628 |   const asteroids = [\n\
629 |     { x: 10, y: 20 },\n\
630 |     { x: 30, y: 40 }\n\
631 |   }\n\
    |   ^\n\
632 |   return asteroids\n\
   `----\n\
> Build failed because of webpack errors\n\
EOF\n\
               exit 1\n\
             fi\n\
             exit 2\n",
        );
        std::fs::write(
            dir.path().join("src/app/game.ts"),
            "export const asteroids = [];\n",
        )
        .unwrap();
        let step = PlanStep {
            id: "build".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run production build".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };

        let (report, lifecycles) =
            verify_step_with_setup_observed(dir.path(), &step, NodeDependencySetupAuthority::None);

        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
        assert!(
            report
                .command_failures
                .iter()
                .all(|failure| !failure.reason.contains("dependency_setup_lifecycle_failed")),
            "{report:?}"
        );
        assert!(report.dependency_missing.is_empty(), "{report:?}");
        assert_eq!(report.compile_errors.len(), 1, "{report:?}");
        assert_eq!(report.compile_errors[0].path, "src/app/game.ts");
        assert_eq!(report.compile_errors[0].line, 631);
        assert_eq!(report.compile_errors[0].message, "Expected ',', got '}'");
        assert!(report.compile_errors[0].excerpt.contains("631 |   }"));
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].final_status, BuildVerifierStatus::Failed);
        assert_eq!(lifecycles[0].final_observation().compile_errors.len(), 1);
        assert!(
            !report
                .primary_reason()
                .contains("dependency_setup_lifecycle_failed"),
            "{report:?}"
        );
    }

    #[test]
    fn nextjs_cannot_find_module_build_failure_stays_dependency_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_next_build_workspace(
            dir.path(),
            "#!/bin/sh\n\
             if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
               echo \"Cannot find module 'x'\" >&2\n\
               exit 1\n\
             fi\n\
             exit 2\n",
        );
        let step = PlanStep {
            id: "build".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run production build".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm run build".to_string()],
        };

        let report = verify_step(dir.path(), &step);

        assert!(matches!(report.status, VerifyStatus::DependencyMissing(_)));
        assert!(report.compile_errors.is_empty(), "{report:?}");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::DependencySetup
        );
    }

    #[test]
    fn setup_authority_dependency_probe_installs_before_raw_verify_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm.sh");
        write_executable(
            &fake_npm,
            "#!/bin/sh\nmkdir -p node_modules/next\necho '{\"version\":\"14.2.0\"}' > node_modules/next/package.json\ntouch package-lock.json\nexit 0\n",
        );
        let step = PlanStep {
            id: "probe-next".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Verify Next package resolution".to_string(),
            expected_paths: Vec::new(),
            verify: vec![r#"node -e "require('next/package.json')""#.to_string()],
        };

        let (report, lifecycles) = verify_step_with_setup_observed_with_options(
            dir.path(),
            &step,
            None,
            NodeDependencySetupAuthority::PlanSetupStep,
            &fake_npm,
            false,
            None,
        );

        assert!(report.is_pass(), "{report:?}");
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].setup_status(), "passed");
        assert_eq!(
            lifecycles[0]
                .setup
                .as_ref()
                .map(|setup| setup.setup_kind.as_str()),
            Some("node_declared_dependencies")
        );
        assert!(!lifecycles[0].before_setup.attempted);
        assert!(
            lifecycles[0]
                .after_setup
                .as_ref()
                .is_some_and(|after| after.attempted)
        );
    }

    #[test]
    fn setup_step_empty_verify_declared_dependencies_runs_state_install_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"left-pad":"^1.3.0"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm.sh");
        write_executable(
            &fake_npm,
            "#!/bin/sh\nmkdir -p node_modules\ntouch package-lock.json\nexit 0\n",
        );
        let step = PlanStep {
            id: "workspace-and-dependencies-setup".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Install declared dependencies".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };

        let (initial_report, initial_lifecycles) = verify_step_with_setup_observed_with_options(
            dir.path(),
            &step,
            None,
            NodeDependencySetupAuthority::PlanSetupStep,
            &fake_npm,
            false,
            None,
        );
        assert!(initial_report.is_pass(), "{initial_report:?}");
        assert!(initial_lifecycles.is_empty());

        let (report, lifecycles) = verify_setup_dependency_state_with_setup_observed_with_options(
            dir.path(),
            NodeDependencySetupAuthority::PlanSetupStep,
            &fake_npm,
            false,
        );

        assert!(report.is_pass(), "{report:?}");
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].setup_status(), "passed");
        assert_eq!(lifecycles[0].final_status, BuildVerifierStatus::Passed);
        assert!(dir.path().join("node_modules").is_dir());
        assert!(dir.path().join("package-lock.json").is_file());
        let setup = lifecycles[0].setup.as_ref().unwrap();
        assert_eq!(setup.lockfile_present_before, Some(false));
        assert_eq!(setup.lockfile_present_after, Some(true));
        assert_eq!(setup.lockfile_created, Some(true));
    }

    #[test]
    fn raw_cannot_find_module_routes_to_dependency_setup_target() {
        let dir = tempfile::tempdir().unwrap();
        let failing = dir.path().join("missing-module.sh");
        write_executable(
            &failing,
            "#!/bin/sh\necho \"Cannot find module 'next/package.json'\" >&2\nexit 1\n",
        );
        let step = PlanStep {
            id: "probe".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Probe dependency resolution".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["./missing-module.sh".to_string()],
        };

        let report = verify_step_with_setup(
            dir.path(),
            &step,
            NodeDependencySetupAuthority::PlanSetupStep,
        );

        assert!(matches!(report.status, VerifyStatus::DependencyMissing(_)));
        assert!(report.command_failures.is_empty(), "{report:?}");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::DependencySetup
        );
    }

    fn package_port_verify_step() -> PlanStep {
        PlanStep {
            id: "verify-package-port".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Verify the package script uses port 3011".to_string(),
            expected_paths: vec!["package.json".to_string()],
            verify: vec![r#"grep -q "-p 3011" package.json"#.to_string()],
        }
    }

    #[test]
    fn runtime_oracle_repairs_grep_dash_pattern_false_negative_once() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev -p 3011","build":"next build"}}"#,
        )
        .unwrap();

        let report = verify_step_with_setup_observed_with_offline_and_events(
            dir.path(),
            &package_port_verify_step(),
            NodeDependencySetupAuthority::None,
            false,
            Some(&events),
        )
        .0;

        assert!(report.is_pass(), "{report:?}");
        assert!(report.command_failures.is_empty(), "{report:?}");
        assert!(
            report.verifier_command_false_negatives.is_empty(),
            "{report:?}"
        );
        assert!(
            report.runtime_command_normalizations.is_empty(),
            "{report:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap_or_default();
        assert!(!event_text.contains("\"verify_command_false_negative_candidate\""));
    }

    #[test]
    fn runtime_oracle_keeps_normalized_grep_no_match_as_artifact_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev -p 3000","build":"next build"}}"#,
        )
        .unwrap();

        let report = verify_step_with_setup_observed_with_offline_and_events(
            dir.path(),
            &package_port_verify_step(),
            NodeDependencySetupAuthority::None,
            false,
            None,
        )
        .0;

        assert!(!report.is_pass(), "{report:?}");
        assert_eq!(report.command_failures.len(), 1);
        assert!(
            report.verifier_command_false_negatives.is_empty(),
            "{report:?}"
        );
        assert_eq!(
            report.command_failures[0].command,
            r#"grep -q -- "-p 3011" package.json"#
        );
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
    }

    #[test]
    fn unrepairable_usage_error_is_verifier_command_false_negative() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("usage_error.py"),
            "import sys\nsys.stderr.write('usage: fake\\ninvalid option\\n')\nsys.exit(2)\n",
        )
        .unwrap();
        let step = PlanStep {
            id: "usage-error".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run a malformed verifier command".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["python3 usage_error.py".to_string()],
        };

        let report = verify_step_with_setup_observed_with_offline_and_events(
            dir.path(),
            &step,
            NodeDependencySetupAuthority::None,
            false,
            None,
        )
        .0;

        assert!(!report.is_pass(), "{report:?}");
        assert!(report.command_failures.is_empty(), "{report:?}");
        assert_eq!(report.verifier_command_false_negatives.len(), 1);
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::VerifierCommand
        );
        let reachability = crate::minimal_loop::reachability::assess_repair_reachability(
            &report,
            None,
            NodeDependencySetupAuthority::None,
            false,
        );
        assert!(!reachability.reachable);
        assert_eq!(
            reachability.blocked_requirements,
            vec!["deterministic_verify_command_bug".to_string()]
        );
    }

    #[test]
    fn verify_timeout_is_oracle_error_false_negative_not_implementation() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let outcome = verifier_env::run_structured_for_verify_with_timeout(
            "sleep 5",
            dir.path(),
            false,
            std::time::Duration::from_millis(20),
        )
        .unwrap();
        let command = normalize_verify_command("sleep 5").unwrap();

        let result =
            handle_failed_verify_command(&command, dir.path(), None, false, Some(&events), outcome);

        let VerifyCommandRunResult::FalseNegative { command, reason } = result else {
            panic!("expected false negative, got {result:?}");
        };
        assert_eq!(command, "sleep 5");
        assert!(reason.contains("OracleError"), "{reason}");
        assert!(
            reason.contains("verify_command_timeout:sleep 5"),
            "{reason}"
        );
        assert!(reason.contains("the verify command hangs"), "{reason}");
        let mut report = VerificationReport::pass();
        report.push_verifier_command_false_negative(command, reason);
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::VerifierCommand
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"verify_command_timeout\""));
        assert!(event_text.contains("\"classification\":\"OracleError\""));
        assert!(event_text.contains("\"repair_target\":\"verifier_command\""));
        assert!(!event_text.contains("\"repair_target\":\"implementation\""));
    }

    #[test]
    fn python_cli_pytest_timeout_uses_compileall_substitution_when_available() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/anvil_app")).unwrap();
        std::fs::write(dir.path().join("src/anvil_app/main.py"), "print('ok')\n").unwrap();
        let timeout = BashOutcome {
            kind: BashOutcomeKind::Timeout,
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            elapsed_ms: 60_000,
            summary: "command timed out after 60000 ms".to_string(),
        };
        let command = normalize_verify_command("python -m pytest").unwrap();

        let result = handle_failed_verify_command(
            &command,
            dir.path(),
            Some("python-cli"),
            false,
            Some(&events),
            timeout,
        );

        let VerifyCommandRunResult::Passed { normalization, .. } = result else {
            panic!("expected substitution pass, got {result:?}");
        };
        let normalization = normalization.unwrap();
        assert_eq!(normalization.original, "python -m pytest");
        assert_eq!(normalization.repaired, "python -m compileall -q src");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"verify_command_timeout\""));
        assert!(event_text.contains("\"classification\":\"OracleError\""));
        assert!(event_text.contains("\"substitution_attempted\":true"));
        assert!(event_text.contains("\"event\":\"verify_command_timeout_substitution\""));
        assert!(event_text.contains("\"status\":\"passed\""));
    }

    fn run_ignored_verify_harness(test_name: &str) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new(exe)
            .args(["--ignored", "--exact", test_name, "--nocapture"])
            .env("NODE_ENV", "production")
            .env("NODE_OPTIONS", "--require ./host-hook.js")
            .status()
            .unwrap()
    }

    #[test]
    fn raw_cannot_find_module_without_authority_reports_setup_authority_required() {
        let dir = tempfile::tempdir().unwrap();
        let failing = dir.path().join("missing-module.sh");
        write_executable(
            &failing,
            "#!/bin/sh\necho \"Cannot find module 'next/package.json'\" >&2\nexit 1\n",
        );
        let step = PlanStep {
            id: "probe".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Probe dependency resolution".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["./missing-module.sh".to_string()],
        };

        let report = verify_step(dir.path(), &step);

        assert!(matches!(report.status, VerifyStatus::DependencyMissing(_)));
        assert!(
            report
                .dependency_missing
                .iter()
                .any(|reason| reason == "dependency_setup_authority_required: ./missing-module.sh"),
            "{report:?}"
        );
    }

    #[test]
    fn node_test_without_package_manifest_records_dependency_lifecycle_without_execution() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "test".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run Node tests".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["npm test".to_string()],
        };
        let (report, lifecycles) =
            verify_step_with_setup_observed(dir.path(), &step, NodeDependencySetupAuthority::None);
        assert!(
            report
                .primary_reason()
                .contains("package.json scripts.test missing"),
            "{report:?}"
        );
        assert_eq!(lifecycles.len(), 1);
        assert!(lifecycles[0].lifecycle_stages().contains(&"setup_blocked"));
        assert!(
            lifecycles[0]
                .lifecycle_stages()
                .contains(&"verification_dependency_missing")
        );
    }

    #[test]
    fn verify_step_aggregates_missing_paths_and_command_failures() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "verify".to_string(),
            expected_paths: vec!["missing-a.txt".to_string(), "missing-b.txt".to_string()],
            verify: vec!["false".to_string()],
        };
        let report = verify_step(dir.path(), &step);
        assert_eq!(report.missing_paths.len(), 2);
        assert_eq!(report.command_failures.len(), 1);
        assert!(matches!(report.status, VerifyStatus::MissingPath(_)));
    }

    #[test]
    fn verify_expected_result_fail_accepts_nonzero_command() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "verify".to_string(),
            expected_result: "fail".to_string(),
            instruction: "red test".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["false".to_string()],
        };
        assert!(verify_step(dir.path(), &step).is_pass());
    }

    #[test]
    fn verify_expected_result_pass_rejects_nonzero_command() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "green test".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["false".to_string()],
        };
        assert!(!verify_step(dir.path(), &step).is_pass());
    }

    #[test]
    fn verification_report_status_compat_accessor_matches_primary_failure() {
        let mut report = VerificationReport::pass();
        report.push_command_failure("cargo test", "failed");
        assert_eq!(
            report.status,
            VerifyStatus::CommandFailed("failed".to_string())
        );
        report.push_missing_path("src/main.rs");
        assert_eq!(
            report.status,
            VerifyStatus::MissingPath("src/main.rs".to_string())
        );
    }
}
