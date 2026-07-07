use std::path::Path;

use serde::Serialize;

use crate::eval_events;
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupObservation, NodeDependencySetupStatus,
};
use crate::minimal_loop::verifier_env;
use crate::planner::profile::{build_oracle_for_command, profile_for_build_requirement};
use crate::planner::verify::validate_verify_command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompileError {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_bound: Option<bool>,
}

impl CompileError {
    pub fn location(&self) -> String {
        if self.line > 0 && self.column > 0 {
            format!("{}:{}:{}", self.path, self.line, self.column)
        } else if self.line > 0 {
            format!("{}:{}", self.path, self.line)
        } else {
            self.path.clone()
        }
    }

    pub fn summary(&self) -> String {
        format!("{} {}", self.location(), self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierRequirement {
    pub command: String,
    pub profile: Option<String>,
    pub reason: String,
    pub authority: String,
    pub status: String,
    pub requires_dependency_setup: bool,
    pub required_for_completion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildVerifierStatus {
    PolicyRejected,
    DependencyMissing,
    Blocked,
    Passed,
    Failed,
}

impl BuildVerifierStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolicyRejected => "policy_rejected",
            Self::DependencyMissing => "dependency_missing",
            Self::Blocked => "blocked",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForeignToolchainObservation {
    pub tool: String,
    pub resolved_path: String,
    pub workspace_root: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierObservation {
    pub command: String,
    pub profile: Option<String>,
    pub authority: String,
    pub required_for_completion: bool,
    pub requires_dependency_setup: bool,
    pub dependency_ready: bool,
    pub attempted: bool,
    pub status: BuildVerifierStatus,
    pub primary_reason: String,
    pub output_snippet: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compile_errors: Vec<CompileError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_toolchain: Option<ForeignToolchainObservation>,
}

impl BuildVerifierObservation {
    pub fn status_str(&self) -> &'static str {
        self.status.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierLifecycleObservation {
    pub requirement: BuildVerifierRequirement,
    pub before_setup: BuildVerifierObservation,
    pub setup: Option<NodeDependencySetupObservation>,
    pub after_setup: Option<BuildVerifierObservation>,
    pub final_status: BuildVerifierStatus,
    pub final_reason: String,
}

impl BuildVerifierLifecycleObservation {
    pub fn final_observation(&self) -> &BuildVerifierObservation {
        self.after_setup.as_ref().unwrap_or(&self.before_setup)
    }

    pub fn setup_status(&self) -> &'static str {
        self.setup
            .as_ref()
            .map(|setup| setup.status.as_str())
            .unwrap_or("not_required")
    }

    pub fn lifecycle_stages(&self) -> Vec<&'static str> {
        let mut stages = vec!["dependency_check"];
        if self.before_setup.status == BuildVerifierStatus::DependencyMissing
            && self.requirement.requires_dependency_setup
        {
            if self
                .setup
                .as_ref()
                .is_some_and(|setup| setup.authority.allows_setup())
            {
                stages.push("setup_authority_selected");
            } else {
                stages.push("setup_authority_missing");
            }
            if self.setup.as_ref().is_some_and(|setup| setup.attempted) {
                stages.push("setup_attempted");
            }
            match self.setup.as_ref().map(|setup| setup.status) {
                Some(NodeDependencySetupStatus::Blocked) => stages.push("setup_blocked"),
                Some(NodeDependencySetupStatus::Attempted) => {}
                Some(NodeDependencySetupStatus::Passed) => stages.push("setup_passed"),
                Some(NodeDependencySetupStatus::Failed) => stages.push("setup_failed"),
                Some(NodeDependencySetupStatus::TimedOut) => stages.push("setup_timed_out"),
                Some(NodeDependencySetupStatus::NotRequired) => stages.push("setup_not_required"),
                None => stages.push("setup_not_requested"),
            }
        }
        if self.after_setup.is_some() {
            stages.push("build_rerun_attempted");
            stages.push("build_rerun");
        }
        stages.push(match self.final_status {
            BuildVerifierStatus::Passed => "verification_passed",
            BuildVerifierStatus::Failed => "verification_failed",
            BuildVerifierStatus::Blocked => "verification_blocked",
            BuildVerifierStatus::DependencyMissing => "verification_dependency_missing",
            BuildVerifierStatus::PolicyRejected => "verification_policy_rejected",
        });
        stages
    }
}

pub fn emit_dependency_build_lifecycle(
    eval_events_path: Option<&Path>,
    mode: &str,
    step_id: Option<&str>,
    lifecycle: &BuildVerifierLifecycleObservation,
) {
    eval_events::emit(
        eval_events_path,
        serde_json::json!({
            "event": "dependency_build_lifecycle",
            "mode": mode,
            "step_id": step_id.unwrap_or(""),
            "lifecycle_stage": "dependency_setup_build",
            "lifecycle_stages": lifecycle.lifecycle_stages(),
            "command": lifecycle.requirement.command,
            "profile": lifecycle.requirement.profile,
            "authority": lifecycle.requirement.authority,
            "required_for_completion": lifecycle.requirement.required_for_completion,
            "requires_dependency_setup": lifecycle.requirement.requires_dependency_setup,
            "before_status": lifecycle.before_setup.status_str(),
            "before_attempted": lifecycle.before_setup.attempted,
            "setup_status": lifecycle.setup_status(),
            "setup_attempted": lifecycle.setup.as_ref().is_some_and(|setup| setup.attempted),
            "setup_authority": lifecycle.setup.as_ref().map(|setup| setup.authority.as_str()).unwrap_or("none"),
            "setup_kind": lifecycle.setup.as_ref().map(|setup| setup.setup_kind.as_str()).unwrap_or("none"),
            "setup_command": lifecycle.setup.as_ref().map(|setup| setup.command.as_str()).unwrap_or(""),
            "setup_changed_paths": lifecycle.setup.as_ref().map(|setup| setup.changed_paths.clone()).unwrap_or_default(),
            "setup_duration_ms": lifecycle.setup.as_ref().and_then(|setup| setup.duration_ms),
            "setup_timeout_ms": lifecycle.setup.as_ref().and_then(|setup| setup.timeout_ms),
            "setup_timeout_classification": lifecycle
                .setup
                .as_ref()
                .filter(|setup| setup.status == NodeDependencySetupStatus::TimedOut)
                .map(|_| "dependency_setup_timeout")
                .unwrap_or(""),
            "setup_lockfile_present_before": lifecycle.setup.as_ref().and_then(|setup| setup.lockfile_present_before),
            "setup_lockfile_present_after": lifecycle.setup.as_ref().and_then(|setup| setup.lockfile_present_after),
            "setup_lockfile_created": lifecycle.setup.as_ref().and_then(|setup| setup.lockfile_created),
            "after_status": lifecycle.after_setup.as_ref().map(BuildVerifierObservation::status_str).unwrap_or(""),
            "after_attempted": lifecycle.after_setup.as_ref().is_some_and(|observation| observation.attempted),
            "build_rerun_attempted": lifecycle.after_setup.as_ref().is_some_and(|observation| observation.attempted),
            "final_status": lifecycle.final_status.as_str(),
            "final_reason": eval_events::body_snippet(&lifecycle.final_reason),
            "compile_errors": lifecycle.final_observation().compile_errors.clone(),
        }),
    );
    emit_foreign_toolchain_detected(eval_events_path, mode, step_id, &lifecycle.before_setup);
    if let Some(after_setup) = lifecycle.after_setup.as_ref() {
        emit_foreign_toolchain_detected(eval_events_path, mode, step_id, after_setup);
    }
}

fn emit_foreign_toolchain_detected(
    eval_events_path: Option<&Path>,
    mode: &str,
    step_id: Option<&str>,
    observation: &BuildVerifierObservation,
) {
    if let Some(foreign) = observation.foreign_toolchain.as_ref() {
        eval_events::emit(
            eval_events_path,
            serde_json::json!({
                "event": "foreign_toolchain_detected",
                "mode": mode,
                "step_id": step_id.unwrap_or(""),
                "command": observation.command,
                "profile": observation.profile,
                "tool": foreign.tool,
                "resolved_path": foreign.resolved_path,
                "workspace_root": foreign.workspace_root,
                "reason": foreign.reason,
                "status": observation.status_str(),
            }),
        );
    }
}

pub fn requirement_from_deferred(
    command: &str,
    profile: Option<&str>,
    reason: &str,
    authority: &str,
    status: &str,
) -> Option<BuildVerifierRequirement> {
    let (_, oracle) = build_oracle_for_command(profile, command)?;
    Some(BuildVerifierRequirement {
        command: oracle.command,
        profile: oracle.profile.or_else(|| profile.map(str::to_string)),
        reason: reason.to_string(),
        authority: authority.to_string(),
        status: status.to_string(),
        requires_dependency_setup: oracle.requires_dependency_setup,
        required_for_completion: status != "optional",
    })
}

pub fn requirement_from_dependency_state(
    root: &Path,
    command: &str,
    profile: Option<&str>,
    reason: &str,
    authority: &str,
    status: &str,
) -> Option<BuildVerifierRequirement> {
    if let Some((profile_impl, oracle)) = build_oracle_for_command(profile, command) {
        if profile_impl.dependency_ready(root, command) {
            return None;
        }
        return Some(BuildVerifierRequirement {
            command: oracle.command,
            profile: oracle.profile.or_else(|| profile.map(str::to_string)),
            reason: reason.to_string(),
            authority: authority.to_string(),
            status: status.to_string(),
            requires_dependency_setup: true,
            required_for_completion: status != "optional",
        });
    }
    if !dependency_setup::package_json_declares_dependencies(root) {
        return None;
    }
    if dependency_setup::node_declared_dependencies_ready(root) {
        return None;
    }
    Some(BuildVerifierRequirement {
        command: command.to_string(),
        profile: profile.map(str::to_string),
        reason: reason.to_string(),
        authority: authority.to_string(),
        status: status.to_string(),
        requires_dependency_setup: true,
        required_for_completion: status != "optional",
    })
}

pub fn requirement_from_dependency_missing_output(
    command: &str,
    profile: Option<&str>,
    reason: &str,
    authority: &str,
    status: &str,
) -> BuildVerifierRequirement {
    let oracle = build_oracle_for_command(profile, command).map(|(_, oracle)| oracle);
    BuildVerifierRequirement {
        command: oracle
            .as_ref()
            .map(|oracle| oracle.command.clone())
            .unwrap_or_else(|| command.to_string()),
        profile: oracle
            .and_then(|oracle| oracle.profile)
            .or_else(|| profile.map(str::to_string)),
        reason: reason.to_string(),
        authority: authority.to_string(),
        status: status.to_string(),
        requires_dependency_setup: true,
        required_for_completion: status != "optional",
    }
}

pub fn observe_requirement(
    root: &Path,
    requirement: &BuildVerifierRequirement,
) -> BuildVerifierObservation {
    let profile = profile_for_build_requirement(requirement);
    let dependency_ready = !requirement.requires_dependency_setup
        || profile.dependency_ready(root, &requirement.command);
    if let Err(err) = validate_verify_command(&requirement.command) {
        return BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: false,
            status: BuildVerifierStatus::PolicyRejected,
            primary_reason: err.to_string(),
            output_snippet: String::new(),
            compile_errors: Vec::new(),
            foreign_toolchain: None,
        };
    }
    if !dependency_ready {
        let foreign_toolchain = profile.foreign_toolchain(root, requirement);
        let mut primary_reason = profile.dependency_missing_reason(root, &requirement.command);
        if let Some(foreign) = foreign_toolchain.as_ref() {
            primary_reason = format!("{primary_reason}; {}", foreign.reason);
        }
        return BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: false,
            status: BuildVerifierStatus::DependencyMissing,
            primary_reason,
            output_snippet: String::new(),
            compile_errors: Vec::new(),
            foreign_toolchain,
        };
    }
    match verifier_env::run_checked(&requirement.command, root, false) {
        Ok(output) => BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: true,
            status: BuildVerifierStatus::Passed,
            primary_reason: "build verifier passed".to_string(),
            output_snippet: eval_events::body_snippet(&output),
            compile_errors: Vec::new(),
            foreign_toolchain: None,
        },
        Err(err) => {
            let reason = err.to_string();
            let mut compile_errors = parse_compile_errors_for_requirement(requirement, &reason);
            profile.annotate_compile_errors(root, &mut compile_errors);
            let status = if profile.dependency_missing_output(&reason) {
                BuildVerifierStatus::DependencyMissing
            } else if !compile_errors.is_empty() {
                BuildVerifierStatus::Failed
            } else if reason.contains("blocked") {
                BuildVerifierStatus::Blocked
            } else {
                BuildVerifierStatus::Failed
            };
            BuildVerifierObservation {
                command: requirement.command.clone(),
                profile: requirement.profile.clone(),
                authority: requirement.authority.clone(),
                required_for_completion: requirement.required_for_completion,
                requires_dependency_setup: requirement.requires_dependency_setup,
                dependency_ready,
                attempted: true,
                status,
                primary_reason: eval_events::body_snippet(&reason),
                output_snippet: eval_events::body_snippet(&reason),
                compile_errors,
                foreign_toolchain: None,
            }
        }
    }
}

pub fn observe_requirement_lifecycle(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
) -> BuildVerifierLifecycleObservation {
    observe_requirement_lifecycle_with_offline(root, requirement, setup_authority, false)
}

pub fn observe_requirement_lifecycle_with_offline(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> BuildVerifierLifecycleObservation {
    observe_requirement_lifecycle_with_setup_program_and_offline(
        root,
        requirement,
        setup_authority,
        Path::new("npm"),
        offline,
    )
}

#[cfg(test)]
pub(crate) fn observe_requirement_lifecycle_with_setup_program(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
) -> BuildVerifierLifecycleObservation {
    observe_requirement_lifecycle_with_setup_program_and_offline(
        root,
        requirement,
        setup_authority,
        npm_program,
        false,
    )
}

pub(crate) fn observe_requirement_lifecycle_with_setup_program_and_offline(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
) -> BuildVerifierLifecycleObservation {
    let before_setup = observe_requirement(root, requirement);
    observe_requirement_lifecycle_from_before(
        root,
        requirement,
        setup_authority,
        npm_program,
        offline,
        before_setup,
    )
}

pub fn observe_dependency_missing_output_lifecycle_with_offline(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    output: &str,
    offline: bool,
) -> BuildVerifierLifecycleObservation {
    observe_dependency_missing_output_lifecycle_with_setup_program_and_offline(
        root,
        requirement,
        setup_authority,
        output,
        Path::new("npm"),
        offline,
    )
}

pub(crate) fn observe_dependency_missing_output_lifecycle_with_setup_program_and_offline(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    output: &str,
    npm_program: &Path,
    offline: bool,
) -> BuildVerifierLifecycleObservation {
    let snippet = eval_events::body_snippet(output);
    let before_setup = BuildVerifierObservation {
        command: requirement.command.clone(),
        profile: requirement.profile.clone(),
        authority: requirement.authority.clone(),
        required_for_completion: requirement.required_for_completion,
        requires_dependency_setup: requirement.requires_dependency_setup,
        dependency_ready: profile_for_build_requirement(requirement)
            .dependency_ready(root, &requirement.command),
        attempted: true,
        status: BuildVerifierStatus::DependencyMissing,
        primary_reason: snippet.clone(),
        output_snippet: snippet,
        compile_errors: Vec::new(),
        foreign_toolchain: profile_for_build_requirement(requirement)
            .foreign_toolchain(root, requirement),
    };
    observe_requirement_lifecycle_from_before(
        root,
        requirement,
        setup_authority,
        npm_program,
        offline,
        before_setup,
    )
}

fn observe_requirement_lifecycle_from_before(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
    offline: bool,
    before_setup: BuildVerifierObservation,
) -> BuildVerifierLifecycleObservation {
    let mut setup = None;
    let mut after_setup = None;
    if before_setup.status == BuildVerifierStatus::DependencyMissing
        && requirement.requires_dependency_setup
    {
        let setup_requirement = dependency_setup_requirement(root, requirement, setup_authority);
        let setup_observation = if setup_requirement.allowed {
            dependency_setup::run_node_dependency_setup_with_program_and_offline(
                root,
                &setup_requirement,
                npm_program,
                offline,
            )
        } else {
            NodeDependencySetupObservation::blocked(
                setup_requirement.setup_kind,
                setup_requirement.package_manager,
                setup_requirement.setup_authority,
                setup_requirement
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "dependency setup blocked".to_string()),
            )
        };
        if setup_observation.status == NodeDependencySetupStatus::Passed {
            after_setup = Some(observe_requirement(root, requirement));
        }
        setup = Some(setup_observation);
    }
    let final_status = after_setup.as_ref().unwrap_or(&before_setup).status;
    let final_reason = after_setup
        .as_ref()
        .unwrap_or(&before_setup)
        .primary_reason
        .clone();
    BuildVerifierLifecycleObservation {
        requirement: requirement.clone(),
        before_setup,
        setup,
        after_setup,
        final_status,
        final_reason,
    }
}

pub fn parse_compile_errors(output: &str) -> Vec<CompileError> {
    let clean = strip_ansi_sequences(output);
    let lines = clean.lines().collect::<Vec<_>>();
    let mut errors = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some((path, line_number, column)) = parse_compile_location_line(line) {
            let details = compile_error_details_after_location(&lines, index);
            let message = compile_message_after_location(&lines, index)
                .unwrap_or_else(|| "compile error".to_string());
            let excerpt = details
                .as_ref()
                .map(|details| details.excerpt.clone())
                .unwrap_or_default();
            push_compile_error(
                &mut errors,
                CompileError {
                    path,
                    line: line_number,
                    column,
                    excerpt,
                    symbol: cannot_find_name_symbol(&message),
                    message,
                    route_bound: None,
                },
            );
        } else if let Some(error) = parse_inline_tsc_error(line) {
            push_compile_error(&mut errors, error);
        }
    }
    if errors.is_empty() {
        parse_failed_to_compile_module_error(&lines, &mut errors);
    }
    if errors.is_empty() {
        parse_swc_source_frame_errors(&lines, &mut errors);
    }
    errors
}

