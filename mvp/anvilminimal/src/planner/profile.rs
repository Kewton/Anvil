use std::path::Path;

use crate::minimal_loop::build_verifier::{
    BuildVerifierRequirement, CompileError, ForeignToolchainObservation,
};
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupRequirement,
};
use crate::minimal_loop::evidence::required_evidence_for_capability;
use crate::planner::signals;
use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileQualityExpectations {
    pub required_artifacts: Vec<String>,
    pub preferred_verify: Vec<String>,
    pub forbidden_verify: Vec<String>,
    pub dependency_order_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ProfileSnapshot {
    Data(crate::planner::profiles::data::ProfileSnapshot),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseVerificationMode {
    IntermediateInvariant,
    FinalAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileBuildOracle {
    pub command: String,
    pub profile: Option<String>,
    pub requires_dependency_setup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileBehaviorProbeReport {
    pub status: &'static str,
    pub reasons: Vec<String>,
    pub evidence_path: Option<String>,
}

impl ProfileBehaviorProbeReport {
    pub fn pass() -> Self {
        Self {
            status: "pass",
            reasons: Vec::new(),
            evidence_path: None,
        }
    }
}

pub trait DomainProfile: Sync {
    fn id(&self) -> &'static str;

    fn matches(&self, profile: &str) -> bool {
        canonical_profile_name(profile) == self.id()
    }

    fn expected_scaffold_paths(&self, _root: &Path, _goal: &str) -> Vec<String> {
        Vec::new()
    }

    fn setup_scaffold_paths(&self, _root: &Path) -> Vec<String> {
        Vec::new()
    }

    fn complete_scaffold(
        &self,
        _root: &Path,
        _missing_paths: &[String],
    ) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn verify_final(&self, _root: &Path, _goal: &str) -> VerificationReport {
        VerificationReport::pass()
    }

    fn verify_invariant(
        &self,
        _root: &Path,
        _goal: &str,
        _snapshot: &ProfileSnapshot,
    ) -> VerificationReport {
        VerificationReport::pass()
    }

    fn before_phase(&self, _root: &Path) -> anyhow::Result<ProfileSnapshot> {
        Ok(ProfileSnapshot::None)
    }

    fn after_phase(&self, _root: &Path, _snapshot: &ProfileSnapshot) -> VerificationReport {
        VerificationReport::pass()
    }

    fn guidance(&self, _goal: &str) -> Option<String> {
        None
    }

    fn runtime_contract(&self, _intent: &str, _goal: &str) -> String {
        "- Keep changes scoped to the current phase and workspace.".to_string()
    }

    fn generation_rules(&self, _intent: &str) -> Option<&'static str> {
        None
    }

    fn quality_expectations(&self, _root: &Path, _goal: &str) -> ProfileQualityExpectations {
        ProfileQualityExpectations::default()
    }

    fn repair_prompt(
        &self,
        _root: &Path,
        _goal: &str,
        _report: &VerificationReport,
    ) -> Option<String> {
        None
    }

    fn deterministic_repair(
        &self,
        _root: &Path,
        _goal: &str,
        _report: &VerificationReport,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn post_step_repair(&self, _root: &Path, _goal: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn build_oracle(&self, _command: &str) -> Option<ProfileBuildOracle> {
        None
    }

    fn dependency_ready(&self, _root: &Path, _command: &str) -> bool {
        true
    }

    fn dependency_missing_reason(&self, _root: &Path, command: &str) -> String {
        format!("dependency setup missing before `{command}`")
    }

    fn dependency_setup_requirement(
        &self,
        _root: &Path,
        _requirement: &BuildVerifierRequirement,
        _setup_authority: NodeDependencySetupAuthority,
    ) -> Option<NodeDependencySetupRequirement> {
        None
    }

    fn dependency_missing_output(&self, _output: &str) -> bool {
        false
    }

    fn parse_compile_errors(&self, _output: &str) -> Vec<CompileError> {
        Vec::new()
    }

    fn annotate_compile_errors(&self, _root: &Path, _errors: &mut [CompileError]) {}

    fn foreign_toolchain(
        &self,
        _root: &Path,
        _requirement: &BuildVerifierRequirement,
    ) -> Option<ForeignToolchainObservation> {
        None
    }

    fn source_paths(&self, _root: &Path) -> Vec<String> {
        Vec::new()
    }

    fn infer_required_capabilities(&self, _goal: &str) -> Vec<String> {
        Vec::new()
    }

    fn infer_required_evidence(
        &self,
        _goal: &str,
        _required_capabilities: &[String],
    ) -> Vec<String> {
        Vec::new()
    }

    fn infer_required_obligations(
        &self,
        _goal: &str,
        _required_capabilities: &[String],
    ) -> Vec<String> {
        Vec::new()
    }

    fn completion_contract_required(&self, _goal: &str, _required_capabilities: &[String]) -> bool {
        false
    }

    fn behavior_probe(
        &self,
        _root: &Path,
        _goal: &str,
        _required_capabilities: &[String],
        _offline: bool,
    ) -> anyhow::Result<ProfileBehaviorProbeReport> {
        Ok(ProfileBehaviorProbeReport::pass())
    }
}

pub struct NextjsProfile;
pub struct DataProfile;
pub struct GenericProfile;

pub const GENERIC_INTERACTIVE_CONTRACT_CAPABILITY: &str = "generic_interactive_contract";

static NEXTJS_PROFILE: NextjsProfile = NextjsProfile;
static DATA_PROFILE: DataProfile = DataProfile;
static PYTHON_CLI_PROFILE: crate::planner::profiles::python_cli::PythonCliProfile =
    crate::planner::profiles::python_cli::PythonCliProfile;
static GENERIC_PROFILE: GenericProfile = GenericProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileInferenceSource {
    Goal,
    Workspace,
}

impl ProfileInferenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileInference {
    pub profile: &'static str,
    pub source: ProfileInferenceSource,
}

impl ProfileInference {
    pub fn summary_line(self) -> String {
        format!(
            "profile_inferred: {} (from: {})",
            self.profile,
            self.source.as_str()
        )
    }
}

pub fn infer_profile(goal: Option<&str>, workspace_root: &Path) -> Option<ProfileInference> {
    if let Some(goal) = goal {
        if signals::contains_nextjs_goal_token(goal) {
            return Some(ProfileInference {
                profile: NEXTJS_PROFILE.id(),
                source: ProfileInferenceSource::Goal,
            });
        }
        if signals::contains_python_cli_goal_token(goal) {
            return Some(ProfileInference {
                profile: PYTHON_CLI_PROFILE.id(),
                source: ProfileInferenceSource::Goal,
            });
        }
    }
    if package_json_has_dependency(workspace_root, "next") {
        return Some(ProfileInference {
            profile: NEXTJS_PROFILE.id(),
            source: ProfileInferenceSource::Workspace,
        });
    }
    if workspace_root.join("pyproject.toml").is_file() {
        return Some(ProfileInference {
            profile: PYTHON_CLI_PROFILE.id(),
            source: ProfileInferenceSource::Workspace,
        });
    }
    None
}

fn package_json_has_dependency(root: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_object))
        .any(|deps| deps.contains_key(name))
}

impl DomainProfile for NextjsProfile {
    fn id(&self) -> &'static str {
        "nextjs"
    }

    fn matches(&self, profile: &str) -> bool {
        matches!(canonical_profile_name(profile).as_str(), "nextjs")
    }

    fn expected_scaffold_paths(&self, root: &Path, goal: &str) -> Vec<String> {
        crate::planner::profiles::nextjs::expected_paths(root, goal)
    }

    fn setup_scaffold_paths(&self, root: &Path) -> Vec<String> {
        crate::planner::profiles::nextjs::setup_scaffold_paths(root)
    }

    fn complete_scaffold(
        &self,
        root: &Path,
        missing_paths: &[String],
    ) -> anyhow::Result<Vec<String>> {
        crate::planner::profiles::nextjs::complete_scaffold(root, missing_paths)
    }

    fn verify_final(&self, root: &Path, goal: &str) -> VerificationReport {
        crate::planner::profiles::nextjs::verify(root, goal)
    }

    fn verify_invariant(
        &self,
        root: &Path,
        goal: &str,
        _snapshot: &ProfileSnapshot,
    ) -> VerificationReport {
        crate::planner::profiles::nextjs::verify_invariant(root, goal)
    }

    fn guidance(&self, goal: &str) -> Option<String> {
        Some(crate::planner::profiles::nextjs::guidance(goal))
    }

    fn runtime_contract(&self, intent: &str, goal: &str) -> String {
        crate::planner::profiles::nextjs::runtime_contract(intent, goal)
    }

    fn generation_rules(&self, intent: &str) -> Option<&'static str> {
        Some(crate::planner::profiles::nextjs::generation_rules(intent))
    }

    fn quality_expectations(&self, root: &Path, goal: &str) -> ProfileQualityExpectations {
        crate::planner::profiles::nextjs::quality_expectations(root, goal)
    }

    fn repair_prompt(
        &self,
        root: &Path,
        goal: &str,
        report: &VerificationReport,
    ) -> Option<String> {
        Some(crate::planner::profiles::nextjs::repair_prompt(
            root, goal, report,
        ))
    }

    fn deterministic_repair(
        &self,
        root: &Path,
        goal: &str,
        report: &VerificationReport,
    ) -> anyhow::Result<bool> {
        crate::planner::profiles::nextjs::auto_repair(root, goal, report)
    }

    fn post_step_repair(&self, root: &Path, goal: &str) -> anyhow::Result<bool> {
        crate::planner::profiles::nextjs::repair_manifest_coherence(root, goal)
    }

    fn build_oracle(&self, command: &str) -> Option<ProfileBuildOracle> {
        is_nextjs_build_command(command).then(|| ProfileBuildOracle {
            command: command.to_string(),
            profile: Some(self.id().to_string()),
            requires_dependency_setup: true,
        })
    }

    fn dependency_ready(&self, root: &Path, command: &str) -> bool {
        if requires_next_binary(command) {
            if requires_package_manifest(command) && !root.join("package.json").is_file() {
                return false;
            }
            dependency_setup::next_build_dependencies_ready(root)
        } else {
            true
        }
    }

    fn dependency_missing_reason(&self, root: &Path, command: &str) -> String {
        if requires_package_manifest(command) && !root.join("package.json").is_file() {
            return "package.json missing before Next.js build verifier".to_string();
        }
        dependency_setup::next_build_missing_dependency_reason(root)
    }

    fn dependency_setup_requirement(
        &self,
        root: &Path,
        requirement: &BuildVerifierRequirement,
        setup_authority: NodeDependencySetupAuthority,
    ) -> Option<NodeDependencySetupRequirement> {
        Some(dependency_setup::requirement_for_next_build(
            root,
            requirement.profile.as_deref(),
            &requirement.reason,
            setup_authority,
        ))
    }

    fn dependency_missing_output(&self, output: &str) -> bool {
        generic_dependency_missing_output(output)
    }

    fn annotate_compile_errors(&self, root: &Path, errors: &mut [CompileError]) {
        let closure = crate::minimal_loop::import_scan::route_bound_closure(root, self.id());
        for error in errors {
            error.route_bound = Some(closure.contains(Path::new(&error.path)));
        }
    }

    fn foreign_toolchain(
        &self,
        root: &Path,
        requirement: &BuildVerifierRequirement,
    ) -> Option<ForeignToolchainObservation> {
        if !requires_next_binary(&requirement.command) {
            return None;
        }
        if dependency_setup::next_package_ready(root) {
            return None;
        }
        let resolved =
            crate::minimal_loop::verifier_env::foreign_node_modules_bin_on_path(root, "next")?;
        Some(ForeignToolchainObservation {
            tool: "next".to_string(),
            resolved_path: resolved.display().to_string(),
            workspace_root: root.display().to_string(),
            reason: format!(
                "foreign_toolchain_detected: workspace node_modules/next missing; PATH would resolve next outside workspace at {}",
                resolved.display()
            ),
        })
    }

    fn source_paths(&self, root: &Path) -> Vec<String> {
        crate::planner::profiles::nextjs::app_source_paths(root)
    }

    fn infer_required_capabilities(&self, goal: &str) -> Vec<String> {
        let mut capabilities = Vec::new();
        let game_like = signals::contains_game_token(goal);
        let persistence_like = signals::contains_persistence_token(goal);
        let interactive_app_like = signals::contains_interactive_token(goal);
        if game_like {
            merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
            merge_unique_strings(&mut capabilities, &["start_or_restart_flow".to_string()]);
            merge_unique_strings(&mut capabilities, &["player_control".to_string()]);
            merge_unique_strings(&mut capabilities, &["adversary_or_challenge".to_string()]);
            merge_unique_strings(&mut capabilities, &["progression_or_score".to_string()]);
            merge_unique_strings(
                &mut capabilities,
                &["failure_or_collision_rule".to_string()],
            );
        } else if interactive_app_like {
            merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
            merge_unique_strings(&mut capabilities, &["user_input_or_action".to_string()]);
            merge_unique_strings(&mut capabilities, &["visible_state_change".to_string()]);
            if persistence_like {
                merge_unique_strings(&mut capabilities, &["persistence".to_string()]);
            }
        }
        capabilities
    }

    fn infer_required_evidence(
        &self,
        _goal: &str,
        required_capabilities: &[String],
    ) -> Vec<String> {
        let mut evidence = vec![
            "nextjs_route_evidence".to_string(),
            "build_command_or_dependency_missing_boundary".to_string(),
        ];
        for capability in required_capabilities {
            merge_unique_strings(&mut evidence, &required_evidence_for_capability(capability));
        }
        evidence
    }

    fn infer_required_obligations(
        &self,
        goal: &str,
        required_capabilities: &[String],
    ) -> Vec<String> {
        let app_like_goal = signals::contains_app_like_token(goal);
        if app_like_goal || !required_capabilities.is_empty() {
            return vec!["implementation".to_string()];
        }
        Vec::new()
    }

    fn completion_contract_required(&self, _goal: &str, _required_capabilities: &[String]) -> bool {
        true
    }
}

impl DomainProfile for DataProfile {
    fn id(&self) -> &'static str {
        "data"
    }

    fn matches(&self, profile: &str) -> bool {
        matches!(
            canonical_profile_name(profile).as_str(),
            "data" | "data-analysis" | "data-pipeline"
        )
    }

    fn verify_final(&self, root: &Path, _goal: &str) -> VerificationReport {
        crate::planner::profiles::data::verify(root)
    }

    fn before_phase(&self, root: &Path) -> anyhow::Result<ProfileSnapshot> {
        Ok(ProfileSnapshot::Data(
            crate::planner::profiles::data::before_phase(root)?,
        ))
    }

    fn after_phase(&self, root: &Path, snapshot: &ProfileSnapshot) -> VerificationReport {
        match snapshot {
            ProfileSnapshot::Data(snapshot) => {
                crate::planner::profiles::data::after_phase(root, snapshot)
            }
            _ => VerificationReport::pass(),
        }
    }

    fn runtime_contract(&self, _intent: &str, _goal: &str) -> String {
        "- Preserve raw input data.\n\
- Write derived outputs to explicit output artifacts.\n\
- Use deterministic checks for generated files when practical."
            .to_string()
    }
}

