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
use crate::minimal_loop::import_scan::{
    ImportedDefinitionExcerpt, imported_symbol_definition_excerpt,
};
use crate::minimal_loop::repair_target::{RepairTarget, classify_repair_target};
use crate::planner::verify::{
    NormalizedVerifyCommand, VerificationReport, normalize_planner_verify_command,
    normalize_verify_command, validate_verify_command,
};
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
            for normalized in normalize_planner_verify_command(&command)? {
                validate_verify_command(&normalized)?;
                if seen_commands.insert(normalized.clone()) {
                    commands.push(normalized);
                }
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
            let normalized = normalize_planner_verify_command(&requirement.command)?;
            if normalized.len() != 1 {
                bail!(
                    "deferred verify requirement must be one deterministic command after normalization"
                );
            }
            requirement.command = normalized.into_iter().next().unwrap_or_default();
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
        for command in normalized_contract_verify_commands(&self.verify_commands, &mut report) {
            if let Some(build_requirement) = build_verifier::requirement_from_deferred(
                &command,
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
            let normalized_command: NormalizedVerifyCommand =
                match normalize_verify_command(&command) {
                    Ok(command) => command,
                    Err(err) => {
                        report.push_command_failure(command.clone(), err.to_string());
                        continue;
                    }
                };
            match crate::minimal_loop::verifier_env::run_checked(&normalized_command, root, offline)
            {
                Ok(output) => {
                    if command.contains("npm") && output.contains("0 tests") {
                        report.push_command_failure(command.clone(), "Node 0 tests rejected");
                    } else if let Some(reason) =
                        classify_python_test_discovery_failure(root, &command, &output)
                    {
                        report.push_command_failure(command.clone(), reason);
                    }
                }
                Err(err) if is_dependency_missing_error(&command, &err.to_string()) => {
                    report.push_dependency_missing(command.clone());
                }
                Err(err) => {
                    let reason = err.to_string();
                    if let Some(reason) =
                        classify_python_test_discovery_failure(root, &command, &reason)
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

fn normalized_contract_verify_commands(
    commands: &[String],
    report: &mut VerificationReport,
) -> Vec<String> {
    let mut out = Vec::new();
    for command in commands {
        match normalize_planner_verify_command(command) {
            Ok(normalized) => out.extend(normalized),
            Err(err) => report.push_command_failure(command.clone(), err.to_string()),
        }
    }
    out
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
            || failure.contains("unverified runtime evidence")
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
    compile_error_repair_guidance_with_root(None, errors)
}

pub(crate) fn compile_error_repair_guidance_with_root(
    root: Option<&Path>,
    errors: &[CompileError],
) -> Vec<String> {
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
            if let Some(suggestion) = compiler_suggestion(&error.message) {
                lines.push(format!("Compiler suggestion: {suggestion}"));
            }
            if let Some(guidance) = nullability_narrowing_repair_guidance(error) {
                lines.push(guidance);
            }
            if let Some(guidance) = call_arity_repair_guidance(root, error) {
                lines.extend(guidance);
            }
            if let Some(guidance) = const_reassignment_repair_guidance(root, error) {
                lines.extend(guidance);
            }
            if let Some(guidance) = duplicate_binding_repair_guidance(error) {
                lines.push(guidance);
            }
            if let Some(guidance) = postcss_plugins_key_repair_guidance(error) {
                lines.push(guidance);
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
            if let Some(root) = root {
                lines.extend(type_script_cross_file_definition_guidance(root, error));
            }
            lines
        })
        .collect()
}

fn nullability_narrowing_repair_guidance(error: &CompileError) -> Option<String> {
    if !typescript_nullability_message(&error.message) {
        return None;
    }
    let variable = extract_first_quoted_symbol(&error.message)?;
    Some(format!(
        "TypeScript nullability repair for `{variable}`: inside the closure at line {}, add `if (!{variable}) return;` before first use, or capture a non-null local after the outer check.",
        error.line
    ))
}

fn typescript_nullability_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("is possibly 'null'")
        || lower.contains("is possibly \"null\"")
        || lower.contains("is possibly `null`")
        || lower.contains("is possibly 'undefined'")
        || lower.contains("is possibly \"undefined\"")
        || lower.contains("is possibly `undefined`")
}

fn call_arity_repair_guidance(root: Option<&Path>, error: &CompileError) -> Option<Vec<String>> {
    let (expected, got) = typescript_call_arity_message(&error.message)?;
    let root = root?;
    let content = std::fs::read_to_string(root.join(&error.path)).ok()?;
    let line = source_line(root, &error.path, error.line)
        .or_else(|| compile_excerpt_source_line(&error.excerpt))?;
    let callee = call_callee_for_error_column(&line, error.column)?;
    let signature = local_function_signature(&content, &callee)?;
    Some(vec![
        format!(
            "TypeScript call-arity repair for `{callee}`: Expected {expected} arguments, but got {got}."
        ),
        format!("Actual same-file signature for `{callee}`: {signature}"),
        format!(
            "TypeScript call-arity remedy menu for `{callee}`: remove the extra argument, or extend the signature -- keep call sites consistent."
        ),
    ])
}

fn const_reassignment_repair_guidance(
    root: Option<&Path>,
    error: &CompileError,
) -> Option<Vec<String>> {
    if !typescript_const_reassignment_message(&error.message) {
        return None;
    }
    let symbol = error
        .symbol
        .clone()
        .or_else(|| extract_first_quoted_symbol(&error.message))?;
    let symbol = symbol.as_str();
    let root = root?;
    let content = std::fs::read_to_string(root.join(&error.path)).ok()?;
    let (declaration_line, declaration_excerpt) =
        const_reassignment_declaration_site(&content, symbol)?;
    Some(vec![
        format!(
            "TypeScript const-reassignment repair for `{symbol}`: {}",
            error.summary()
        ),
        format!(
            "Declaration site for `{symbol}` in {}:{}: {}",
            error.path, declaration_line, declaration_excerpt
        ),
        format!(
            "TypeScript const-reassignment remedy menu for `{symbol}`: declare with let, or lift into state if it changes per frame -- keep declaration and all assignments consistent."
        ),
    ])
}

fn typescript_const_reassignment_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("cannot assign to")
        || lower.contains("read only property")
        || lower.contains("reassign")
        || lower.contains("constant")
}

fn const_reassignment_declaration_site(content: &str, symbol: &str) -> Option<(usize, String)> {
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if const_reassignment_declares_symbol(trimmed, symbol) {
            return Some((index + 1, trimmed.to_string()));
        }
    }
    None
}

fn const_reassignment_declares_symbol(line: &str, symbol: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with("const ") {
        return false;
    }
    if line.contains(&format!("const {symbol} "))
        || line.contains(&format!("const {symbol}="))
        || line.contains(&format!("const {symbol},"))
        || line.contains(&format!("const {symbol})"))
        || line.contains(&format!("const {symbol}]"))
    {
        return true;
    }
    line.starts_with("const [") && line.contains(symbol)
        || line.starts_with("const {") && line.contains(symbol)
}

fn typescript_call_arity_message(message: &str) -> Option<(usize, usize)> {
    let after_expected = message.split_once("Expected ")?.1;
    let (expected, rest) = after_expected.split_once(" arguments")?;
    let after_got = rest.split_once("but got ")?.1;
    let got = after_got
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    Some((expected.trim().parse().ok()?, got.parse().ok()?))
}

fn duplicate_binding_repair_guidance(error: &CompileError) -> Option<String> {
    let symbol = typescript_duplicate_binding_message(&error.message)?;
    let mut line_numbers = compile_excerpt_source_line_numbers(&error.excerpt);
    if error.line > 0 && !line_numbers.contains(&error.line) {
        line_numbers.push(error.line);
    }
    line_numbers.sort_unstable();
    line_numbers.dedup();
    let earlier = *line_numbers.first()?;
    let later = *line_numbers.last()?;
    if earlier == later {
        return Some(format!(
            "TypeScript duplicate-binding repair for `{symbol}`: line {later} redeclares `{symbol}` in the same block; remove or rename the redeclaration so only one binding remains in scope."
        ));
    }
    Some(format!(
        "TypeScript duplicate-binding repair for `{symbol}`: lines {earlier} and {later} both declare `{symbol}`; remove or rename the later redeclaration (line {later}); the earlier binding (line {earlier}) is already in scope in this block."
    ))
}

fn typescript_duplicate_binding_message(message: &str) -> Option<String> {
    message
        .to_ascii_lowercase()
        .contains("defined multiple times")
        .then(|| extract_first_quoted_symbol(message))
        .flatten()
}

fn postcss_plugins_key_repair_guidance(error: &CompileError) -> Option<String> {
    let lower = error.message.to_ascii_lowercase();
    if !(lower.contains("postcss")
        && lower.contains("plugins")
        && (lower.contains("must export") || lower.contains("export a")))
    {
        return None;
    }
    Some(
        "PostCSS config-format remedy: make postcss.config.js export a plugins key using CommonJS, e.g. module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } }; or make the package coherently ESM with .mjs/package.json type module."
            .to_string(),
    )
}

fn compile_excerpt_source_line_numbers(excerpt: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for line in excerpt.lines() {
        let Some((left, _)) = line.split_once('|') else {
            continue;
        };
        let Some(raw_line) = left.split_whitespace().find(|part| {
            part.chars()
                .all(|ch| ch.is_ascii_digit() || ch == '>' || ch == ':')
                && part.chars().any(|ch| ch.is_ascii_digit())
        }) else {
            continue;
        };
        let digits = raw_line
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if let Ok(line_number) = digits.parse::<usize>()
            && !out.contains(&line_number)
        {
            out.push(line_number);
        }
    }
    out
}

fn call_callee_for_error_column(line: &str, column: usize) -> Option<String> {
    let column_index = column.saturating_sub(1);
    let mut best: Option<(usize, String)> = None;
    for (open_index, _) in line.match_indices('(') {
        let prefix = &line[..open_index];
        let callee = prefix
            .chars()
            .rev()
            .skip_while(|ch| ch.is_ascii_whitespace())
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$' || *ch == '.')
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let callee = callee.rsplit('.').next().unwrap_or_default().trim();
        if !is_identifier(callee) {
            continue;
        }
        if open_index <= column_index {
            best = Some((open_index, callee.to_string()));
        } else if best.is_none() {
            best = Some((open_index, callee.to_string()));
            break;
        }
    }
    best.map(|(_, callee)| callee)
}

fn local_function_signature(content: &str, callee: &str) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if local_function_definition_starts(trimmed, callee)
            || local_arrow_function_definition_starts(trimmed, callee)
        {
            return Some(clean_signature_lines(&lines[index..]));
        }
    }
    None
}