fn push_compile_error(errors: &mut Vec<CompileError>, error: CompileError) {
    if !errors.iter().any(|existing| {
        existing.path == error.path
            && existing.line == error.line
            && existing.column == error.column
            && existing.message == error.message
    }) {
        errors.push(error);
    }
}

fn parse_compile_location_line(line: &str) -> Option<(String, usize, usize)> {
    let trimmed = trim_compile_line(line);
    if trimmed.contains(",-[") || trimmed.contains("`-[") {
        return None;
    }
    let mut parts = trimmed.rsplitn(3, ':');
    let column = parts.next()?.trim().parse::<usize>().ok()?;
    let line_number = parts.next()?.trim().parse::<usize>().ok()?;
    let path = normalize_compile_error_path(parts.next()?.trim())?;
    if compile_error_path_is_supported(&path) {
        Some((path, line_number, column))
    } else {
        None
    }
}

fn parse_inline_tsc_error(line: &str) -> Option<CompileError> {
    let trimmed = trim_compile_line(line);
    let marker = " - error ";
    let (location, message) = trimmed
        .split_once(marker)
        .or_else(|| trimmed.split_once(": error "))?;
    let (path, line_number, column) = parse_compile_location_line(location)?;
    let message = message.trim().to_string();
    Some(CompileError {
        path,
        line: line_number,
        column,
        excerpt: String::new(),
        symbol: cannot_find_name_symbol(&message),
        message,
        route_bound: None,
    })
}