impl DomainProfile for GenericProfile {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn matches(&self, _profile: &str) -> bool {
        true
    }

    fn runtime_contract(&self, _intent: &str, _goal: &str) -> String {
        "- Keep changes scoped to the current phase and workspace.".to_string()
    }

    fn generation_rules(&self, profile: &str) -> Option<&'static str> {
        match profile {
            "rust" => Some(
                "- Profile rust: preserve Cargo project semantics. Keep Cargo.toml before cargo check/test verification, do not weaken scripts or tests to hide failures, and end with cargo check or cargo test when practical.\n",
            ),
            "python" => Some(
                "- Profile python: keep dependency setup separate from deterministic verification. Prefer python -m py_compile, pytest, or unittest checks after source files exist. Do not put package installation in verify commands.\n",
            ),
            _ => None,
        }
    }

    fn build_oracle(&self, command: &str) -> Option<ProfileBuildOracle> {
        let lower = command.trim().to_ascii_lowercase();
        let recognized = requires_node_test_runner(command)
            || requires_node_dependency_probe(command)
            || lower == "cargo build"
            || lower.starts_with("cargo build ");
        recognized.then(|| ProfileBuildOracle {
            command: command.to_string(),
            profile: None,
            requires_dependency_setup: requires_node_test_runner(command)
                || requires_node_dependency_probe(command),
        })
    }

    fn dependency_ready(&self, root: &Path, command: &str) -> bool {
        if requires_node_test_runner(command) {
            dependency_setup::node_test_runner_bindable(root)
        } else if dependency_setup::package_json_declares_dependencies(root) {
            dependency_setup::node_declared_dependencies_ready(root)
        } else {
            true
        }
    }

    fn dependency_missing_reason(&self, root: &Path, command: &str) -> String {
        if requires_node_test_runner(command) {
            "package.json scripts.test missing before Node test verifier".to_string()
        } else if dependency_setup::package_json_declares_dependencies(root) {
            dependency_setup::node_declared_dependencies_missing_reason(root)
        } else {
            format!("dependency setup missing before `{command}`")
        }
    }

    fn dependency_setup_requirement(
        &self,
        root: &Path,
        requirement: &BuildVerifierRequirement,
        setup_authority: NodeDependencySetupAuthority,
    ) -> Option<NodeDependencySetupRequirement> {
        if requires_node_test_runner(&requirement.command) {
            return Some(dependency_setup::requirement_for_node_test_runner(
                root,
                requirement.profile.as_deref(),
                &requirement.reason,
                setup_authority,
            ));
        }
        Some(
            dependency_setup::requirement_for_node_declared_dependencies(
                root,
                requirement.profile.as_deref(),
                &requirement.reason,
                setup_authority,
            ),
        )
    }

    fn dependency_missing_output(&self, output: &str) -> bool {
        generic_dependency_missing_output(output)
    }

    fn infer_required_capabilities(&self, goal: &str) -> Vec<String> {
        if generic_app_intent_goal(goal) {
            vec![GENERIC_INTERACTIVE_CONTRACT_CAPABILITY.to_string()]
        } else {
            Vec::new()
        }
    }

    fn infer_required_evidence(&self, goal: &str, required_capabilities: &[String]) -> Vec<String> {
        if generic_app_intent_goal(goal)
            || required_capabilities
                .iter()
                .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        {
            return vec![
                "user_input_handler_evidence".to_string(),
                "stateful_update_evidence".to_string(),
                "visible_interactive_surface_evidence".to_string(),
            ];
        }
        Vec::new()
    }

    fn infer_required_obligations(
        &self,
        goal: &str,
        required_capabilities: &[String],
    ) -> Vec<String> {
        if generic_app_intent_goal(goal)
            || required_capabilities
                .iter()
                .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        {
            return vec!["implementation".to_string()];
        }
        Vec::new()
    }

    fn completion_contract_required(&self, goal: &str, required_capabilities: &[String]) -> bool {
        generic_app_intent_goal(goal)
            || required_capabilities
                .iter()
                .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
    }
}

