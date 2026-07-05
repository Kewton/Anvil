use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::build_verifier::{
    self, BuildVerifierLifecycleObservation, BuildVerifierStatus, CompileError,
};
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::evidence::{RuntimeAcceptanceReport, required_evidence_for_capability};
use crate::minimal_loop::repair_target::{RepairTarget, classify_repair_target};
use crate::planner::verify::{VerificationReport, validate_verify_command};
use crate::tools::path_guard::{
    resolve_existing, resolve_optional_existing, validate_workspace_relative,
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompletionContract {
    #[serde(default)]
    pub required_paths: Vec<String>,
    #[serde(default)]
    pub verify_commands: Vec<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub deterministic_oracles: Vec<String>,
    #[serde(default)]
    pub required_evidence: Vec<String>,
    #[serde(default)]
    pub evidence_hint_tokens: Vec<String>,
    #[serde(default)]
    pub required_obligations: Vec<String>,
    #[serde(default)]
    pub deferred_verify_requirements: Vec<DeferredVerifyRequirement>,
    #[serde(default = "default_verify_repair_cap")]
    pub verify_repair_cap: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeferredVerifyRequirement {
    pub command: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub authority: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "default_deferred_status")]
    pub status: String,
}

impl CompletionContract {
    pub fn load_for_config(config: &Config) -> anyhow::Result<Option<Self>> {
        let path = config
            .completion_contract_path
            .clone()
            .or_else(|| std::env::var_os("ANVIL_COMPLETION_CONTRACT").map(PathBuf::from));
        let Some(path) = path else {
            return Ok(None);
        };
        let path = normalize_contract_file_path(&config.workspace_root, &path)?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read completion contract {}", path.display()))?;
        let contract: Self = serde_json::from_str(&text)
            .with_context(|| format!("invalid completion contract JSON {}", path.display()))?;
        Ok(Some(contract.validate(&config.workspace_root)?))
    }

    pub fn validate(mut self, root: &Path) -> anyhow::Result<Self> {
        let mut seen_paths = BTreeSet::new();
        let mut paths = Vec::new();
        for path in self.required_paths {
            validate_contract_path(root, &path)?;
            if seen_paths.insert(path.clone()) {
                paths.push(path);
            }
        }
        let mut seen_commands = BTreeSet::new();
        let mut commands = Vec::new();
        for command in self.verify_commands {
            validate_verify_command(&command)?;
            if seen_commands.insert(command.clone()) {
                commands.push(command);
            }
        }
        self.required_paths = paths;
        self.verify_commands = commands;
        self.required_capabilities = normalize_unique_list(self.required_capabilities);
        self.deterministic_oracles = normalize_unique_list(self.deterministic_oracles);
        self.required_evidence = normalize_unique_list(self.required_evidence);
        self.evidence_hint_tokens = normalize_evidence_hint_tokens(self.evidence_hint_tokens);
        if let Some(goal) = self.goal.clone() {
            self.merge_evidence_hint_tokens_from_goal(&goal);
        }
        self.required_obligations = normalize_obligation_roles(self.required_obligations)?;
        let mut evidence_seen = self
            .required_evidence
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for capability in &self.required_capabilities {
            for evidence in required_evidence_for_capability(capability) {
                if evidence_seen.insert(evidence.clone()) {
                    self.required_evidence.push(evidence);
                }
            }
        }
        let mut seen_deferred = BTreeSet::new();
        let mut deferred = Vec::new();
        for mut requirement in self.deferred_verify_requirements {
            validate_verify_command(&requirement.command)?;
            requirement.reason = requirement.reason.trim().to_string();
            requirement.authority = requirement.authority.trim().to_string();
            requirement.status = requirement.status.trim().to_string();
            if requirement.status.is_empty() {
                requirement.status = default_deferred_status();
            }
            let key = (
                requirement.command.clone(),
                requirement.authority.clone(),
                requirement.profile.clone().unwrap_or_default(),
            );
            if seen_deferred.insert(key) {
                deferred.push(requirement);
            }
        }
        if let Some(profile) = self.profile.take() {
            let trimmed = profile.trim();
            if !trimmed.is_empty() {
                self.profile = Some(trimmed.to_string());
            }
        }
        self.deferred_verify_requirements = deferred;
        if self.verify_repair_cap == 0 {
            self.verify_repair_cap = default_verify_repair_cap();
        }
        Ok(self)
    }

    pub fn has_verify(&self) -> bool {
        !self.verify_commands.is_empty()
            || self.profile_requires_completion_gate()
            || !self.required_capabilities.is_empty()
            || !self.required_evidence.is_empty()
            || !self.required_obligations.is_empty()
            || !self.deterministic_oracles.is_empty()
            || !self.deferred_verify_requirements.is_empty()
    }

    pub fn dependency_precondition_active(&self, root: &Path) -> bool {
        self.verify_commands.iter().any(|command| {
            build_verifier::requires_next_binary(command)
                && !crate::minimal_loop::dependency_setup::next_build_dependencies_ready(root)
        })
    }

    pub fn verify(&self, root: &Path) -> VerificationReport {
        self.verify_with_goal(root, "")
    }

    pub fn verify_with_goal(&self, root: &Path, fallback_goal: &str) -> VerificationReport {
        self.verify_with_goal_observed(root, fallback_goal).0
    }