fn local_function_definition_starts(line: &str, callee: &str) -> bool {
    [
        format!("function {callee}"),
        format!("async function {callee}"),
        format!("export function {callee}"),
        format!("export async function {callee}"),
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn local_arrow_function_definition_starts(line: &str, callee: &str) -> bool {
    [
        format!("const {callee} ="),
        format!("let {callee} ="),
        format!("var {callee} ="),
        format!("export const {callee} ="),
        format!("export let {callee} ="),
        format!("export var {callee} ="),
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn clean_signature_lines(lines: &[&str]) -> String {
    let mut out = Vec::new();
    for line in lines.iter().take(8) {
        let mut trimmed = line.trim().to_string();
        if let Some((head, _)) = trimmed.split_once('{') {
            trimmed = format!("{} {{", head.trim_end());
            out.push(trimmed);
            break;
        }
        if let Some((head, _)) = trimmed.split_once("=>") {
            trimmed = format!("{} =>", head.trim_end());
            out.push(trimmed);
            break;
        }
        out.push(trimmed);
        if out.last().is_some_and(|line| line.ends_with(';')) {
            break;
        }
    }
    out.join("\n")
}

fn type_script_cross_file_definition_guidance(root: &Path, error: &CompileError) -> Vec<String> {
    let mut contexts = Vec::new();
    if let Some(symbol) = error.symbol.as_deref()
        && let Some(context) = imported_symbol_definition_excerpt(root, &error.path, symbol)
    {
        contexts.push(context);
    }
    if let Some(suggestion) = compiler_suggested_symbol(&error.message)
        && let Some(imported_call) = local_receiver_imported_call(root, &error.path, &suggestion)
        && let Some(context) = imported_symbol_definition_excerpt(root, &error.path, &imported_call)
    {
        contexts.push(context);
    }
    if let Some(property) = property_missing_name(&error.message)
        && let Some(receiver) = property_receiver_identifier(root, error, &property)
    {
        if let Some(imported_call) = local_receiver_imported_call(root, &error.path, &receiver)
            && let Some(context) =
                imported_symbol_definition_excerpt(root, &error.path, &imported_call)
        {
            contexts.push(context);
        } else if let Some(context) =
            imported_symbol_definition_excerpt(root, &error.path, &receiver)
        {
            contexts.push(context);
        }
    }
    if let Some(property) = property_missing_name(&error.message)
        && let Some(imported_call) =
            destructured_property_imported_call(root, &error.path, &property)
        && let Some(context) = imported_symbol_definition_excerpt(root, &error.path, &imported_call)
    {
        contexts.push(context);
    }
    let missing_property = property_missing_name(&error.message);
    let missing_type = property_missing_type_name(&error.message);
    if let Some(type_name) = missing_type.as_deref()
        && let Some(context) = imported_symbol_definition_excerpt(root, &error.path, type_name)
    {
        contexts.push(context);
    }
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    for context in contexts {
        let key = format!("{}:{}", context.definition_path, context.imported_name);
        if !seen.insert(key) {
            continue;
        }
        lines.extend(render_imported_definition_context(&context));
        if let Some(property) = missing_property.as_deref() {
            lines.push(missing_property_repair_menu(
                property,
                missing_type.as_deref(),
                &context,
            ));
        }
    }
    lines
}

fn render_imported_definition_context(context: &ImportedDefinitionExcerpt) -> Vec<String> {
    vec![
        format!(
            "Imported definition context for `{}` from {}:",
            context.local_name, context.definition_path
        ),
        context.excerpt.clone(),
        "TypeScript member repair menu: use an exported member, export the missing one, or remove the call."
            .to_string(),
    ]
}

fn compiler_suggestion(message: &str) -> Option<String> {
    let start = message.find("Did you mean")?;
    Some(message[start..].trim().to_string())
}

fn compiler_suggested_symbol(message: &str) -> Option<String> {
    let suggestion = compiler_suggestion(message)?;
    extract_first_quoted_symbol(&suggestion)
}

fn property_missing_name(message: &str) -> Option<String> {
    message
        .contains("Property ")
        .then(|| extract_first_quoted_symbol(message))
        .flatten()
}

fn property_missing_type_name(message: &str) -> Option<String> {
    let (_, rest) = message.split_once(" on type ")?;
    let raw = extract_first_quoted_symbol(rest)?;
    is_identifier(&raw).then_some(raw)
}

fn missing_property_repair_menu(
    property: &str,
    type_name: Option<&str>,
    context: &ImportedDefinitionExcerpt,
) -> String {
    let type_label = type_name.unwrap_or(context.local_name.as_str());
    let existing_member_hint = if context.excerpt.contains("getState") {
        "call an existing member (e.g. poll getState() from the rAF loop)"
    } else {
        "call an existing member"
    };
    format!(
        "TypeScript member repair menu: {existing_member_hint}, or add {property} to {type_label}'s definition -- keep both files consistent."
    )
}

fn extract_first_quoted_symbol(message: &str) -> Option<String> {
    for quote in ['\'', '"', '`'] {
        let Some(start) = message.find(quote) else {
            continue;
        };
        let rest = &message[start + quote.len_utf8()..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        let symbol = rest[..end].trim();
        if !symbol.is_empty() {
            return Some(symbol.to_string());
        }
    }
    None
}

fn local_receiver_imported_call(root: &Path, source: &str, receiver: &str) -> Option<String> {
    let content = std::fs::read_to_string(root.join(source)).ok()?;
    let assignment = find_local_assignment_expression(&content, receiver)?;
    let callee = leading_call_identifier(&assignment)?;
    imported_symbol_definition_excerpt(root, source, &callee).map(|_| callee)
}

fn destructured_property_imported_call(
    root: &Path,
    source: &str,
    property: &str,
) -> Option<String> {
    let content = std::fs::read_to_string(root.join(source)).ok()?;
    let assignment = find_object_destructure_assignment_expression(&content, property)?;
    let callee = leading_call_identifier(&assignment)?;
    imported_symbol_definition_excerpt(root, source, &callee).map(|_| callee)
}

fn find_local_assignment_expression(content: &str, receiver: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        for prefix in [
            format!("const {receiver} ="),
            format!("let {receiver} ="),
            format!("var {receiver} ="),
        ] {
            if let Some(rest) = trimmed.strip_prefix(&prefix) {
                return Some(rest.trim().trim_end_matches(';').to_string());
            }
        }
    }
    None
}

fn find_object_destructure_assignment_expression(content: &str, property: &str) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    for (start, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !["const", "let", "var"]
            .iter()
            .any(|keyword| trimmed.starts_with(&format!("{keyword} {{")))
        {
            continue;
        }
        let mut block = String::new();
        for line in lines.iter().skip(start).take(80) {
            let trimmed = line.trim();
            if !block.is_empty() {
                block.push('\n');
            }
            block.push_str(trimmed);
            if let Some(assignment) = destructure_assignment_after_close(trimmed)
                && destructuring_contains_property(&block, property)
            {
                return Some(assignment);
            }
            if trimmed.ends_with(';') && !block.contains("} =") && !block.contains("}=") {
                break;
            }
        }
    }
    None
}

fn destructure_assignment_after_close(line: &str) -> Option<String> {
    line.split_once("} =")
        .map(|(_, rest)| rest)
        .or_else(|| line.split_once("}=").map(|(_, rest)| rest))
        .map(|rest| rest.trim().trim_end_matches(';').to_string())
        .filter(|rest| !rest.is_empty())
}

fn destructuring_contains_property(block: &str, property: &str) -> bool {
    let Some((_, after_open)) = block.split_once('{') else {
        return false;
    };
    let Some((inside, _)) = after_open.rsplit_once('}') else {
        return false;
    };
    inside
        .split([',', '\n'])
        .map(|part| {
            part.trim()
                .trim_start_matches("...")
                .split_once(':')
                .map(|(name, _)| name)
                .unwrap_or_else(|| part.split_once('=').map(|(name, _)| name).unwrap_or(part))
                .trim()
        })
        .any(|name| name == property)
}

fn leading_call_identifier(expression: &str) -> Option<String> {
    let trimmed = expression.trim();
    let open = trimmed.find('(')?;
    let ident = trimmed[..open]
        .trim()
        .trim_start_matches("await ")
        .trim_start_matches("new ")
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim();
    is_identifier(ident).then(|| ident.to_string())
}

fn property_receiver_identifier(
    root: &Path,
    error: &CompileError,
    property: &str,
) -> Option<String> {
    let line = source_line(root, &error.path, error.line)
        .or_else(|| compile_excerpt_source_line(&error.excerpt))?;
    receiver_before_property(&line, property)
}

fn source_line(root: &Path, path: &str, line: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    std::fs::read_to_string(root.join(path))
        .ok()?
        .lines()
        .nth(line.saturating_sub(1))
        .map(str::to_string)
}

fn compile_excerpt_source_line(excerpt: &str) -> Option<String> {
    excerpt.lines().find_map(|line| {
        line.split_once('|')
            .map(|(_, rest)| rest.trim().to_string())
            .filter(|rest| !rest.is_empty() && !rest.starts_with('^'))
    })
}

fn receiver_before_property(line: &str, property: &str) -> Option<String> {
    let needle = format!(".{property}");
    let index = line.find(&needle)?;
    let before = &line[..index];
    let receiver = before
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    is_identifier(&receiver).then_some(receiver)
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
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
    compile_repair_prompt_section_with_root(None, errors, protection)
}

pub(crate) fn compile_repair_prompt_section_with_root(
    root: Option<&Path>,
    errors: &[CompileError],
    protection: CompileRepairPromptProtection,
) -> String {
    if errors.is_empty() {
        return "- none".to_string();
    }
    let mut lines = compile_error_repair_guidance_with_root(root, errors);
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
    if failure.contains("restart_or_recoverable_state_evidence")
        && failure.contains("terminal_state_not_reached")
    {
        lines.push(
            "For restart_or_recoverable_state_evidence partial verification, either expose an in-play restart control, or accept the partial classification (the restart exists but cannot be behaviorally verified by the generic probe)."
                .to_string(),
        );
    } else if failure.contains("restart_or_recoverable_state_evidence") {
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
    fn compile_repair_prompt_includes_call_arity_signature_and_remedy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "type Sprite = { x: number; y: number };\n\
function renderSprite(ctx: CanvasRenderingContext2D, sprite: Sprite, scale: number) {\n\
  ctx.fillRect(sprite.x, sprite.y, scale, scale);\n\
}\n\
export default function Page() {\n\
  renderSprite(document.createElement('canvas').getContext('2d')!, { x: 1, y: 2 }, 12, 'debug');\n\
  return <main />;\n\
}\n",
        )
        .unwrap();
        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 6,
                column: 3,
                message: "Type error: Expected 3 arguments, but got 4.".to_string(),
                excerpt: "6 |   renderSprite(document.createElement('canvas').getContext('2d')!, { x: 1, y: 2 }, 12, 'debug');\n  |   ^".to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "TypeScript call-arity repair for `renderSprite`: Expected 3 arguments, but got 4."
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "Actual same-file signature for `renderSprite`: function renderSprite(ctx: CanvasRenderingContext2D, sprite: Sprite, scale: number) {"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "remove the extra argument, or extend the signature -- keep call sites consistent"
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_duplicate_binding_line_remedy() {
        let prompt = compile_repair_prompt_section(
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 479,
                column: 13,
                message: "the name `player` is defined multiple times".to_string(),
                excerpt: "  359 |       const player = playerRef.current;\n      :             ------ previous definition of `player` here\n  479 |       const player = playerRef.current;\n      :             ------ `player` redefined here".to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "lines 359 and 479 both declare `player`; remove or rename the later redeclaration (line 479); the earlier binding (line 359) is already in scope in this block"
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_const_reassignment_declaration_and_remedy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            "import { useState } from \"react\";\n\
export default function SpaceInvaders() {\n\
  const [playerX, setPlayerX] = useState(0);\n\
  playerX = playerX + 1;\n\
  return <main>{playerX}</main>;\n\
}\n",
        )
        .unwrap();
        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/components/SpaceInvaders.tsx".to_string(),
                line: 4,
                column: 3,
                message: "Type error: Cannot assign to 'playerX' because it is a constant."
                    .to_string(),
                excerpt: "4 |   playerX = playerX + 1;\n  |   ^".to_string(),
                symbol: Some("playerX".to_string()),
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "TypeScript const-reassignment repair for `playerX`: src/components/SpaceInvaders.tsx:4:3"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "Declaration site for `playerX` in src/components/SpaceInvaders.tsx:3: const [playerX, setPlayerX] = useState(0);"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "declare with let, or lift into state if it changes per frame -- keep declaration and all assignments consistent"
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_postcss_plugins_key_remedy() {
        let prompt = compile_repair_prompt_section(
            &[CompileError {
                path: "postcss.config.js".to_string(),
                line: 1,
                column: 1,
                message: "Error: Your custom PostCSS configuration must export a `plugins` key."
                    .to_string(),
                excerpt: "Error: Your custom PostCSS configuration must export a `plugins` key."
                    .to_string(),
                symbol: None,
                route_bound: None,
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(prompt.contains("PostCSS config-format remedy"), "{prompt}");
        assert!(prompt.contains("module.exports = { plugins"), "{prompt}");
    }

    #[test]
    fn compile_repair_prompt_includes_imported_hook_context_for_ts_suggestion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/hooks")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import { useTodos } from \"../hooks/useTodos\";\n\
export default function Page() {\n\
  const todos = useTodos();\n\
  return <button onClick={() => setTodos([...todos.items])}>Save</button>;\n\
}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/hooks/useTodos.ts"),
            "export function useTodos() {\n\
  const items = [] as string[];\n\
  const addTodo = (value: string) => items.push(value);\n\
  return {\n\
    items,\n\
    addTodo,\n\
  };\n\
}\n",
        )
        .unwrap();
        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 4,
                column: 33,
                message: "Cannot find name 'setTodos'. Did you mean 'todos'?".to_string(),
                excerpt: "4 |   return <button onClick={() => setTodos([...todos.items])}>Save</button>;\n  |                                 ^".to_string(),
                symbol: Some("setTodos".to_string()),
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains("Compiler suggestion: Did you mean 'todos'?"),
            "{prompt}"
        );
        assert!(
            prompt
                .contains("Imported definition context for `useTodos` from src/hooks/useTodos.ts:"),
            "{prompt}"
        );
        assert!(prompt.contains("export function useTodos()"), "{prompt}");
        assert!(prompt.contains("return {\nitems,\naddTodo,"), "{prompt}");
        assert!(
            prompt.contains(
                "TypeScript member repair menu: use an exported member, export the missing one, or remove the call."
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_imported_hook_context_for_missing_property() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/hooks")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import { useTodos } from \"../hooks/useTodos\";\n\
export default function Page() {\n\
  const todos = useTodos();\n\
  todos.setTodos([]);\n\
  return <main>{todos.items.length}</main>;\n\
}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/hooks/useTodos.ts"),
            "export function useTodos() {\n\
  const items = [] as string[];\n\
  const addTodo = (value: string) => items.push(value);\n\
  return {\n\
    items,\n\
    addTodo,\n\
  };\n\
}\n",
        )
        .unwrap();
        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 4,
                column: 9,
                message: "Property 'setTodos' does not exist on type '{ items: string[]; addTodo: (value: string) => number; }'."
                    .to_string(),
                excerpt: "4 |   todos.setTodos([]);\n  |         ^".to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt
                .contains("Imported definition context for `useTodos` from src/hooks/useTodos.ts:"),
            "{prompt}"
        );
        assert!(prompt.contains("return {\nitems,\naddTodo,"), "{prompt}");
        assert!(
            prompt.contains(
                "TypeScript member repair menu: use an exported member, export the missing one, or remove the call."
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_imported_hook_api_for_destructured_missing_property() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/hooks")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import { useGameEngine } from \"../hooks/useGameEngine\";\n\
export default function Page() {\n\
  const {\n\
    phase,\n\
    score,\n\
    movePlayer,\n\
  } = useGameEngine();\n\
  return <main>{phase}{score}</main>;\n\
}\n",
        )
        .unwrap();
        let filler = (0..32)
            .map(|index| format!("  const filler{index} = {index};"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            dir.path().join("src/hooks/useGameEngine.ts"),
            format!(
                "export function useGameEngine() {{\n\
  const phase = 'playing';\n\
  const score = 10;\n\
  const restartGame = () => undefined;\n\
{filler}\n\
  return {{\n\
    phase,\n\
    score,\n\
    restartGame,\n\
    CANVAS_WIDTH: 800,\n\
    CANVAS_HEIGHT: 600\n\
  }};\n\
}}\n"
            ),
        )
        .unwrap();
        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 6,
                column: 5,
                message: "Type error: Property 'movePlayer' does not exist on type '{ phase: string; score: number; restartGame: () => undefined; CANVAS_WIDTH: number; CANVAS_HEIGHT: number; }'."
                    .to_string(),
                excerpt: "4 |     phase,\n5 |     score,\n6 |     movePlayer,\n  |     ^".to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "Imported definition context for `useGameEngine` from src/hooks/useGameEngine.ts:"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains("Public API surface for `useGameEngine`:"),
            "{prompt}"
        );
        assert!(prompt.contains("return {\n    phase"), "{prompt}");
        assert!(prompt.contains("restartGame"), "{prompt}");
        assert!(
            prompt.contains(
                "TypeScript member repair menu: call an existing member, or add movePlayer to useGameEngine's definition -- keep both files consistent"
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_imported_class_public_api_for_missing_property() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/lib")).unwrap();
        std::fs::write(
            dir.path().join("src/app/SpaceInvadersGame.tsx"),
            "import { SpaceInvadersEngine } from \"../lib/game-engine\";\n\
export default function SpaceInvadersGame() {\n\
  const canvas = document.createElement('canvas');\n\
  const engine = new SpaceInvadersEngine(canvas);\n\
  engine.onStateChange((state) => console.log(state));\n\
  return <canvas />;\n\
}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/lib/game-engine.ts"),
            "export interface GameState { score: number; status: string; }\n\
export class SpaceInvadersEngine {\n\
  private running = false;\n\
  public start() { this.running = true; }\n\
  public pause() { this.running = false; }\n\
  public reset() { this.running = false; }\n\
  public setKey(key: string, pressed: boolean) { void key; void pressed; }\n\
  public getState(): GameState { return { score: 0, status: 'ready' }; }\n\
  public destroy() { this.running = false; }\n\
}\n",
        )
        .unwrap();

        let prompt = compile_repair_prompt_section_with_root(
            Some(dir.path()),
            &[CompileError {
                path: "src/app/SpaceInvadersGame.tsx".to_string(),
                line: 5,
                column: 10,
                message:
                    "Type error: Property 'onStateChange' does not exist on type 'SpaceInvadersEngine'."
                        .to_string(),
                excerpt:
                    "5 |   engine.onStateChange((state) => console.log(state));\n  |          ^"
                        .to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "Imported definition context for `SpaceInvadersEngine` from src/lib/game-engine.ts:"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains("Public API surface for `SpaceInvadersEngine`"),
            "{prompt}"
        );
        assert!(prompt.contains("public start();"), "{prompt}");
        assert!(prompt.contains("public pause();"), "{prompt}");
        assert!(prompt.contains("public getState(): GameState;"), "{prompt}");
        assert!(
            prompt.contains(
                "call an existing member (e.g. poll getState() from the rAF loop), or add onStateChange to SpaceInvadersEngine's definition -- keep both files consistent"
            ),
            "{prompt}"
        );
    }

    #[test]
    fn compile_repair_prompt_includes_nullability_narrowing_remedy() {
        let prompt = compile_repair_prompt_section(
            &[CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 409,
                column: 7,
                message: "Type error: 'ctx' is possibly 'null'.".to_string(),
                excerpt: "407 |\n408 |       // Render\n409 |       ctx.save();\n|       ^"
                    .to_string(),
                symbol: None,
                route_bound: Some(true),
            }],
            CompileRepairPromptProtection::default(),
        );

        assert!(
            prompt.contains(
                "TypeScript nullability repair for `ctx`: inside the closure at line 409, add `if (!ctx) return;` before first use, or capture a non-null local after the outer check."
            ),
            "{prompt}"
        );
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
    fn structured_contract_splits_multi_grep_verify_commands() {
        let dir = tempfile::tempdir().unwrap();
        let err = CompletionContract {
            required_paths: Vec::new(),
            verify_commands: vec![
                r#"grep -q "alpha" src/report.txt && grep -q "beta" src/report.txt"#.to_string(),
            ],
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

        assert!(err.contains("unsupported fragment"), "{err}");
        assert!(err.contains("allowed categories"), "{err}");
        assert!(!err.contains("may not use shell control syntax"), "{err}");
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
            "npm run build | grep error",
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
    fn restart_terminal_unreached_feedback_offers_in_play_or_accept_partial() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "unverified runtime evidence: restart_or_recoverable_state_evidence:unverified:terminal_state_not_reached"
                .to_string(),
        );

        let feedback = format_verify_feedback(&report);

        assert!(
            feedback.contains(
                "either expose an in-play restart control, or accept the partial classification"
            ),
            "{feedback}"
        );
        assert!(
            feedback.contains("cannot be behaviorally verified by the generic probe"),
            "{feedback}"
        );
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