fn generic_app_intent_goal(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    signals::contains_app_intent_token(goal)
        && !signals::contains_nextjs_goal_token(goal)
        && !signals::contains_python_cli_goal_token(goal)
        && !generic_app_intent_excluded_by_internal_or_artifact_context(&lower)
}

fn generic_app_intent_excluded_by_internal_or_artifact_context(lower: &str) -> bool {
    [
        "app entrypoint",
        "src/app/",
        "required final artifacts:",
        "original ultra goal:",
        "phase id:",
        "phase task:",
        "failed phase:",
        "failed step:",
        "repair target:",
        "recovery handoff",
        "profile:",
    ]
    .iter()
    .any(|token| lower.contains(token))
}

pub fn domain_profile(profile: &str) -> &'static dyn DomainProfile {
    if NEXTJS_PROFILE.matches(profile) {
        &NEXTJS_PROFILE
    } else if PYTHON_CLI_PROFILE.matches(profile) {
        &PYTHON_CLI_PROFILE
    } else if DATA_PROFILE.matches(profile) {
        &DATA_PROFILE
    } else {
        &GENERIC_PROFILE
    }
}

pub fn build_oracle_for_command(
    profile: Option<&str>,
    command: &str,
) -> Option<(&'static dyn DomainProfile, ProfileBuildOracle)> {
    if let Some(profile_name) = profile {
        let profile = domain_profile(profile_name);
        if let Some(oracle) = profile.build_oracle(command) {
            return Some((profile, oracle));
        }
    }
    for profile in [
        &NEXTJS_PROFILE as &'static dyn DomainProfile,
        &GENERIC_PROFILE as &'static dyn DomainProfile,
    ] {
        if let Some(oracle) = profile.build_oracle(command) {
            return Some((profile, oracle));
        }
    }
    None
}