    pub fn verify_with_goal_observed(
        &self,
        root: &Path,
        fallback_goal: &str,
    ) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
        self.verify_with_goal_observed_with_setup_authority(
            root,
            fallback_goal,
            NodeDependencySetupAuthority::None,
            false,
        )
    }

    pub fn verify_with_goal_observed_with_setup_authority(
        &self,
        root: &Path,
        fallback_goal: &str,
        setup_authority: NodeDependencySetupAuthority,
        offline: bool,
    ) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
        self.verify_with_goal_observed_inner(
            root,
            fallback_goal,
            setup_authority,
            Path::new("npm"),
            offline,
        )
    }

    #[cfg(test)]
    pub(crate) fn verify_with_goal_observed_with_setup_program_and_authority(
        &self,
        root: &Path,
        fallback_goal: &str,
        setup_authority: NodeDependencySetupAuthority,
        npm_program: &Path,
        offline: bool,
    ) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
        self.verify_with_goal_observed_inner(
            root,
            fallback_goal,
            setup_authority,
            npm_program,
            offline,
        )
    }

    fn verify_with_goal_observed_inner(
        &self,
        root: &Path,
        fallback_goal: &str,
        setup_authority: NodeDependencySetupAuthority,
        npm_program: &Path,
        offline: bool,
    ) -> (VerificationReport, Vec<BuildVerifierLifecycleObservation>) {
        let mut report = VerificationReport::pass();
        let mut build_verifier_observations = Vec::new();
        for path in &self.required_paths {
            if resolve_existing(root, path).is_err() {
                report.push_missing_path(path.clone());
            }
        }
        for command in &self.verify_commands {
            if let Err(err) = validate_verify_command(command) {
                report.push_command_failure(command.clone(), err.to_string());
                continue;
            }
            if let Some(build_requirement) = build_verifier::requirement_from_deferred(
                command,
                self.active_profile(),
                "completion verify command",
                "completion_contract",
                "required",
            ) {
                let lifecycle =
                    build_verifier::observe_requirement_lifecycle_with_setup_program_and_offline(
                        root,
                        &build_requirement,
                        setup_authority,
                        npm_program,
                        offline,
                    );
                let observation = lifecycle.final_observation();
                if observation.status != BuildVerifierStatus::Passed {
                    match observation.status {
                        BuildVerifierStatus::DependencyMissing => {
                            report.push_dependency_missing(format!(
                                "dependency_setup_missing: {}",
                                lifecycle.final_reason
                            ));
                        }
                        BuildVerifierStatus::PolicyRejected => {
                            report.push_command_failure(
                                command.clone(),
                                format!("build_verify_policy_rejected: {}", lifecycle.final_reason),
                            );
                        }
                        BuildVerifierStatus::Blocked => {
                            report.push_profile_failure(format!(
                                "build_verify_blocked: command `{}` reason `{}`",
                                command, lifecycle.final_reason
                            ));
                        }
                        BuildVerifierStatus::Failed => {
                            if observation.compile_errors.is_empty() {
                                report.push_command_failure(
                                    command.clone(),
                                    format!("build_verify_failed: {}", lifecycle.final_reason),
                                );
                            } else {
                                report.push_compile_errors(
                                    command.clone(),
                                    observation.compile_errors.clone(),
                                );
                            }
                        }
                        BuildVerifierStatus::Passed => {}
                    }
                }
                build_verifier_observations.push(lifecycle);
                continue;
            }
            match crate::minimal_loop::verifier_env::run_checked(command, root, offline) {
                Ok(output) => {
                    if command.contains("npm") && output.contains("0 tests") {
                        report.push_command_failure(command.clone(), "Node 0 tests rejected");
                    } else if let Some(reason) =
                        classify_python_test_discovery_failure(root, command, &output)
                    {
                        report.push_command_failure(command.clone(), reason);
                    }
                }
                Err(err) if is_dependency_missing_error(command, &err.to_string()) => {
                    report.push_dependency_missing(command.clone());
                }
                Err(err) => {
                    let reason = err.to_string();
                    if let Some(reason) =
                        classify_python_test_discovery_failure(root, command, &reason)
                    {
                        report.push_command_failure(command.clone(), reason);
                    } else {
                        report.push_command_failure(command.clone(), reason);
                    }
                }
            }
        }
        let mut profile_passed = false;
        if let Some(profile) = self.active_profile() {
            let goal = self.goal.as_deref().unwrap_or(fallback_goal);
            let profile_report = crate::planner::profile::verify_profile(root, profile, goal);
            if profile_report.is_pass() {
                profile_passed = true;
            } else {
                for reason in profile_report.profile_failures {
                    report.push_profile_failure(reason);
                }
                for reason in profile_report.dependency_missing {
                    report.push_dependency_missing(reason);
                }
                for failure in profile_report.command_failures {
                    report.push_command_failure(failure.command, failure.reason);
                }
                for error in profile_report.compile_errors {
                    if !report.compile_errors.contains(&error) {
                        report.compile_errors.push(error);
                    }
                }
                for path in profile_report.missing_paths {
                    report.push_missing_path(path);
                }
                report.refresh_status();
            }
        }
        for requirement in &self.deferred_verify_requirements {
            if let Some(build_requirement) = build_verifier::requirement_from_deferred(
                &requirement.command,
                requirement.profile.as_deref(),
                &requirement.reason,
                &requirement.authority,
                &requirement.status,
            ) {
                let lifecycle =
                    build_verifier::observe_requirement_lifecycle_with_setup_program_and_offline(
                        root,
                        &build_requirement,
                        setup_authority_for_deferred(requirement, setup_authority),
                        npm_program,
                        offline,
                    );
                let observation = lifecycle.final_observation();
                if build_requirement.required_for_completion
                    && observation.status != BuildVerifierStatus::Passed
                {
                    match observation.status {
                        BuildVerifierStatus::DependencyMissing => {
                            report.push_dependency_missing(format!(
                                "dependency_setup_missing: {}",
                                observation.primary_reason
                            ));
                        }
                        BuildVerifierStatus::PolicyRejected => {
                            report.push_command_failure(
                                requirement.command.clone(),
                                format!(
                                    "build_verify_policy_rejected: {}",
                                    observation.primary_reason
                                ),
                            );
                        }
                        BuildVerifierStatus::Blocked => {
                            report.push_profile_failure(format!(
                                "build_verify_blocked: command `{}` reason `{}`",
                                requirement.command, observation.primary_reason
                            ));
                        }
                        BuildVerifierStatus::Failed => {
                            if observation.compile_errors.is_empty() {
                                report.push_command_failure(
                                    requirement.command.clone(),
                                    format!("build_verify_failed: {}", observation.primary_reason),
                                );
                            } else {
                                report.push_compile_errors(
                                    requirement.command.clone(),
                                    observation.compile_errors.clone(),
                                );
                            }
                        }
                        BuildVerifierStatus::Passed => {}
                    }
                }
                build_verifier_observations.push(lifecycle);
                continue;
            }
            if self.deferred_requirement_covered(root, requirement, profile_passed) {
                continue;
            }
            report.push_profile_failure(self.deferred_requirement_blocking_reason(
                root,
                requirement,
                profile_passed,
            ));
        }
        let acceptance = self.runtime_acceptance_report(root);
        if !acceptance.passed {
            if !acceptance.missing_capabilities.is_empty() {
                report.push_profile_failure(format!(
                    "missing_required_capabilities:{}",
                    acceptance.missing_capabilities.join(",")
                ));
            }
            if !acceptance.missing_evidence.is_empty() {
                report.push_profile_failure(format!(
                    "missing_required_evidence:{}",
                    acceptance.missing_evidence.join(",")
                ));
            }
            if !acceptance.weak_evidence.is_empty() {
                report.push_profile_failure(format!(
                    "weak_verification_evidence:{}",
                    acceptance.weak_evidence.join(",")
                ));
            }
            if !acceptance.missing_obligations.is_empty() {
                report.push_profile_failure(format!(
                    "missing_required_evidence:required_obligation:{}",
                    acceptance.missing_obligations.join(",")
                ));
            }
            for target in &acceptance.obligation_repair_targets {
                report.push_profile_failure(format!(
                    "missing_required_obligation_target:{}:{}",
                    target.obligation, target.target_path
                ));
            }
            if !acceptance.inconclusive_reasons.is_empty() {
                report.push_profile_failure(format!(
                    "inconclusive_acceptance:{}",
                    acceptance.inconclusive_reasons.join(",")
                ));
            }
        }
        (report, build_verifier_observations)
    }

    pub fn runtime_acceptance_report(&self, root: &Path) -> RuntimeAcceptanceReport {
        let deferred_commands = self
            .deferred_verify_requirements
            .iter()
            .map(|requirement| requirement.command.clone())
            .collect::<Vec<_>>();
        crate::minimal_loop::evidence::verify_runtime_acceptance_with_hints(
            root,
            &self.required_paths,
            &self.verify_commands,
            &self.required_capabilities,
            &self.required_evidence,
            &self.required_obligations,
            &deferred_commands,
            &self.evidence_hint_tokens,
        )
    }

    pub fn merge_evidence_hint_tokens_from_goal(&mut self, goal: &str) {
        let mut seen = self
            .evidence_hint_tokens
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for token in evidence_hint_tokens_for_goal(goal) {
            if seen.insert(token.clone()) {
                self.evidence_hint_tokens.push(token);
            }
        }
        self.evidence_hint_tokens =
            normalize_evidence_hint_tokens(std::mem::take(&mut self.evidence_hint_tokens));
    }

    pub fn deferred_status_summary(&self, root: &Path, fallback_goal: &str) -> Vec<String> {
        let profile_passed = self
            .active_profile()
            .map(|profile| {
                let goal = self.goal.as_deref().unwrap_or(fallback_goal);
                crate::planner::profile::verify_profile(root, profile, goal).is_pass()
            })
            .unwrap_or(false);
        self.deferred_verify_requirements
            .iter()
            .map(|requirement| {
                let status = if let Some(build_requirement) =
                    build_verifier::requirement_from_deferred(
                        &requirement.command,
                        requirement.profile.as_deref(),
                        &requirement.reason,
                        &requirement.authority,
                        &requirement.status,
                    ) {
                    if !build_requirement.required_for_completion {
                        build_requirement.status
                    } else if build_requirement.requires_dependency_setup
                        && build_verifier::requires_next_binary(&build_requirement.command)
                        && !crate::minimal_loop::dependency_setup::next_build_dependencies_ready(
                            root,
                        )
                    {
                        "dependency_setup_missing".to_string()
                    } else {
                        "build_verifier_required".to_string()
                    }
                } else if self.deferred_requirement_covered(root, requirement, profile_passed) {
                    "covered_by_static_profile_check".to_string()
                } else {
                    requirement.status.clone()
                };
                format!("{}:{status}", requirement.command)
            })
            .collect()
    }

    fn active_profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
            .filter(|profile| !profile_is_generic(profile))
    }

    fn profile_requires_completion_gate(&self) -> bool {
        self.active_profile().is_some()
    }

    fn deferred_requirement_covered(
        &self,
        root: &Path,
        requirement: &DeferredVerifyRequirement,
        profile_passed: bool,
    ) -> bool {
        if requirement.status == "covered_by_static_profile_check" {
            return true;
        }
        let _ = (root, profile_passed);
        false
    }

    fn deferred_requirement_blocking_reason(
        &self,
        root: &Path,
        requirement: &DeferredVerifyRequirement,
        profile_passed: bool,
    ) -> String {
        if !profile_passed {
            return format!(
                "deferred verify requirement pending: command `{}` status `{}` reason `{}`",
                requirement.command, requirement.status, requirement.reason
            );
        }
        let _ = root;
        format!(
            "deferred verify requirement pending: command `{}` status `{}` reason `{}`",
            requirement.command, requirement.status, requirement.reason
        )
    }
}