fn compile_message_after_location(lines: &[&str], location_index: usize) -> Option<String> {
    compile_error_details_after_location(lines, location_index).map(|details| details.message)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileErrorDetails {
    message: String,
    locator: Option<(String, usize, usize)>,
    excerpt: String,
}

fn compile_error_details_after_location(
    lines: &[&str],
    location_index: usize,
) -> Option<CompileErrorDetails> {
    lines
        .iter()
        .enumerate()
        .skip(location_index + 1)
        .find_map(|(index, line)| {
            let line = trim_compile_line(line);
            let matched = !line.is_empty()
                && (line.contains("Type error:")
                    || line.contains("Syntax error:")
                    || line.contains("Syntax Error")
                    || line.starts_with("Error:")
                    || line.starts_with("error "));
            if !matched {
                return None;
            }
            Some(compile_error_details_from_message_line(lines, index))
        })
}

fn compile_error_details_from_message_line(
    lines: &[&str],
    message_index: usize,
) -> CompileErrorDetails {
    let raw_message = trim_compile_line(lines[message_index]);
    let message = if is_bare_error_line(raw_message) {
        collect_swc_x_message_lines(lines, message_index)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| raw_message.to_string())
    } else {
        raw_message.to_string()
    };
    let locator = swc_frame_locator_after(lines, message_index);
    let excerpt = compile_excerpt_after_message(lines, message_index);
    CompileErrorDetails {
        message,
        locator,
        excerpt,
    }
}