pub fn profile_for_build_requirement(
    requirement: &BuildVerifierRequirement,
) -> &'static dyn DomainProfile {
    if let Some(profile) = requirement.profile.as_deref() {
        return domain_profile(profile);
    }
    build_oracle_for_command(None, &requirement.command)
        .map(|(profile, _)| profile)
        .unwrap_or(&GENERIC_PROFILE)
}

pub fn canonical_profile_name(profile: &str) -> String {
    match profile.trim().to_ascii_lowercase().as_str() {
        "next-js" | "next.js" => "nextjs".to_string(),
        "python" | "python-cli" | "py-cli" | "py" => "python-cli".to_string(),
        other => other.to_string(),
    }
}

pub fn is_nextjs_profile(profile: &str) -> bool {
    NEXTJS_PROFILE.matches(profile)
}

pub fn verify_profile(root: &Path, profile: &str, goal: &str) -> VerificationReport {
    verify_profile_final(root, profile, goal)
}

pub fn verify_profile_final(root: &Path, profile: &str, goal: &str) -> VerificationReport {
    domain_profile(profile).verify_final(root, goal)
}

pub fn verify_profile_invariant(
    root: &Path,
    profile: &str,
    goal: &str,
    snapshot: &ProfileSnapshot,
) -> VerificationReport {
    let profile_impl = domain_profile(profile);
    let snapshot_report = profile_impl.after_phase(root, snapshot);
    if !snapshot_report.is_pass() {
        return snapshot_report;
    }
    profile_impl.verify_invariant(root, goal, snapshot)
}

