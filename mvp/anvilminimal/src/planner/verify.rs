use std::path::Path;

use crate::minimal_loop::build_verifier::{
    self, BuildVerifierLifecycleObservation, BuildVerifierStatus,
};
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::verifier_env;
use crate::planner::step_plan::{ExpectedResult, PlanStep};
use crate::tools::path_guard::{resolve_existing, validate_workspace_relative};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyStatus {
    Pass,
    MissingPath(String),
    CommandFailed(String),
    DependencyMissing(String),
    ProfileContractFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub status: VerifyStatus,
    pub missing_paths: Vec<String>,
    pub command_failures: Vec<CommandFailure>,
    pub dependency_missing: Vec<String>,
    pub profile_failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFailure {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyCommandViolationKind {
    Empty,
    Blocked,
    ShellControlSyntax,
    SetupOrDevServer,
    WorkspaceEscape,
}

impl VerifyCommandViolationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Blocked => "blocked",
            Self::ShellControlSyntax => "shell_control_syntax",
            Self::SetupOrDevServer => "setup_or_dev_server",
            Self::WorkspaceEscape => "workspace_escape",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "verify command is empty",
            Self::Blocked => "verify command is blocked",
            Self::ShellControlSyntax => "verify command may not use shell control syntax",
            Self::SetupOrDevServer => "verify command may not perform setup or start a dev server",
            Self::WorkspaceEscape => "verify command manifest path escapes workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCommandDiagnosis {
    pub normalized: String,
    pub violation: Option<VerifyCommandViolationKind>,
    pub reason: Option<String>,
}

impl VerificationReport {
    pub fn pass() -> Self {
        Self {
            status: VerifyStatus::Pass,
            missing_paths: Vec::new(),
            command_failures: Vec::new(),
            dependency_missing: Vec::new(),
            profile_failures: Vec::new(),
        }
    }

    pub fn is_pass(&self) -> bool {
        self.status == VerifyStatus::Pass
            && self.missing_paths.is_empty()
            && self.command_failures.is_empty()
            && self.dependency_missing.is_empty()
            && self.profile_failures.is_empty()
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

    pub fn push_dependency_missing(&mut self, reason: impl Into<String>) {
        self.dependency_missing.push(reason.into());
        self.refresh_status();
    }

    pub fn push_profile_failure(&mut self, reason: impl Into<String>) {
        self.profile_failures.push(reason.into());
        self.refresh_status();
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
    verify_step_with_setup_observed_with_options(
        root,
        step,
        setup_authority,
        Path::new("npm"),
        offline,
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
fn verify_setup_dependency_state_with_setup_observed_with_options(
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
fn verify_setup_dependency_state_with_setup_observed_with_options(
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
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
    let mut report = VerificationReport::pass();
    let mut build_lifecycles = Vec::new();
    for path in &step.expected_paths {
        if resolve_existing(root, path).is_err() {
            report.push_missing_path(path.clone());
        }
    }
    for command in &step.verify {
        if let Err(err) = validate_verify_command(command) {
            report.push_command_failure(command.clone(), err.to_string());
            continue;
        }
        if let Some(requirement) = build_verifier::requirement_from_deferred(
            command,
            build_verifier_profile(command),
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
                build_verifier_profile(command),
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
        match verifier_env::run_checked(command, root, false) {
            Ok(output) => {
                if step.expected_result_kind() == ExpectedResult::Fail {
                    report.push_command_failure(
                        command.clone(),
                        "expected command to fail but it passed",
                    );
                } else if command.contains("npm") && output.contains("0 tests") {
                    report.push_command_failure(command.clone(), "Node 0 tests rejected");
                }
            }
            Err(err) => {
                let reason = err.to_string();
                if build_verifier::is_dependency_missing_output(&reason) {
                    if setup_authority.allows_setup() {
                        let requirement =
                            build_verifier::requirement_from_dependency_missing_output(
                                command,
                                build_verifier_profile(command),
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
                    report.push_command_failure(command.clone(), reason);
                }
            }
        }
    }
    (report, build_lifecycles)
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
            report.push_command_failure(
                command.to_string(),
                format!(
                    "dependency_setup_lifecycle_failed: {}",
                    lifecycle_failure_with_setup_output(lifecycle)
                ),
            );
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
    let diagnosis = diagnose_verify_command(command);
    if let Some(violation) = diagnosis.violation {
        anyhow::bail!(
            "{}",
            diagnosis
                .reason
                .unwrap_or_else(|| violation.message().to_string())
        );
    }
    Ok(())
}

pub fn normalize_verify_command(command: &str) -> anyhow::Result<String> {
    let diagnosis = diagnose_verify_command(command);
    if let Some(violation) = diagnosis.violation {
        anyhow::bail!(
            "{}",
            diagnosis
                .reason
                .unwrap_or_else(|| violation.message().to_string())
        );
    }
    Ok(diagnosis.normalized)
}

pub fn normalize_planner_verify_command(command: &str) -> anyhow::Result<Vec<String>> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if !contains_shell_control_syntax(trimmed) {
        return Ok(vec![normalize_verify_command(trimmed)?]);
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
    if contains_shell_control_syntax(&normalized) {
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::ShellControlSyntax,
            None,
        );
    }
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("npm install")
        || lower.contains("pnpm install")
        || lower.contains("yarn install")
        || lower.contains("cargo install")
        || lower.contains("next dev")
        || lower.contains("vite --host")
    {
        return verify_command_violation(
            normalized,
            VerifyCommandViolationKind::SetupOrDevServer,
            None,
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
        if !is_safe_split_verify_fragment(&normalized) {
            anyhow::bail!(
                "verify command shell split contains unsupported fragment: {}",
                normalized
            );
        }
        out.push(normalized);
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

fn contains_shell_control_syntax(command: &str) -> bool {
    command.bytes().any(|byte| {
        matches!(
            byte,
            b';' | b'&' | b'|' | b'<' | b'>' | b'`' | b'\n' | b'\r' | b'\\'
        )
    }) || command.contains("$(")
}

fn is_nextjs_build_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower == "npm run build"
        || lower.starts_with("npm run build ")
        || lower.contains("next build")
        || lower == "pnpm build"
        || lower.starts_with("pnpm build ")
        || lower == "yarn build"
        || lower.starts_with("yarn build ")
}

fn build_verifier_profile(command: &str) -> Option<&'static str> {
    if is_nextjs_build_command(command) {
        Some("nextjs")
    } else {
        None
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
        assert_eq!(
            diagnosis.reason.as_deref(),
            Some("verify command may not use shell control syntax")
        );
    }

    #[test]
    fn verify_command_diagnoses_setup_or_dev_server() {
        let diagnosis = diagnose_verify_command("next dev -p 3011");
        assert_eq!(
            diagnosis.violation,
            Some(VerifyCommandViolationKind::SetupOrDevServer)
        );
        assert_eq!(
            diagnosis.reason.as_deref(),
            Some("verify command may not perform setup or start a dev server")
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
        assert_eq!(normalized, "cargo test --locked");
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
            NodeDependencySetupAuthority::PlanSetupStep,
            &fake_npm,
            false,
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
            NodeDependencySetupAuthority::PlanSetupStep,
            &fake_npm,
            false,
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