fn setup_authority_for_deferred(
    requirement: &DeferredVerifyRequirement,
    fallback: NodeDependencySetupAuthority,
) -> NodeDependencySetupAuthority {
    let authority = requirement.authority.to_ascii_lowercase();
    if authority.contains("eval") && authority.contains("setup") {
        NodeDependencySetupAuthority::EvalExplicit
    } else if authority.contains("completion") && authority.contains("setup") {
        NodeDependencySetupAuthority::CompletionContract
    } else if authority.contains("tui") && authority.contains("setup") {
        NodeDependencySetupAuthority::TuiConfirmed
    } else {
        fallback
    }
}

pub fn format_verify_feedback(report: &VerificationReport) -> String {
    format_verify_feedback_with_contract(report, None)
}

pub(crate) fn format_verify_feedback_with_contract(
    report: &VerificationReport,
    contract: Option<&CompletionContract>,
) -> String {
    let mut lines = vec![
        "Deterministic completion verification failed. Fix the implementation and retry."
            .to_string(),
    ];
    let target = classify_repair_target(report);
    lines.push(format!(
        "Repair target: {}. {}",
        target.as_str(),
        target.guidance()
    ));
    if matches!(
        target,
        RepairTarget::CapabilityMissing | RepairTarget::EmptyApp | RepairTarget::Implementation
    ) {
        lines.push(format!(
            "Target implementation files: {}",
            target_implementation_files(report, contract).join(", ")
        ));
    }
    if !report.missing_paths.is_empty() {
        lines.push(format!(
            "Missing required paths: {}",
            report.missing_paths.join(", ")
        ));
    }
    if !report.compile_errors.is_empty() {
        lines.push("Compile repair details:".to_string());
        lines.extend(
            compile_repair_prompt_section(
                &report.compile_errors,
                CompileRepairPromptProtection::default(),
            )
            .lines()
            .map(str::to_string),
        );
    }
    for reason in &report.dependency_missing {
        lines.push(format!("Dependency missing: {reason}"));
    }
    for failure in &report.command_failures {
        if failure
            .reason
            .contains("test_framework_mismatch:pytest_style_under_unittest")
        {
            lines.push(
                "Test framework mismatch: `python3 -m unittest` does not discover pytest-style free functions. Convert tests to `unittest.TestCase` methods or update the allowed verify command."
                    .to_string(),
            );
        }
        lines.push(format!(
            "Command failed: `{}`\n{}",
            failure.command,
            eval_events::body_snippet(&failure.reason)
        ));
        let targets = target_candidates_from_failure(&failure.command, &failure.reason);
        if !targets.is_empty() {
            lines.push(format!(
                "Likely target files: {}",
                targets.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some(excerpt) = assertion_excerpt(&failure.reason) {
            lines.push(format!(
                "Failure excerpt: {}",
                eval_events::body_snippet(&excerpt)
            ));
        }
    }
    for failure in &report.verifier_command_false_negatives {
        lines.push(format!(
            "Verifier command false negative: `{}`\n{}",
            failure.command,
            eval_events::body_snippet(&failure.reason)
        ));
        if failure.reason.contains("verify_command_timeout") {
            lines.push(
                "The verify command hangs - replace it with a bounded check. Do not spend implementation-edit turns on this OracleError unless a separate artifact failure is present."
                    .to_string(),
            );
        } else {
            lines.push(
                "The verify command is malformed; the artifact may already satisfy the requirement."
                    .to_string(),
            );
        }
    }
    for failure in &report.profile_failures {
        lines.push(format!("Profile contract failed: {failure}"));
        if failure.contains("CSS side-effect imports require")
            && failure.contains("declare module \"*.css\"")
        {
            lines.push(
                "Next.js CSS repair: replace or create `src/app/global.d.ts` with exactly `declare module \"*.css\";` on its own line."
                    .to_string(),
            );
        }
        if failure.contains("must start with \"use client\"") {
            lines.push(
                "Next.js client component repair: put `\"use client\";` as the first non-empty statement in the interactive app page before imports."
                    .to_string(),
            );
        }
        if failure.contains("missing_required_capabilities")
            || failure.contains("missing_required_evidence")
            || failure.contains("weak_verification_evidence")
        {
            lines.push(
                "Capability evidence guidance: add the smallest concrete test/check/UI evidence requested by the task, then keep the existing verification strong enough to exercise that evidence."
                    .to_string(),
            );
            lines.extend(evidence_repair_guidance(failure));
        }
    }
    lines.push(
        "Repair guidance: inspect the smallest relevant file range, then make a concrete Write/Edit change to the implementation, setup, or generated test artifact that actually owns the failure. Do not bypass verification or weaken assertions without evidence that the generated test contradicts the requested behavior."
            .to_string(),
    );
    lines.join("\n")
}

pub(crate) fn target_implementation_files(
    report: &VerificationReport,
    contract: Option<&CompletionContract>,
) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for error in &report.compile_errors {
        if looks_like_implementation_target_path(&error.path) {
            paths.insert(error.path.clone());
        }
    }
    if !paths.is_empty() {
        return paths.into_iter().collect();
    }
    if let Some(contract) = contract {
        paths.extend(
            contract
                .required_paths
                .iter()
                .filter(|path| looks_like_implementation_target_path(path))
                .cloned(),
        );
    }
    if paths.is_empty() {
        paths.extend(
            report
                .missing_paths
                .iter()
                .filter(|path| looks_like_implementation_target_path(path))
                .cloned(),
        );
    }
    if paths.is_empty() {
        for failure in &report.command_failures {
            paths.extend(
                target_candidates_from_failure(&failure.command, &failure.reason)
                    .into_iter()
                    .filter(|path| looks_like_implementation_target_path(path)),
            );
        }
    }
    if paths.is_empty() {
        paths.insert("src/app/page.tsx".to_string());
    }
    paths.into_iter().collect()
}

pub(crate) fn compile_error_repair_guidance(errors: &[CompileError]) -> Vec<String> {
    errors
        .iter()
        .flat_map(|error| {
            let mut lines = vec![
                format!("Compile error: {}", error.summary()),
                format!("Compile error location: {}", error.location()),
                format!("Compile error message: {}", error.message),
            ];
            if !error.excerpt.trim().is_empty() {
                lines.push(format!(
                    "Compile error excerpt for {}:\n{}",
                    error.location(),
                    error.excerpt.trim()
                ));
            }
            lines.push(format!(
                "You MUST modify {} using the edit tool; a reply without file edits fails this repair.",
                error.path
            ));
            if let Some(symbol) = error.symbol.as_deref() {
                let route_note = if error.route_bound == Some(false) {
                    " - the file is not imported by any route"
                } else {
                    ""
                };
                lines.push(format!(
                    "Cannot-find-name repair for `{symbol}` in {}: define {symbol}, or replace the reference with an existing handler, or remove the dead code{route_note}.",
                    error.location()
                ));
            }
            lines
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CompileRepairPromptProtection {
    pub reanchored_retry: bool,
    pub narrow_no_snapshot_retry: bool,
}

pub(crate) fn compile_repair_prompt_section(
    errors: &[CompileError],
    protection: CompileRepairPromptProtection,
) -> String {
    if errors.is_empty() {
        return "- none".to_string();
    }
    let mut lines = compile_error_repair_guidance(errors);
    let paths = errors
        .iter()
        .map(|error| error.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    lines.push(format!(
        "Compile repair edit mandate: edit one of these source files using the edit tool: {}.",
        paths.join(", ")
    ));
    lines.push(
        "Do not answer in prose only; a repair response without a source edit fails this compile repair."
            .to_string(),
    );
    if protection.reanchored_retry {
        lines.push(format!(
            "Compile repair re-anchor: the previous compile repair turn changed no files. You MUST edit one of these source files now: {}.",
            paths.join(", ")
        ));
    }
    if protection.narrow_no_snapshot_retry {
        lines.push(
            "No rollback snapshot is available for this compile failure. This is one narrow compile-only repair turn: fix these lines; do not restructure."
                .to_string(),
        );
        lines.push(
            "Use ONLY the compile error frames above as the repair scope; avoid unrelated cleanup, redesign, dependency churn, or feature work."
                .to_string(),
        );
    }
    lines
        .into_iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_unique_list(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

pub fn evidence_hint_tokens_for_goal(goal: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for token in ascii_goal_words(goal)
        .into_iter()
        .chain(katakana_goal_tokens(goal))
    {
        if seen.insert(token.clone()) {
            out.push(token);
        }
    }
    out
}

fn normalize_evidence_hint_tokens(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = normalize_evidence_hint_token(&value);
        if !normalized.is_empty()
            && !evidence_hint_stopword(&normalized)
            && seen.insert(normalized.clone())
        {
            out.push(normalized);
        }
    }
    out
}

fn normalize_evidence_hint_token(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_ascii() {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

fn ascii_goal_words(goal: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in goal.chars() {
        if ch.is_ascii_alphabetic() {
            current.push(ch.to_ascii_lowercase());
        } else {
            push_ascii_goal_word(&mut words, &mut current);
        }
    }
    push_ascii_goal_word(&mut words, &mut current);
    normalize_evidence_hint_tokens(words)
}

fn push_ascii_goal_word(words: &mut Vec<String>, current: &mut String) {
    if current.len() >= 4 {
        words.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn katakana_goal_tokens(goal: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in goal.chars() {
        if is_katakana_hint_char(ch) {
            current.push(ch);
        } else {
            push_katakana_goal_token(&mut tokens, &mut current);
        }
    }
    push_katakana_goal_token(&mut tokens, &mut current);
    normalize_evidence_hint_tokens(tokens)
}

fn push_katakana_goal_token(tokens: &mut Vec<String>, current: &mut String) {
    if current.chars().count() >= 3 {
        let token = std::mem::take(current);
        tokens.push(token.clone());
        for prefix in KATAKANA_GOAL_PREFIX_STOPWORDS {
            if let Some(suffix) = token.strip_prefix(prefix)
                && suffix.chars().count() >= 3
            {
                tokens.push(suffix.to_string());
            }
        }
    } else {
        current.clear();
    }
}

fn is_katakana_hint_char(ch: char) -> bool {
    matches!(ch, '\u{30A0}'..='\u{30FF}' | '\u{FF66}'..='\u{FF9F}')
}

fn evidence_hint_stopword(token: &str) -> bool {
    ASCII_GOAL_HINT_STOPWORDS.contains(&token) || JAPANESE_GOAL_HINT_STOPWORDS.contains(&token)
}

const ASCII_GOAL_HINT_STOPWORDS: &[&str] = &[
    "application",
    "browser",
    "build",
    "canvas",
    "client",
    "component",
    "create",
    "develop",
    "development",
    "feature",
    "game",
    "games",
    "implement",
    "implementation",
    "interactive",
    "next",
    "nextjs",
    "page",
    "playable",
    "port",
    "project",
    "react",
    "screen",
    "shooting",
    "space",
    "typescript",
    "using",
    "with",
];

const JAPANESE_GOAL_HINT_STOPWORDS: &[&str] = &[
    "アプリ",
    "ゲーム",
    "シューティング",
    "スペース",
    "ネクスト",
    "ブラウザ",
    "ページ",
    "ポート",
    "実装",
    "作成",
    "開発",
];

const KATAKANA_GOAL_PREFIX_STOPWORDS: &[&str] = &["スペース"];

fn normalize_obligation_roles(values: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
        if normalized.is_empty() {
            continue;
        }
        if !matches!(
            normalized.as_str(),
            "setup" | "scaffold" | "implementation" | "verification" | "acceptance_evidence"
        ) {
            bail!("unsupported completion obligation role: {value}");
        }
        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }
    Ok(out)
}

fn evidence_repair_guidance(failure: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if failure.contains("test_artifact")
        || failure.contains("bound_verify_command")
        || failure.contains("node_smoke_without_assertion")
    {
        lines.push(
            "For JavaScript/TypeScript, add an assertion-backed self-test or test artifact that the verify command actually runs, for example `node file.js` must execute `assert` checks instead of only exporting functions."
                .to_string(),
        );
    }
    if failure.contains("non_zero_test_or_assertion_evidence")
        || failure.contains("cargo_test_without_test_evidence")
    {
        lines.push(
            "For Rust, add at least one real `#[test]` or `#[cfg(test)]` module with assertions so `cargo test` verifies behavior rather than reporting zero tests."
                .to_string(),
        );
    }
    if failure.contains("interactive_ui_source_evidence")
        || failure.contains("non_static_screen_evidence")
    {
        lines.push(
            "For interactive UI work, implement state changes and input handlers in source code; a static title or unhandled instruction text is not enough."
                .to_string(),
        );
    }
    if failure.contains("challenge_or_adversary_evidence") {
        lines.push(
            "For challenge_or_adversary_evidence, edit the task implementation artifact to wire a reachable challenge/adversary entity into state evolution, not only a static label."
                .to_string(),
        );
    }
    if failure.contains("failure_or_collision_evidence") {
        lines.push(
            "For failure_or_collision_evidence, edit the task implementation artifact to wire a collision/failure conditional that transitions to a reachable failure state."
                .to_string(),
        );
    }
    if failure.contains("restart_or_recoverable_state_evidence") {
        lines.push(
            "For restart_or_recoverable_state_evidence, edit the task implementation artifact to provide a reachable terminal/failure state and a restart control (data-anvil-action=\"restart\") that resets observable state."
                .to_string(),
        );
    }
    if failure.contains("score_or_progression_evidence") {
        lines.push(
            "For score_or_progression_evidence, edit the task implementation artifact to wire score/progression updates to meaningful state transitions, not only an isolated counter."
                .to_string(),
        );
    }
    if failure.contains("stateful_update_evidence") {
        lines.push(
            "For stateful_update_evidence, edit the task implementation artifact to implement state mutations over time or in response to input, such as React state updates, reducer dispatch, timers, or animation-frame updates."
                .to_string(),
        );
    }
    if failure.contains("user_input_handler_evidence") {
        lines.push(
            "For user_input_handler_evidence, edit the task implementation artifact to wire keyboard, pointer, click, touch, or form handlers to gameplay state changes."
                .to_string(),
        );
    }
    lines
}

fn is_dependency_missing_error(command: &str, reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    let first_word = command.split_whitespace().next().unwrap_or_default();
    let executable_markers = [
        "command not found",
        "not found:",
        "no such file or directory",
        "failed to spawn command",
    ];
    if !executable_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }
    if lower.contains("assertionerror") || lower.contains("not found in") {
        return false;
    }
    first_word.is_empty() || lower.contains(&first_word.to_ascii_lowercase())
}

fn target_candidates_from_failure(command: &str, reason: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for token in command
        .split_whitespace()
        .chain(reason.split(|c: char| c.is_whitespace() || c == ':' || c == '"' || c == '\''))
    {
        let candidate = token
            .trim_matches(|c: char| matches!(c, ',' | '.' | ')' | '(' | '[' | ']'))
            .trim_start_matches("./");
        if looks_like_source_or_test_file(candidate)
            && validate_workspace_relative(candidate).is_ok()
        {
            out.insert(candidate.to_string());
        }
    }
    out
}

fn looks_like_source_or_test_file(candidate: &str) -> bool {
    let path = Path::new(candidate);
    if path.components().count() == 0 {
        return false;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("py" | "js" | "jsx" | "ts" | "tsx" | "rs" | "go" | "java")
    )
}

fn looks_like_implementation_target_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    looks_like_source_or_test_file(path)
        && !looks_like_setup_target_path(&lower)
        && !looks_like_framework_target_path(&lower)
        && !looks_like_evidence_target_path(&lower)
        && !lower.ends_with(".md")
        && !lower.ends_with(".css")
        && !lower.ends_with(".scss")
        && !lower.ends_with(".sass")
        && !lower.ends_with(".less")
        && !lower.ends_with(".d.ts")
        && !lower.ends_with("layout.tsx")
        && !lower.ends_with("layout.jsx")
}

fn looks_like_setup_target_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
    ) || lower.ends_with("/package.json")
        || lower.ends_with("/package-lock.json")
        || lower.ends_with("/pnpm-lock.yaml")
        || lower.ends_with("/yarn.lock")
        || lower.ends_with("/cargo.toml")
        || lower.ends_with("/cargo.lock")
        || lower.ends_with("/pyproject.toml")
        || lower.ends_with("/requirements.txt")
}

fn looks_like_framework_target_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("tsconfig")
        || lower.contains("next.config")
        || lower.contains("tailwind")
        || lower.contains("postcss")
        || lower.ends_with("global.d.ts")
        || lower.ends_with("globals.css")
        || lower.ends_with("layout.tsx")
        || lower.ends_with("layout.jsx")
}

fn looks_like_evidence_target_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("test")
        || lower.contains("spec")
        || lower.contains("__tests__")
        || lower.ends_with(".snap")
        || lower.contains("evidence")
        || lower.ends_with("readme.md")
}

fn assertion_excerpt(reason: &str) -> Option<String> {
    let lines = reason
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("assert")
                || lower.contains("expected")
                || lower.contains("actual")
                || lower.contains("left:")
                || lower.contains("right:")
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn validate_contract_path(root: &Path, raw: &str) -> anyhow::Result<()> {
    validate_workspace_relative(raw)?;
    let path = Path::new(raw);
    let blocked = [".anvil", ".git", "target", "node_modules", ".next", ".env"];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| blocked.contains(&part))
    }) {
        bail!("completion contract path is blocked: {raw}");
    }
    resolve_optional_existing(root, raw)
        .with_context(|| format!("completion contract path escapes workspace: {raw}"))?;
    Ok(())
}

fn normalize_contract_file_path(root: &Path, raw: &Path) -> anyhow::Result<PathBuf> {
    if raw
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".env")
    {
        bail!("completion contract file may not be .env");
    }
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let canonical = path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize completion contract {}",
            path.display()
        )
    })?;
    let mut allowed_roots = vec![root.canonicalize()?];
    for candidate in [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
    ] {
        if let Ok(temp) = candidate.canonicalize()
            && !allowed_roots.contains(&temp)
        {
            allowed_roots.push(temp);
        }
    }
    if allowed_roots
        .iter()
        .any(|allowed| canonical.starts_with(allowed))
    {
        Ok(canonical)
    } else {
        bail!(
            "completion contract file must be under workspace or temp directory: {}",
            canonical.display()
        );
    }
}