pub fn profile_before_phase(root: &Path, profile: &str) -> anyhow::Result<ProfileSnapshot> {
    domain_profile(profile).before_phase(root)
}

pub fn profile_after_phase(
    root: &Path,
    profile: &str,
    snapshot: &ProfileSnapshot,
) -> VerificationReport {
    domain_profile(profile).after_phase(root, snapshot)
}

pub fn profile_guidance(profile: &str, goal: &str) -> Option<String> {
    domain_profile(profile).guidance(goal)
}

pub fn profile_runtime_contract(profile: &str, intent: &str, goal: &str) -> String {
    match canonical_profile_name(profile).as_str() {
        "rust" => "- Preserve Cargo.toml and crate entrypoints.\n\
- Prefer cargo check or cargo test for deterministic verification.\n\
- Do not weaken tests or public behavior to hide failures."
            .to_string(),
        "python" => "- Preserve the existing Python package/import layout.\n\
- Keep dependency setup separate from deterministic verification.\n\
- Prefer pytest, unittest, or python -m py_compile checks after source files exist."
            .to_string(),
        "docs" | "documentation" => "- Produce or update documentation artifacts.\n\
- Keep claims grounded in inspected files.\n\
- Avoid source-code changes unless the phase explicitly requires them."
            .to_string(),
        _ => domain_profile(profile).runtime_contract(intent, goal),
    }
}