fn is_bare_error_line(line: &str) -> bool {
    line.trim() == "Error:"
}

fn collect_swc_x_message_lines(lines: &[&str], message_index: usize) -> Option<String> {
    let mut messages = Vec::new();
    let mut started = false;
    for line in lines
        .iter()
        .skip(message_index + 1)
        .map(|line| trim_compile_line(line))
    {
        if line.is_empty() {
            if started {
                break;
            }
            continue;
        }
        if let Some(message) = line
            .strip_prefix("x ")
            .or_else(|| line.strip_prefix("x\t"))
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            started = true;
            messages.push(message.to_string());
            continue;
        }
        if started {
            break;
        }
        if parse_swc_frame_locator(line).is_some() || swc_code_frame_line(line) {
            break;
        }
    }
    (!messages.is_empty()).then(|| messages.join(" "))
}

fn swc_frame_locator_after(lines: &[&str], message_index: usize) -> Option<(String, usize, usize)> {
    lines
        .iter()
        .skip(message_index + 1)
        .take(12)
        .find_map(|line| parse_swc_frame_locator(line))
}

fn parse_swc_frame_locator(line: &str) -> Option<(String, usize, usize)> {
    let trimmed = trim_compile_line(line);
    let start = trimmed.find(",-[")? + 3;
    let end = trimmed[start..].find(']')? + start;
    let location = &trimmed[start..end];
    let mut parts = location.rsplitn(3, ':');
    let column = parts.next()?.trim().parse::<usize>().ok()?;
    let line_number = parts.next()?.trim().parse::<usize>().ok()?;
    let path = normalize_compile_error_path(parts.next()?.trim())?;
    compile_error_path_is_supported(&path).then_some((path, line_number, column))
}