fn profile_is_generic(profile: &str) -> bool {
    matches!(profile, "" | "generic" | "default" | "none")
}

fn classify_python_test_discovery_failure(
    root: &Path,
    command: &str,
    output: &str,
) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("no tests ran") || lower.contains("ran 0 tests") {
        if pytest_style_under_unittest(root, command) {
            return Some("test_framework_mismatch:pytest_style_under_unittest".to_string());
        }
        Some("test_discovery_failure:no_tests_ran".to_string())
    } else {
        None
    }
}

fn pytest_style_under_unittest(root: &Path, command: &str) -> bool {
    let Some(path) = unittest_target_path(command) else {
        return false;
    };
    let target = root.join(path);
    let Ok(content) = std::fs::read_to_string(target) else {
        return false;
    };
    content.contains("def test_") && !content.contains("unittest.TestCase")
}

fn unittest_target_path(command: &str) -> Option<&str> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let module_index = parts
        .windows(2)
        .position(|window| window[0] == "-m" && window[1] == "unittest")?;
    parts
        .iter()
        .skip(module_index + 2)
        .copied()
        .find(|part| !part.starts_with('-') && part.ends_with(".py"))
}

fn default_verify_repair_cap() -> usize {
    2
}

fn default_deferred_status() -> String {
    "pending".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn structured_contract_deduplicates_and_accepts_safe_verify() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: vec!["src/main.rs".to_string(), "src/main.rs".to_string()],
            verify_commands: vec!["cargo test".to_string(), "cargo test".to_string()],
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 0,
        }
        .validate(dir.path())
        .unwrap();
        assert_eq!(contract.required_paths, vec!["src/main.rs"]);
        assert_eq!(contract.verify_commands, vec!["cargo test"]);
        assert_eq!(contract.verify_repair_cap, 2);
    }

    #[test]
    fn structured_contract_deduplicates_capabilities_and_derives_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: Vec::new(),
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: vec![
                " deterministic_test ".to_string(),
                "deterministic_test".to_string(),
            ],
            deterministic_oracles: vec![" source_semantic ".to_string(), "".to_string()],
            required_evidence: vec!["custom_evidence".to_string()],
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        assert_eq!(contract.required_capabilities, vec!["deterministic_test"]);
        assert_eq!(contract.deterministic_oracles, vec!["source_semantic"]);
        assert!(
            contract
                .required_evidence
                .contains(&"custom_evidence".to_string())
        );
        assert!(
            contract
                .required_evidence
                .contains(&"test_artifact".to_string())
        );
        assert!(
            contract
                .required_evidence
                .contains(&"bound_verify_command".to_string())
        );
    }

    #[test]
    fn contract_loads_old_json_without_evidence_hint_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let contract: CompletionContract = serde_json::from_str(
            r#"{"required_paths":["src/main.rs"],"required_evidence":["challenge_or_adversary_evidence"]}"#,
        )
        .unwrap();
        let contract = contract.validate(dir.path()).unwrap();
        assert!(contract.evidence_hint_tokens.is_empty());
        assert_eq!(
            contract.required_evidence,
            vec!["challenge_or_adversary_evidence"]
        );
    }

    #[test]
    fn contract_derives_goal_evidence_hint_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: Vec::new(),
            verify_commands: Vec::new(),
            profile: None,
            goal: Some("シューティングでドラゴンを倒すゲーム".to_string()),
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        assert!(
            contract
                .evidence_hint_tokens
                .contains(&"ドラゴン".to_string())
        );
        assert!(
            evidence_hint_tokens_for_goal("スペースインベーダー")
                .contains(&"インベーダー".to_string())
        );
    }

    #[test]
    fn structured_contract_normalizes_required_obligations() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: Vec::new(),
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: vec![
                " implementation ".to_string(),
                "acceptance-evidence".to_string(),
                "implementation".to_string(),
            ],
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        assert_eq!(
            contract.required_obligations,
            vec!["implementation", "acceptance_evidence"]
        );
        let err = CompletionContract {
            required_paths: Vec::new(),
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: vec!["reporting".to_string()],
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap_err()
        .to_string();
        assert!(err.contains("unsupported completion obligation role"));
    }

    #[test]
    fn contract_rejects_escape_and_secret_paths() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "../x",
            "/tmp/x",
            ".env",
            ".anvil/session.json",
            "target/debug/app",
        ] {
            let err = CompletionContract {
                required_paths: vec![path.to_string()],
                verify_commands: Vec::new(),
                profile: None,
                goal: None,
                required_capabilities: Vec::new(),
                deterministic_oracles: Vec::new(),
                required_evidence: Vec::new(),
                evidence_hint_tokens: Vec::new(),
                required_obligations: Vec::new(),
                deferred_verify_requirements: Vec::new(),
                verify_repair_cap: 2,
            }
            .validate(dir.path())
            .unwrap_err()
            .to_string();
            assert!(!err.is_empty(), "{path}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn contract_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
        let err = CompletionContract {
            required_paths: vec!["out/file.txt".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap_err()
        .to_string();
        assert!(err.contains("escapes workspace") || err.contains("path"));
    }

    #[test]
    fn contract_rejects_setup_and_shell_control_verify_commands() {
        let dir = tempfile::tempdir().unwrap();
        for command in [
            "npm install",
            "npm test && npm run build",
            "next dev -p 3011",
        ] {
            let err = CompletionContract {
                required_paths: Vec::new(),
                verify_commands: vec![command.to_string()],
                profile: None,
                goal: None,
                required_capabilities: Vec::new(),
                deterministic_oracles: Vec::new(),
                required_evidence: Vec::new(),
                evidence_hint_tokens: Vec::new(),
                required_obligations: Vec::new(),
                deferred_verify_requirements: Vec::new(),
                verify_repair_cap: 2,
            }
            .validate(dir.path())
            .unwrap_err()
            .to_string();
            assert!(!err.is_empty(), "{command}");
        }
    }

    #[test]
    fn contract_file_path_rejects_env_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=x").unwrap();
        let err = normalize_contract_file_path(dir.path(), Path::new(".env"))
            .unwrap_err()
            .to_string();
        assert!(err.contains(".env"));
    }

    #[test]
    fn python_no_tests_ran_is_test_discovery_failure() {
        assert_eq!(
            classify_python_test_discovery_failure(
                tempfile::tempdir().unwrap().path(),
                "python3 -m unittest",
                "Ran 0 tests in 0.000s\n\nOK"
            )
            .as_deref(),
            Some("test_discovery_failure:no_tests_ran")
        );
        assert_eq!(
            classify_python_test_discovery_failure(
                tempfile::tempdir().unwrap().path(),
                "python3 -m unittest",
                "NO TESTS RAN"
            )
            .as_deref(),
            Some("test_discovery_failure:no_tests_ran")
        );
    }

    #[test]
    fn assertion_text_not_found_is_not_dependency_missing() {
        let reason = "command failed: python3 -m unittest test_markdown_lint.py\n\
            AssertionError: '2: Heading level jumps from 1 to 3' not found in '1: Invalid heading format'";
        assert!(!is_dependency_missing_error(
            "python3 -m unittest test_markdown_lint.py",
            reason
        ));
        assert!(is_dependency_missing_error(
            "python3 -m unittest test_markdown_lint.py",
            "sh: python3: command not found"
        ));
    }

    #[test]
    fn verify_feedback_includes_command_target_and_assertion_excerpt() {
        let mut report = VerificationReport::pass();
        report.push_command_failure(
            "python3 -m unittest test_markdown_lint.py",
            "Traceback\n  File \"test_markdown_lint.py\", line 10\nAssertionError: 2 != 3\nExpected three headings\nActual two headings",
        );
        let feedback = format_verify_feedback(&report);
        assert!(feedback.contains("Command failed: `python3 -m unittest test_markdown_lint.py`"));
        assert!(feedback.contains("Likely target files: test_markdown_lint.py"));
        assert!(feedback.contains("Failure excerpt:"));
        assert!(feedback.contains("AssertionError"));
        assert!(feedback.contains("Repair guidance:"));
    }

    #[test]
    fn verify_feedback_does_not_suggest_test_weakening() {
        let mut report = VerificationReport::pass();
        report.push_command_failure("python3 -m unittest test_a.py", "AssertionError: bad");
        let feedback = format_verify_feedback(&report).to_ascii_lowercase();
        for forbidden in [
            "set the assertion to true",
            "delete the assertion",
            "skip the test",
            "ignore the failure",
        ] {
            assert!(!feedback.contains(forbidden), "{forbidden}: {feedback}");
        }
    }

    #[test]
    fn verify_feedback_includes_nextjs_profile_repairs() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "CSS side-effect imports require a declaration file such as src/app/global.d.ts with declare module \"*.css\"",
        );
        report.push_profile_failure(
            "src/app/page.tsx uses browser/client APIs and must start with \"use client\"",
        );
        let feedback = format_verify_feedback(&report);
        assert!(feedback.contains("declare module \"*.css\";"));
        assert!(feedback.contains("first non-empty statement"));
    }

    #[test]
    fn verify_feedback_anchors_gameplay_evidence_to_implementation_files() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: vec![
                "package.json".to_string(),
                "src/app/page.tsx".to_string(),
                "src/app/layout.tsx".to_string(),
                "src/app/global.d.ts".to_string(),
                "tests/gameplay.test.ts".to_string(),
            ],
            verify_commands: Vec::new(),
            profile: Some("nextjs".to_string()),
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "missing_required_evidence:challenge_or_adversary_evidence,failure_or_collision_evidence,restart_or_recoverable_state_evidence,score_or_progression_evidence,stateful_update_evidence,user_input_handler_evidence",
        );
        let feedback = format_verify_feedback_with_contract(&report, Some(&contract));
        assert!(feedback.contains("Target implementation files: src/app/page.tsx"));
        assert!(feedback.contains("reachable challenge/adversary entity"));
        assert!(feedback.contains("not only a static label"));
        assert!(feedback.contains("collision/failure conditional"));
        assert!(feedback.contains("reachable failure state"));
        assert!(feedback.contains("data-anvil-action=\"restart\""));
        assert!(feedback.contains("resets observable state"));
        assert!(feedback.contains("score/progression updates"));
        assert!(feedback.contains("state mutations over time or in response to input"));
        assert!(feedback.contains("wire keyboard, pointer, click, touch, or form handlers"));
    }

    #[test]
    fn unittest_zero_tests_is_not_verify_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test_repair_report.py"),
            "def test_free():\n    pass\n",
        )
        .unwrap();
        let report = CompletionContract {
            required_paths: vec!["test_repair_report.py".to_string()],
            verify_commands: vec!["python3 -m unittest test_repair_report.py".to_string()],
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap()
        .verify(dir.path());
        assert!(!report.is_pass());
        assert!(
            report
                .primary_reason()
                .contains("test_framework_mismatch:pytest_style_under_unittest")
        );
    }

    #[test]
    fn unittest_zero_tests_classifies_pytest_style_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test_free.py"),
            "def test_free():\n    pass\n",
        )
        .unwrap();
        assert_eq!(
            classify_python_test_discovery_failure(
                dir.path(),
                "python3 -m unittest test_free.py",
                "Ran 0 tests in 0.000s"
            )
            .as_deref(),
            Some("test_framework_mismatch:pytest_style_under_unittest")
        );
    }

    #[test]
    fn completion_contract_pending_deferred_requirement_blocks_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let contract = CompletionContract {
            required_paths: vec!["package.json".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: vec![DeferredVerifyRequirement {
                command: "npm run build".to_string(),
                reason: "requires dependency setup".to_string(),
                authority: "postcheck".to_string(),
                profile: Some("nextjs".to_string()),
                status: "blocked_by_dependency_setup".to_string(),
            }],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let report = contract.verify(dir.path());
        assert!(!report.is_pass());
        assert!(
            report.primary_reason().contains("dependency_setup_missing"),
            "{report:?}"
        );
    }

    #[test]
    fn manifest_only_nextjs_build_verify_is_not_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        let contract = CompletionContract {
            required_paths: vec!["package.json".to_string()],
            verify_commands: vec!["npm run build".to_string()],
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let (report, lifecycles) = contract.verify_with_goal_observed(dir.path(), "");
        assert!(!report.is_pass(), "{report:?}");
        assert!(report.primary_reason().contains("dependency_setup_missing"));
        assert_eq!(lifecycles.len(), 1);
        assert!(lifecycles[0].lifecycle_stages().contains(&"setup_blocked"));
    }

    #[test]
    fn contract_deferred_build_with_plan_setup_authority_installs_then_passes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm.sh");
        write_executable(
            &fake_npm,
            "#!/bin/sh\nmkdir -p node_modules/.bin node_modules/next\ncat > node_modules/next/package.json <<'EOF'\n{\"version\":\"14.2.0\"}\nEOF\ncat > node_modules/.bin/next <<'EOF'\n#!/bin/sh\nexit 0\nEOF\ncat > node_modules/.bin/npm <<'EOF'\n#!/bin/sh\nif [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then exit 0; fi\nexit 1\nEOF\nchmod +x node_modules/.bin/next node_modules/.bin/npm\ntouch package-lock.json\nexit 0\n",
        );
        let contract = CompletionContract {
            required_paths: vec!["package.json".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: vec![DeferredVerifyRequirement {
                command: "npm run build".to_string(),
                reason: "requires dependency setup".to_string(),
                authority: "postcheck".to_string(),
                profile: Some("nextjs".to_string()),
                status: "blocked_by_dependency_setup".to_string(),
            }],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();

        let (report, lifecycles) = contract
            .verify_with_goal_observed_with_setup_program_and_authority(
                dir.path(),
                "",
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
    fn contract_deferred_build_without_authority_stays_dependency_missing_without_spawn() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        )
        .unwrap();
        let contract = CompletionContract {
            required_paths: vec!["package.json".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: vec![DeferredVerifyRequirement {
                command: "npm run build".to_string(),
                reason: "requires dependency setup".to_string(),
                authority: "postcheck".to_string(),
                profile: Some("nextjs".to_string()),
                status: "blocked_by_dependency_setup".to_string(),
            }],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();

        let (report, lifecycles) = contract
            .verify_with_goal_observed_with_setup_program_and_authority(
                dir.path(),
                "",
                NodeDependencySetupAuthority::None,
                Path::new("missing-fake-npm"),
                false,
            );

        assert!(!report.is_pass(), "{report:?}");
        assert!(matches!(
            report.status,
            crate::planner::verify::VerifyStatus::DependencyMissing(_)
        ));
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].setup_status(), "blocked");
        assert!(
            !lifecycles[0]
                .setup
                .as_ref()
                .is_some_and(|setup| setup.attempted)
        );
        assert!(!dir.path().join("node_modules").exists());
        assert!(!dir.path().join("package-lock.json").exists());
    }

    #[test]
    fn completion_contract_deferred_nextjs_build_requires_dependency_setup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"15.0.0","react":"19.0.0","react-dom":"19.0.0"},"devDependencies":{"typescript":"5.5.0","@types/node":"20.0.0","@types/react":"19.0.0","@types/react-dom":"19.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"bundler"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return <main/>;}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function RootLayout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        let contract = CompletionContract {
            required_paths: vec![
                "package.json".to_string(),
                "src/app/page.tsx".to_string(),
                "src/app/layout.tsx".to_string(),
                "src/app/global.d.ts".to_string(),
            ],
            verify_commands: Vec::new(),
            profile: Some("nextjs".to_string()),
            goal: Some("Create a Next.js app".to_string()),
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: vec![DeferredVerifyRequirement {
                command: "npm run build".to_string(),
                reason: "requires dependency setup".to_string(),
                authority: "postcheck".to_string(),
                profile: Some("nextjs".to_string()),
                status: "blocked_by_dependency_setup".to_string(),
            }],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let report = contract.verify(dir.path());
        assert!(!report.is_pass(), "{report:?}");
        assert!(
            report.primary_reason().contains("node_modules/.bin/next"),
            "{report:?}"
        );
        assert_eq!(
            contract.deferred_status_summary(dir.path(), "Create a Next.js app"),
            vec!["npm run build:dependency_setup_missing"]
        );
    }

    #[test]
    fn completion_contract_deferred_nextjs_build_does_not_use_static_tsconfig_coverage() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"15.0.0","react":"19.0.0","react-dom":"19.0.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"20.0.0","@types/react":"19.0.0","@types/react-dom":"19.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return <main/>;}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function RootLayout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        let contract = CompletionContract {
            required_paths: vec!["package.json".to_string()],
            verify_commands: Vec::new(),
            profile: Some("nextjs".to_string()),
            goal: Some("Create a Next.js app".to_string()),
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: vec![DeferredVerifyRequirement {
                command: "npm run build".to_string(),
                reason: "requires dependency setup".to_string(),
                authority: "postcheck".to_string(),
                profile: Some("nextjs".to_string()),
                status: "blocked_by_dependency_setup".to_string(),
            }],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let report = contract.verify(dir.path());
        assert!(!report.is_pass());
        assert!(
            report.primary_reason().contains("node_modules/.bin/next"),
            "{report:?}"
        );
    }
}