pub fn profile_generation_rules(profile: &str, intent: &str) -> Option<&'static str> {
    match canonical_profile_name(profile).as_str() {
        "rust" => Some(
            "- Profile rust: preserve Cargo project semantics. Keep Cargo.toml before cargo check/test verification, do not weaken scripts or tests to hide failures, and end with cargo check or cargo test when practical.\n",
        ),
        "python" => Some(
            "- Profile python: keep dependency setup separate from deterministic verification. Prefer python -m py_compile, pytest, or unittest checks after source files exist. Do not put package installation in verify commands.\n",
        ),
        _ => domain_profile(profile).generation_rules(intent),
    }
}

pub fn profile_expected_paths(root: &Path, profile: &str, goal: &str) -> Vec<String> {
    domain_profile(profile).expected_scaffold_paths(root, goal)
}

pub fn profile_setup_scaffold_paths(root: &Path, profile: &str) -> Vec<String> {
    domain_profile(profile).setup_scaffold_paths(root)
}

pub fn profile_complete_scaffold(
    root: &Path,
    profile: &str,
    missing_paths: &[String],
) -> anyhow::Result<Vec<String>> {
    domain_profile(profile).complete_scaffold(root, missing_paths)
}

pub fn profile_quality_expectations(
    root: &Path,
    profile: &str,
    goal: &str,
) -> ProfileQualityExpectations {
    domain_profile(profile).quality_expectations(root, goal)
}