fn compile_excerpt_after_message(lines: &[&str], message_index: usize) -> String {
    let excerpt = lines
        .iter()
        .skip(message_index + 1)
        .filter_map(|line| {
            let trimmed = trim_compile_line(line);
            if swc_code_frame_line(trimmed) {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .take(6)
        .collect::<Vec<_>>();
    excerpt.join("\n")
}

fn swc_code_frame_line(line: &str) -> bool {
    if line.starts_with('|') {
        return true;
    }
    let Some((left, _)) = line.split_once('|') else {
        return false;
    };
    let left = left.trim();
    left.is_empty() || left.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_failed_to_compile_module_error(lines: &[&str], errors: &mut Vec<CompileError>) {
    let Some(failed_index) = lines
        .iter()
        .position(|line| trim_compile_line(line).contains("Failed to compile"))
    else {
        return;
    };
    let Some(message_index) =
        lines
            .iter()
            .enumerate()
            .skip(failed_index + 1)
            .find_map(|(line_index, line)| {
                let line = trim_compile_line(line);
                (!line.is_empty()
                    && (line.starts_with("Error:")
                        || line.contains("Syntax Error")
                        || line.contains("Syntax error:")
                        || line.contains("Type error:")))
                .then_some(line_index)
            })
    else {
        return;
    };
    let path_before_message = lines
        .iter()
        .enumerate()
        .skip(failed_index + 1)
        .take(message_index.saturating_sub(failed_index + 1))
        .filter_map(|(_, line)| {
            normalize_compile_error_path(trim_compile_line(line))
                .filter(|path| compile_error_path_is_supported(path))
        })
        .next_back();
    let details = compile_error_details_from_message_line(lines, message_index);
    let Some((path, line, column)) = details
        .locator
        .as_ref()
        .map(|(path, line, column)| (path.clone(), *line, *column))
        .or_else(|| path_before_message.map(|path| (path, 0, 0)))
    else {
        return;
    };
    push_compile_error(
        errors,
        CompileError {
            path,
            line,
            column,
            excerpt: details.excerpt.clone(),
            symbol: cannot_find_name_symbol(&details.message),
            message: details.message,
            route_bound: None,
        },
    );
}

fn parse_swc_source_frame_errors(lines: &[&str], errors: &mut Vec<CompileError>) {
    for (message_index, raw) in lines.iter().enumerate() {
        let line = trim_compile_line(raw);
        let matched = !line.is_empty()
            && (line.starts_with("Error:")
                || line.contains("Syntax Error")
                || line.contains("Syntax error:"));
        if !matched {
            continue;
        }
        let details = compile_error_details_from_message_line(lines, message_index);
        let Some((path, line, column)) = details.locator.as_ref().cloned() else {
            continue;
        };
        push_compile_error(
            errors,
            CompileError {
                path,
                line,
                column,
                excerpt: details.excerpt.clone(),
                symbol: cannot_find_name_symbol(&details.message),
                message: details.message,
                route_bound: None,
            },
        );
    }
}

fn normalize_compile_error_path(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim_start_matches("./");
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.starts_with('/') {
        trimmed
            .find("/src/")
            .map(|index| trimmed[index + 1..].to_string())
            .or_else(|| {
                trimmed
                    .find("/app/")
                    .map(|index| trimmed[index + 1..].to_string())
            })
            .or_else(|| {
                trimmed
                    .find("/pages/")
                    .map(|index| trimmed[index + 1..].to_string())
            })?
    } else {
        trimmed.to_string()
    };
    Some(normalized.replace('\\', "/"))
}

fn compile_error_path_is_supported(path: &str) -> bool {
    let path = Path::new(path);
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("tsx" | "ts" | "jsx" | "js" | "css")
    )
}

fn cannot_find_name_symbol(message: &str) -> Option<String> {
    extract_quoted_after(message, "Cannot find name ")
}

fn extract_quoted_after(message: &str, marker: &str) -> Option<String> {
    let rest = message.split_once(marker)?.1.trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    let symbol = rest[..end].trim();
    (!symbol.is_empty()).then(|| symbol.to_string())
}

fn trim_compile_line(line: &str) -> &str {
    line.trim()
        .trim_start_matches('>')
        .trim()
        .trim_start_matches("at ")
        .trim()
}

fn strip_ansi_sequences(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_compile_errors_for_requirement(
    requirement: &BuildVerifierRequirement,
    output: &str,
) -> Vec<CompileError> {
    let profile = profile_for_build_requirement(requirement);
    let errors = profile.parse_compile_errors(output);
    if errors.is_empty() {
        parse_compile_errors(output)
    } else {
        errors
    }
}

pub fn is_build_verifier_command(command: &str) -> bool {
    build_oracle_for_command(None, command).is_some()
}

pub fn requires_next_binary(command: &str) -> bool {
    crate::planner::profile::requires_next_binary(command)
}

fn dependency_setup_requirement(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
) -> dependency_setup::NodeDependencySetupRequirement {
    profile_for_build_requirement(requirement)
        .dependency_setup_requirement(root, requirement, setup_authority)
        .unwrap_or_else(|| {
            dependency_setup::requirement_for_node_declared_dependencies(
                root,
                requirement.profile.as_deref(),
                &requirement.reason,
                setup_authority,
            )
        })
}

pub fn is_dependency_missing_output(output: &str) -> bool {
    crate::planner::profile::domain_profile("generic").dependency_missing_output(output)
        || crate::planner::profile::domain_profile("python-cli").dependency_missing_output(output)
}

pub fn requires_node_dependency_probe(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    (lower.contains("node -e") && lower.contains("require(")) || lower.contains("npx --no-install")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

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
    fn deferred_next_build_becomes_required_build_verifier() {
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        assert!(requirement.required_for_completion);
        assert!(requirement.requires_dependency_setup);
    }

    #[test]
    fn static_profile_coverage_status_does_not_disable_build_verifier() {
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "legacy static coverage marker",
            "profile:nextjs",
            "covered_by_static_profile_check",
        )
        .unwrap();
        assert!(requirement.required_for_completion);
    }

    #[test]
    fn parse_next_type_error_extracts_path_line_column_and_symbol() {
        let output = r#"
> next build

Failed to compile.

./src/components/SpaceInvaders.tsx:137:28
Type error: Cannot find name 'reset'.
"#;

        let errors = parse_compile_errors(output);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "src/components/SpaceInvaders.tsx");
        assert_eq!(errors[0].line, 137);
        assert_eq!(errors[0].column, 28);
        assert_eq!(errors[0].symbol.as_deref(), Some("reset"));
        assert_eq!(errors[0].route_bound, None);
    }

    #[test]
    fn parse_swc_bare_error_frame_extracts_message_location_and_excerpt() {
        let output = r#"
> next build

Failed to compile.

Error:
  x Expected ';', '}' or <eof>
   ,-[/tmp/anvil/src/app/page.tsx:12:1]
 9 |   return (
10 |     <main>
11 |       <button>Start</button>
12 |     </main>
   |     ^
13 |   )
"#;

        let errors = parse_compile_errors(output);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "src/app/page.tsx");
        assert_eq!(errors[0].line, 12);
        assert_eq!(errors[0].column, 1);
        assert_eq!(errors[0].message, "Expected ';', '}' or <eof>");
        assert!(errors[0].excerpt.contains("12 |"), "{errors:?}");
        assert!(errors[0].excerpt.contains("|     ^"), "{errors:?}");
    }

    #[test]
    fn parse_swc_path_header_frame_without_failed_compile_banner() {
        let output = r#"
outcome: CommandFailed
status: exit status: 1
summary: command failed
stdout:
./src/app/game.ts
Error:
  x Expected ',', got '}'

   ,-[./src/app/game.ts:631:1]
628 |   const asteroids = [
629 |     { x: 10, y: 20 },
630 |     { x: 30, y: 40 }
631 |   }
    |   ^
632 |   return asteroids
   `----
> Build failed because of webpack errors
stderr:
"#;

        let errors = parse_compile_errors(output);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "src/app/game.ts");
        assert_eq!(errors[0].line, 631);
        assert_eq!(errors[0].column, 1);
        assert_eq!(errors[0].message, "Expected ',', got '}'");
        assert!(errors[0].excerpt.contains("631 |   }"), "{errors:?}");
        assert!(errors[0].excerpt.contains("|   ^"), "{errors:?}");
    }

    #[test]
    fn next_build_reports_dependency_missing_before_execution() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let observation = observe_requirement(dir.path(), &requirement);
        assert_eq!(observation.status, BuildVerifierStatus::DependencyMissing);
        assert!(!observation.attempted);
    }

    #[test]
    fn next_build_without_manifest_reports_manifest_boundary_before_execution() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();

        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );

        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert!(!lifecycle.before_setup.attempted);
        assert!(
            lifecycle
                .before_setup
                .primary_reason
                .contains("package.json missing before Next.js build verifier"),
            "{lifecycle:?}"
        );
        assert_eq!(lifecycle.setup_status(), "blocked");
        assert_eq!(
            lifecycle
                .setup
                .as_ref()
                .map(|setup| setup.primary_reason.as_str()),
            Some("package.json missing")
        );
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_dependency_missing")
        );
    }

    #[test]
    fn dependency_missing_then_setup_blocked_records_lifecycle_taxonomy() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "node_modules/.bin/next build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert_eq!(lifecycle.setup_status(), "blocked");
        assert_eq!(
            lifecycle.lifecycle_stages(),
            vec![
                "dependency_check",
                "setup_authority_missing",
                "setup_blocked",
                "verification_dependency_missing"
            ]
        );
    }

    #[test]
    fn next_build_with_tailwind_requires_tailwind_node_modules_not_only_next_binary() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/next")).unwrap();
        std::fs::write(dir.path().join("node_modules/.bin/next"), "").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let observation = observe_requirement(dir.path(), &requirement);
        assert_eq!(observation.status, BuildVerifierStatus::DependencyMissing);
        assert!(!observation.attempted);
        assert!(
            observation
                .primary_reason
                .contains("node_modules/tailwindcss"),
            "{observation:?}"
        );
    }

    #[test]
    fn nextjs_build_with_foreign_next_on_path_is_dependency_missing_and_emits_event() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        let foreign_bin = dir.path().join("foreign/node_modules/.bin");
        std::fs::create_dir_all(&foreign_bin).unwrap();
        write_executable(&foreign_bin.join("next"), "#!/bin/sh\nexit 0\n");
        let events = dir.path().join("events.jsonl");
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let child_path = std::env::join_paths(
            std::iter::once(foreign_bin.clone()).chain(std::env::split_paths(&original_path)),
        )
        .unwrap();

        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "minimal_loop::build_verifier::tests::nextjs_build_with_foreign_next_on_path_child",
                "--nocapture",
            ])
            .env("ANVIL_FOREIGN_TOOLCHAIN_ROOT", dir.path())
            .env("ANVIL_FOREIGN_TOOLCHAIN_EVENTS", &events)
            .env("PATH", child_path)
            .status()
            .unwrap();
        assert!(status.success(), "{status}");

        let text = std::fs::read_to_string(events).unwrap();
        assert!(text.contains("\"event\":\"foreign_toolchain_detected\""));
        assert!(text.contains("\"status\":\"dependency_missing\""));
        assert!(text.contains("foreign_toolchain_detected"));
    }

    #[test]
    #[ignore]
    fn nextjs_build_with_foreign_next_on_path_child() {
        let root = std::env::var_os("ANVIL_FOREIGN_TOOLCHAIN_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap();
        let events = std::env::var_os("ANVIL_FOREIGN_TOOLCHAIN_EVENTS")
            .map(std::path::PathBuf::from)
            .unwrap();
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();

        let lifecycle =
            observe_requirement_lifecycle(&root, &requirement, NodeDependencySetupAuthority::None);

        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert!(!lifecycle.before_setup.attempted);
        assert!(
            lifecycle.before_setup.foreign_toolchain.is_some(),
            "{lifecycle:?}"
        );
        emit_dependency_build_lifecycle(Some(&events), "minimal-loop", Some("step"), &lifecycle);
    }

    #[test]
    fn dependency_missing_setup_allowed_then_build_rerun_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm.sh");
        std::fs::write(
            &fake_npm,
            "#!/bin/sh\nmkdir -p node_modules/.bin node_modules/next\ncat > node_modules/next/package.json <<'EOF'\n{\"version\":\"14.2.0\"}\nEOF\ncat > node_modules/.bin/next <<'EOF'\n#!/bin/sh\nexit 0\nEOF\nchmod +x node_modules/.bin/next\ntouch package-lock.json\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_npm).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_npm, perms).unwrap();
        }
        let requirement = requirement_from_deferred(
            "node_modules/.bin/next build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle_with_setup_program(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::EvalExplicit,
            &fake_npm,
        );
        assert_eq!(lifecycle.setup_status(), "passed");
        assert_eq!(lifecycle.final_status, BuildVerifierStatus::Passed);
        assert!(lifecycle.after_setup.is_some());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_passed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"build_rerun_attempted")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"build_rerun"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_passed")
        );
    }

    #[test]
    fn dependency_missing_setup_failed_records_attempted_and_failed_stages() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm-fail.sh");
        std::fs::write(&fake_npm, "#!/bin/sh\necho install failed >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_npm).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_npm, perms).unwrap();
        }
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle_with_setup_program(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::EvalExplicit,
            &fake_npm,
        );
        assert_eq!(lifecycle.setup_status(), "failed");
        assert_eq!(
            lifecycle.final_status,
            BuildVerifierStatus::DependencyMissing
        );
        assert!(lifecycle.after_setup.is_none());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_failed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_dependency_missing")
        );
    }

    #[test]
    fn node_test_runner_missing_manifest_setup_blocked_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import test from 'node:test';\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm test",
            Some("js"),
            "node test verifier",
            "profile:js",
            "required",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert_eq!(lifecycle.setup_status(), "blocked");
        assert_eq!(
            lifecycle
                .setup
                .as_ref()
                .map(|setup| setup.setup_kind.as_str()),
            Some("node_test_runner_manifest")
        );
        assert_eq!(
            lifecycle.lifecycle_stages(),
            vec![
                "dependency_check",
                "setup_authority_missing",
                "setup_blocked",
                "verification_dependency_missing"
            ]
        );
    }

    #[test]
    fn node_test_runner_setup_allowed_then_test_rerun_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('ok', () => assert.equal(1, 1));\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm test",
            Some("js"),
            "node test verifier",
            "profile:js",
            "required",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert_eq!(lifecycle.setup_status(), "passed");
        assert_eq!(lifecycle.final_status, BuildVerifierStatus::Passed);
        assert!(lifecycle.after_setup.is_some());
        assert!(dir.path().join("package.json").is_file());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_passed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"build_rerun_attempted")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"build_rerun"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_passed")
        );
    }

    #[test]
    fn dependency_build_lifecycle_event_uses_same_taxonomy_for_modes() {
        let dir = TempDir::new().unwrap();
        let events = dir.path().join("events.jsonl");
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        for mode in ["minimal-loop", "plan-run", "ultra-plan-run"] {
            emit_dependency_build_lifecycle(Some(&events), mode, Some("step"), &lifecycle);
        }
        let text = std::fs::read_to_string(events).unwrap();
        assert_eq!(
            text.matches("\"event\":\"dependency_build_lifecycle\"")
                .count(),
            3
        );
        assert!(text.contains("\"mode\":\"minimal-loop\""));
        assert!(text.contains("\"mode\":\"plan-run\""));
        assert!(text.contains("\"mode\":\"ultra-plan-run\""));
        assert!(text.contains("\"lifecycle_stage\":\"dependency_setup_build\""));
        assert!(text.contains("setup_blocked"));
    }

    #[test]
    fn shell_control_syntax_is_policy_rejected() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "npm run build && npm test",
            Some("nextjs"),
            "bad command",
            "test",
            "pending",
        )
        .unwrap();
        let observation = observe_requirement(dir.path(), &requirement);
        assert_eq!(observation.status, BuildVerifierStatus::PolicyRejected);
    }
}