pub fn profile_repair_prompt(
    root: &Path,
    profile: &str,
    goal: &str,
    report: &VerificationReport,
) -> Option<String> {
    domain_profile(profile).repair_prompt(root, goal, report)
}

pub fn profile_auto_repair(
    root: &Path,
    profile: &str,
    goal: &str,
    report: &VerificationReport,
) -> anyhow::Result<bool> {
    domain_profile(profile).deterministic_repair(root, goal, report)
}

pub fn profile_post_step_repair(root: &Path, profile: &str, goal: &str) -> anyhow::Result<bool> {
    domain_profile(profile).post_step_repair(root, goal)
}

pub fn profile_failure(reason: impl Into<String>) -> VerificationReport {
    VerificationReport::profile_failed(reason)
}

fn is_nextjs_build_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized == "npm run build"
        || normalized == "pnpm build"
        || normalized == "yarn build"
        || normalized.starts_with("npm run build ")
        || normalized.starts_with("pnpm build ")
        || normalized.starts_with("yarn build ")
        || normalized.contains("next build")
}

pub fn requires_next_binary(command: &str) -> bool {
    is_nextjs_build_command(command)
}

fn requires_package_manifest(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.starts_with("npm ")
        || normalized.starts_with("pnpm ")
        || normalized.starts_with("yarn ")
}

fn requires_node_test_runner(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    normalized == "npm test"
        || normalized == "npm run test"
        || normalized == "pnpm test"
        || normalized == "yarn test"
        || normalized.starts_with("npm test ")
        || normalized.starts_with("npm run test ")
        || normalized.starts_with("pnpm test ")
        || normalized.starts_with("yarn test ")
}

fn requires_node_dependency_probe(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    (lower.contains("node -e") && lower.contains("require(")) || lower.contains("npx --no-install")
}

fn generic_dependency_missing_output(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("command not found")
        || lower.contains("not found")
        || lower.contains("cannot find module")
        || lower.contains("module not found")
        || lower.contains("modulenotfounderror")
        || lower.contains("can't find crate")
        || lower.contains("no such file or directory")
}

fn merge_unique_strings(out: &mut Vec<String>, incoming: &[String]) {
    for item in incoming {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
}
